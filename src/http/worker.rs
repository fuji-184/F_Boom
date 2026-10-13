use std::time::Instant;

pub(super) struct Ctx {
    pub client: reqwest::Client,
    // Pinjaman dari config yang diperpanjang ke 'static (lihat shard_main).
    // Copy 16 byte, tanpa clone / Arc.
    pub url: &'static str,
}

// Trait lokal: tidak butuh Send/Sync karena semua task jalan via
// spawn_local di thread yang sama (tidak pernah migrasi).
pub(super) trait HttpMethod {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder;
}

pub(super) struct Get;
pub(super) struct Post;
pub(super) struct Put;
pub(super) struct Delete;

impl HttpMethod for Get    { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.get(u)    } }
impl HttpMethod for Post   { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.post(u)   } }
impl HttpMethod for Put    { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.put(u)    } }
impl HttpMethod for Delete { fn build(c: &reqwest::Client, u: &str) -> reqwest::RequestBuilder { c.delete(u) } }

pub(super) trait HttpBody {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder;
}

pub(super) struct NoBody;

// Body menunjuk ke payload config (dipinjam, lifetime diperpanjang).
// Per request: Body::from(&'static str) = bungkus referensi, nol copy.
pub(super) struct JsonBody {
    s: &'static str,
}

impl JsonBody {
    pub fn new(s: &'static str) -> Self {
        Self { s }
    }
}

impl HttpBody for NoBody {
    #[inline(always)]
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder { req }
}

impl HttpBody for JsonBody {
    #[inline(always)]
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        // Tanpa .header() per request: content-type JSON dipasang sekali
        // sebagai default_headers di client (cold). Body dari &'static str
        // = bungkus referensi, nol copy/alloc.
        req.body(self.s)
    }
}

// Share-nothing worker: satu task = satu koneksi logis yang self-drive.
// Tanpa Arc / clone value: caller memberikan raw pointer ke Ctx dan body
// yang hidup selama shard berjalan. Task hanya copy pointer (8 byte)
// lalu deref via unsafe. Tidak ada ref-count, tidak ada clone heap.
// Body dibaca bersama (read-only), aman karena pemilik shard melakukan
// manual drop SETELAH semua task join.
//
// Dijalankan via `spawn_local` (bukan `spawn` global), jadi future TIDAK
// perlu `Send` dan task tidak pernah pindah thread.
//
// SAFETY kontrak (dijamin oleh shard_main):
// - `ctx_ptr` / `body_ptr` valid & tidak dimutasi selama worker berjalan.
// - Semua task jalan di `current_thread` runtime yang sama (satu OS thread),
//   hanya baca, tanpa write bersama.
pub(super) async fn run<M: HttpMethod, B: HttpBody>(
    ctx_ptr: *const Ctx,
    body_ptr: *const B,
    deadline: Instant,
) -> (Vec<u64>, u64) {
    // SAFETY: dijamin valid oleh shard_main sampai semua task selesai.
    let ctx: &Ctx = unsafe { &*ctx_ptr };
    let body: &B = unsafe { &*body_ptr };

    let mut latencies = Vec::with_capacity(4096);
    let mut sent: u64 = 0;

    while Instant::now() < deadline {
        let t0 = Instant::now();
        let req = body.apply(M::build(&ctx.client, ctx.url));

        let ok = match req.send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        };

        sent += 1;
        if ok {
            latencies.push(t0.elapsed().as_nanos() as u64);
        }
    }

    (latencies, sent)
}

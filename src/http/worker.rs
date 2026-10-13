use std::net::SocketAddr;
use std::time::{Duration, Instant};

use http_body_util::Full;

// Body request seragam untuk semua method: Full di atas slice statis.
// GET/DELETE memakai slice kosong (tanpa alokasi); POST/PUT memakai
// payload config yang dipinjam (tanpa clone). Full::new = bungkus biasa.
pub(super) type ReqBody = Full<&'static [u8]>;

// State milik satu thread untuk satu target. Dibuat SEKALI (cold),
// di-Box + raw pointer + manual drop oleh pemilik shard.
// Tanpa clone value: addr Copy, sisanya pinjaman 'static dari config,
// method enum Copy. Nol alokasi heap di dalam struct ini.
pub(super) struct TargetState {
    pub addr:      SocketAddr,
    pub path:      &'static str, // origin-form request target
    pub authority: &'static str, // nilai header Host
    pub method:    MethodKind,
    pub body:      &'static [u8],
    pub timeout:   Duration,
}

#[derive(Copy, Clone)]
pub(super) enum MethodKind {
    Get,
    Post,
    Put,
    Delete,
}

// Share-nothing worker: satu task = satu koneksi logis yang self-drive.
// Tanpa Arc / clone value: caller memberikan raw pointer ke TargetState
// yang hidup selama shard berjalan. Task hanya copy pointer (8 byte)
// lalu deref via unsafe. Manual drop oleh pemilik shard SETELAH join.
//
// Dijalankan via `spawn_local`, jadi future TIDAK perlu `Send` dan task
// tidak pernah pindah thread.
//
// Pola koneksi: handshake SEKALI per koneksi (bukan per request),
// driver connection di-spawn lokal, worker loop kirim-baca sampai
// deadline. Putus di tengah → handshake ulang (reconnect).
// Timeout per request menjaga deadline bila server menahan body.
//
// SAFETY kontrak (dijamin oleh thread_main):
// - `state_ptr` valid & tidak dimutasi selama worker berjalan.
// - Semua task jalan di `current_thread` runtime yang sama (satu OS thread),
//   hanya baca, tanpa write bersama.
pub(super) async fn run(state_ptr: *const TargetState, deadline: Instant) -> (Vec<u64>, u64) {
    // SAFETY: dijamin valid oleh thread_main sampai semua task selesai.
    let st: &TargetState = unsafe { &*state_ptr };

    let mut latencies = Vec::with_capacity(4096);
    let mut sent: u64 = 0;

    'outer: while Instant::now() < deadline {
        // (Re)connect: TcpStream fresh + handshake hyper HTTP/1.
        // Gagal (server down/penuh) = 1 sampel gagal, coba lagi langsung
        // (semantik sama seperti versi reqwest: tanpa sleep).
        let stream = match tokio::net::TcpStream::connect(st.addr).await {
            Ok(s) => s,
            Err(_) => {
                sent += 1;
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, conn) =
            match hyper::client::conn::http1::handshake::<_, ReqBody>(io).await {
                Ok(x) => x,
                Err(_) => {
                    sent += 1;
                    continue;
                }
            };

        // Driver koneksi jalan berdampingan di thread yang sama.
        // Mati sendiri saat koneksi tutup; worker deteksi lewat error send.
        tokio::task::spawn_local(async move {
            let _ = conn.await;
        });

        while Instant::now() < deadline {
            let t0 = Instant::now();
            let req = build_request(st);

            // Timeout melingkupi send + baca body penuh (semantik sama
            // seperti reqwest .timeout(): satu timer per request).
            let r = tokio::time::timeout(st.timeout, async {
                let resp = sender.send_request(req).await.map_err(|_| ())?;
                let ok = resp.status().is_success();
                // Drain body sampai habis agar koneksi reusable (keep-alive).
                // Frame dibuang tanpa ditampung.
                use http_body::Body as _;
                let mut body = std::pin::pin!(resp.into_body());
                loop {
                    match std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
                        Some(Ok(_)) => {}
                        _ => break,
                    }
                }
                Ok::<bool, ()>(ok)
            })
            .await;

            match r {
                // Sukses (status 2xx atau bukan — tetap dihitung terkirim,
                // latency hanya untuk yang sukses, seperti semula).
                Ok(Ok(ok)) => {
                    sent += 1;
                    if ok {
                        latencies.push(t0.elapsed().as_nanos() as u64);
                    }
                }
                // Error hyper / body rusak → koneksi tak sehat, reconnect.
                Ok(Err(())) => {
                    sent += 1;
                    continue 'outer;
                }
                // Timeout → koneksi mungkin masih sehat, pakai terus.
                Err(_) => {
                    sent += 1;
                }
            }
        }
        break;
    }

    (latencies, sent)
}

/// Bangun owned Request per pengiriman. Satu-satunya alokasi di hot path
/// adalah string Uri (~path, kecil, bounded) — minimum yang jujur karena
/// hyper wajib owned Uri. Header Host dari referensi statis (tanpa alokasi),
/// body dari referensi statis (tanpa alokasi), method konstanta (tanpa alokasi).
fn build_request(st: &TargetState) -> http::Request<ReqBody> {
    let method = match st.method {
        MethodKind::Get => http::Method::GET,
        MethodKind::Post => http::Method::POST,
        MethodKind::Put => http::Method::PUT,
        MethodKind::Delete => http::Method::DELETE,
    };

    http::Request::builder()
        .method(method)
        .uri(st.path) // origin-form (/path?query), seperti wrk
        .header(
            http::header::HOST,
            // from_static = bungkus referensi, nol alokasi. Aman: authority
            // sudah divalidasi sekali (cold) sebelum state dibuat.
            http::header::HeaderValue::from_static(st.authority),
        )
        .body(Full::new(st.body))
        // Hanya gagal bila bagian statis invalid — sudah divalidasi di awal.
        .expect("pre-validated request parts must build")
}

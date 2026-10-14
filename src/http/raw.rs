//! Klien HTTP/1.1 mentah di atas TCP (tanpa hyper/reqwest).
//!
//! Satu worker = satu koneksi TCP persisten yang self-drive: tulis request
//! (pre-built bytes), baca respons (header + body), ulangi sampai deadline.
//! Tanpa task driver, tanpa handshake HTTP, tanpa tower layers — future
//! worker murni sekuensial, wakeup minimal.
//!
//! Tanpa clone value: request bytes dimiliki per thread (1 alokasi cold),
//! buffer baca dipakai ulang (clear, tanpa realloc setelah hangat),
//! parsing level byte tanpa konversi UTF-8 / alokasi. Tak ada panic atas
//! byte server: semua indeks di-check.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::bench::{Live, Prep, TargetOut};
use super::worker::MethodKind;

// Batas buffer respons per worker: respons lebih besar = fail + reconnect,
// bukan OOM. Bench realistis (byte–KB) tak pernah menyentuh ini.
const MAX_BUF: usize = 64 << 20;

/// State milik satu thread untuk satu target. Dibuat SEKALI (cold),
/// di-Box + raw pointer + manual drop oleh pemilik shard.
/// request dimiliki (1 alokasi cold); per request hanya dipinjam.
pub(super) struct RawState {
    pub addr: SocketAddr,
    pub request: Vec<u8>,
    pub timeout: Duration,
}

/// Drive satu OS thread untuk SEMUA target (mode raw): LocalSet +
/// spawn_local, tanpa channel global. Cerminan hyper_drive di bench.rs.
pub(super) async fn drive(
    live: &[Live],
    preps: &[Prep],
    t: usize,
) -> Vec<TargetOut> {
    let n = live.len();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            struct Pend {
                idx: usize,
                h: tokio::task::JoinHandle<(Vec<u64>, u64)>,
            }

            let mut state_ptrs: Vec<*const RawState> = Vec::with_capacity(n);
            let mut pend: Vec<Pend> = Vec::new();
            let mut remaining = vec![0usize; n];
            let mut outs: Vec<TargetOut> =
                (0..n).map(|_| TargetOut::default()).collect();

            for (i, (l, prep)) in live.iter().zip(preps.iter()).enumerate() {
                let cfg = &l.cfg;
                let conc = prep.shards[t];
                if conc == 0 {
                    continue;
                }

                let parts = match super::bench::split_url(&cfg.url) {
                    Some(p) => p,
                    // Sudah difilter di benchmark_app; defensif saja.
                    None => {
                        eprintln!("error: unsupported URL, skipping target shard: {}", cfg.url);
                        continue;
                    }
                };

                // Method SEKALI per target per thread (payload diabaikan
                // untuk get/delete, seperti mode hyper).
                let (method, payload): (MethodKind, Option<&str>) = match cfg.method.as_str() {
                    "get" => (MethodKind::Get, None),
                    "delete" => (MethodKind::Delete, None),
                    "post" => (MethodKind::Post, cfg.payload.as_ref().map(|p| p.val.as_str())),
                    "put" => (MethodKind::Put, cfg.payload.as_ref().map(|p| p.val.as_str())),
                    unknown => {
                        eprintln!(
                            "error: unsupported HTTP method '{unknown}'; skipping target shard"
                        );
                        continue;
                    }
                };

                // Satu alokasi request per thread per target (cold, dipindah).
                let request = build_request_bytes(method, parts.authority, parts.path, payload);
                let state_ptr: *const RawState = Box::into_raw(Box::new(RawState {
                    addr: l.addr,
                    request,
                    timeout: std::time::Duration::from_secs(cfg.timeout),
                }));
                state_ptrs.push(state_ptr);

                let before = pend.len();
                for _ in 0..conc {
                    pend.push(Pend {
                        idx: i,
                        h: tokio::task::spawn_local(run(state_ptr, prep.deadline)),
                    });
                }
                remaining[i] = pend.len() - before;
            }

            for p in pend {
                match p.h.await {
                    Ok((lat, s)) => {
                        outs[p.idx].sent += s;
                        outs[p.idx].lat.extend(lat);
                    }
                    Err(_) => eprintln!("warning: http worker task panicked"),
                }
                remaining[p.idx] -= 1;
                if remaining[p.idx] == 0 && outs[p.idx].finished.is_none() {
                    outs[p.idx].finished = Some(std::time::Instant::now());
                }
            }

            // SAFETY: semua task sudah join, tidak ada deref tersisa.
            // Manual drop tepat sekali per Box.
            for ptr in state_ptrs {
                unsafe { drop(Box::from_raw(ptr as *mut RawState)) };
            }

            outs
        })
        .await
}

/// Bangun SELURUH request bytes sekali (cold): request-line + Host +
/// keep-alive + (Content-Length + Content-Type + body). Tanpa format!,
/// tanpa clone — extend mentah + itoa manual.
fn build_request_bytes(
    method: MethodKind,
    authority: &str,
    path: &str,
    payload: Option<&str>,
) -> Vec<u8> {
    let (m, body) = match method {
        MethodKind::Get => ("GET", None),
        MethodKind::Delete => ("DELETE", None),
        MethodKind::Post => ("POST", payload),
        MethodKind::Put => ("PUT", payload),
    };
    let body_len = body.map(|b| b.len()).unwrap_or(0);

    let mut v = Vec::with_capacity(
        m.len() + 1 + path.len() + 12 + authority.len() + 64 + body_len,
    );
    v.extend_from_slice(m.as_bytes());
    v.extend_from_slice(b" ");
    v.extend_from_slice(path.as_bytes());
    v.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    v.extend_from_slice(authority.as_bytes());
    v.extend_from_slice(b"\r\nConnection: keep-alive\r\n");
    if let Some(b) = body {
        v.extend_from_slice(b"Content-Length: ");
        append_usize(&mut v, b.len());
        v.extend_from_slice(b"\r\nContent-Type: application/json\r\n");
    }
    v.extend_from_slice(b"\r\n");
    if let Some(b) = body {
        v.extend_from_slice(b.as_bytes());
    }
    v
}

/// Tulis desimal tanpa format! (tanpa alokasi).
fn append_usize(v: &mut Vec<u8>, mut n: usize) {
    if n == 0 {
        v.push(b'0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    while n > 0 {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    v.extend_from_slice(&tmp[i..]);
}

// Share-nothing worker: satu task = satu koneksi TCP persisten.
// Tanpa handshake HTTP, tanpa task driver — future murni sekuensial
// (tulis → baca → catat → ulangi). Tanpa Arc / clone value: raw pointer
// ke RawState + manual drop oleh pemilik shard (lihat SAFETY di bawah).
//
// SAFETY kontrak (dijamin oleh drive):
// - `state_ptr` valid & tidak dimutasi selama worker berjalan.
// - Semua task jalan di `current_thread` runtime yang sama (satu OS thread),
//   hanya baca, tanpa write bersama.
pub(super) async fn run(state_ptr: *const RawState, deadline: Instant) -> (Vec<u64>, u64) {
    // SAFETY: dijamin valid oleh drive sampai semua task selesai.
    let st: &RawState = unsafe { &*state_ptr };

    let mut latencies = Vec::with_capacity(4096);
    let mut sent: u64 = 0;
    // Buffer baca dipakai ulang: clear per request (kapasitas dipertahankan,
    // tanpa realloc setelah hangat menampung respons terbesar).
    let mut buf = Vec::with_capacity(4096);

    'outer: while Instant::now() < deadline {
        // (Re)connect: TCP fresh langsung siap (tanpa handshake aplikasi).
        // Gagal = 1 sampel gagal, coba lagi langsung.
        let stream = match TcpStream::connect(st.addr).await {
            Ok(s) => s,
            Err(_) => {
                sent += 1;
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let mut stream = stream;

        // t0 ganda sebagai batas deadline: 2x clock/vdso per iterasi
        // (bukan 3x). Semantik identik: iterasi berjalan iff now < deadline.
        loop {
            let t0 = Instant::now();
            if t0 >= deadline {
                break;
            }
            let r = tokio::time::timeout(st.timeout, async {
                stream.write_all(&st.request).await.map_err(|_| ())?;
                read_response(&mut stream, &mut buf).await
            })
            .await;

            match r {
                // Sukses baca penuh. `reuse == false` (server tutup) bukan
                // gagal: tetap dihitung + latency, lalu reconnect bersih
                // tanpa sampel gagal tambahan.
                Ok(Ok((ok, reuse))) => {
                    sent += 1;
                    if ok {
                        latencies.push(t0.elapsed().as_nanos() as u64);
                    }
                    if !reuse {
                        continue 'outer;
                    }
                }
                // Timeout / error tulis-baca → 1 gagal + reconnect.
                _ => {
                    sent += 1;
                    continue 'outer;
                }
            }
        }
        break;
    }

    (latencies, sent)
}

/// Kepala respons ter-parse (level byte, tanpa alokasi).
struct Head {
    code: u16,
    content_len: Option<usize>,
    chunked: bool,
    /// Boleh dipakai ulang (bukan close-delimited).
    reuse: bool,
}

/// Baca SATU respons penuh: header + body. Kembalikan (status-2xx, reusable).
/// Buffer dipakai ulang antar request (tidak ditampung per respons).
async fn read_response(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
) -> Result<(bool, bool), ()> {
    buf.clear();

    // -- 1. header sampai \r\n\r\n --
    let head_end = loop {
        if let Some(p) = find_headers_end(buf) {
            break p;
        }
        if buf.len() >= MAX_BUF {
            return Err(());
        }
        let n = stream.read_buf(buf).await.map_err(|_| ())?;
        if n == 0 {
            return Err(()); // EOF prematur → reconnect
        }
    };

    let head = parse_head(&buf[..head_end])?;
    let mut pos = head_end;

    // 204/304/1xx: tak ada body apa pun. Reuse ikut flag connection.
    if head.code == 204 || head.code == 304 || head.code / 100 == 1 {
        return Ok((head.code / 100 == 2, head.reuse));
    }

    if head.chunked {
        read_chunked(stream, buf, &mut pos).await?;
        return Ok((head.code / 100 == 2, head.reuse));
    }

    if let Some(len) = head.content_len {
        while buf.len() - pos < len {
            if buf.len() >= MAX_BUF {
                return Err(());
            }
            let n = stream.read_buf(buf).await.map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
        }
        return Ok((head.code / 100 == 2, head.reuse));
    }

    // Tanpa framing body (close-delimited): baca sampai EOF seperti wrk,
    // lalu reconnect (tak reusable). Latency mencakup body penuh.
    loop {
        if buf.len() >= MAX_BUF {
            return Err(());
        }
        let n = stream.read_buf(buf).await.map_err(|_| ())?;
        if n == 0 {
            break;
        }
    }
    Ok((head.code / 100 == 2, false))
}

/// Cari akhir header (\r\n\r\n); kembalikan posisi awal body.
fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

/// Parse status-line + header yang relevan. Ketat tapi tanpa panic:
/// apa pun yang malformed → Err (fail + reconnect), bukan crash.
fn parse_head(h: &[u8]) -> Result<Head, ()> {
    let eol = h.iter().position(|&b| b == b'\n').ok_or(())?;
    let mut line = &h[..eol];
    line = line.strip_suffix(b"\r").unwrap_or(line);

    // "HTTP/1.1 200 OK"
    let mut parts = line.split(|&b| b == b' ');
    let ver = parts.next().ok_or(())?;
    let code_b = parts.next().ok_or(())?;
    if code_b.len() != 3 || !code_b.iter().all(|b| b.is_ascii_digit()) {
        return Err(());
    }
    let code: u16 = ((code_b[0] - b'0') as u16) * 100
        + ((code_b[1] - b'0') as u16) * 10
        + (code_b[2] - b'0') as u16;
    // Pola slice eksak: aman tanpa indeks mentah.
    let v11 = matches!(ver, [b'H', b'T', b'T', b'P', b'/', b'1', b'.', b'1']);

    let mut rest = &h[eol + 1..];
    let mut content_len: Option<usize> = None;
    let mut chunked = false;
    let mut close = false;
    let mut keep_alive = false;

    while !rest.is_empty() {
        let e = rest.iter().position(|&b| b == b'\n').ok_or(())?;
        let mut l = &rest[..e];
        rest = &rest[e + 1..];
        l = l.strip_suffix(b"\r").unwrap_or(l);
        if l.is_empty() {
            break;
        }
        let c = l.iter().position(|&b| b == b':').ok_or(())?;
        let (name, mut val) = (&l[..c], &l[c + 1..]);
        while matches!(val.first(), Some(b' ' | b'\t')) {
            val = &val[1..];
        }
        if name.eq_ignore_ascii_case(b"content-length") {
            content_len = Some(parse_usize(val)?);
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            if contains_token(val, b"chunked") {
                chunked = true;
            }
        } else if name.eq_ignore_ascii_case(b"connection") {
            if contains_token(val, b"close") {
                close = true;
            }
            if contains_token(val, b"keep-alive") {
                keep_alive = true;
            }
        }
    }

    Ok(Head {
        code,
        content_len,
        chunked,
        reuse: !close && (v11 || keep_alive),
    })
}

/// Parse desimal ketat (non-digit → Err). Saturating anti-overflow.
fn parse_usize(v: &[u8]) -> Result<usize, ()> {
    if v.is_empty() {
        return Err(());
    }
    let mut n: usize = 0;
    for &b in v {
        if !b.is_ascii_digit() {
            return Err(());
        }
        n = n
            .saturating_mul(10)
            .saturating_add((b - b'0') as usize);
    }
    Ok(n)
}

/// Parse heksadesimal ketat untuk ukuran chunk. Menolak alokasi raksasa.
fn parse_hex(v: &[u8]) -> Result<usize, ()> {
    if v.is_empty() {
        return Err(());
    }
    let mut n: usize = 0;
    for &b in v {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(()),
        };
        n = n
            .saturating_mul(16)
            .saturating_add(d as usize);
        if n > MAX_BUF {
            return Err(());
        }
    }
    Ok(n)
}

/// Substring case-insensitive (untuk token header). Tanpa alokasi.
fn contains_token(hay: &[u8], tok: &[u8]) -> bool {
    hay.len() >= tok.len()
        && (0..=hay.len() - tok.len()).any(|i| {
            hay[i..i + tok.len()]
                .iter()
                .zip(tok.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
        })
}

/// Potong whitespace ASCII di kedua ujung (CRLF/SP/TAB). Tanpa alokasi.
fn trim_ws(mut v: &[u8]) -> &[u8] {
    while matches!(v.first(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        v = &v[1..];
    }
    while matches!(v.last(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        v = &v[..v.len() - 1];
    }
    v
}

/// Baca chunked-encoding sampai chunk 0 + trailer. Posisi (`pos`) maju
/// di dalam buffer bersama; body dibuang (tidak ditampung).
async fn read_chunked(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    pos: &mut usize,
) -> Result<(), ()> {
    loop {
        // Baris ukuran (abaikan extension setelah ';').
        let (s, e) = read_line(stream, buf, pos).await?;
        let line = trim_ws(&buf[s..e]);
        let digits = match line.iter().position(|&b| b == b';') {
            Some(i) => &line[..i],
            None => line,
        };
        let size = parse_hex(trim_ws(digits))?;
        if size == 0 {
            // Trailer sampai baris kosong.
            loop {
                let (ts, te) = read_line(stream, buf, pos).await?;
                if trim_ws(&buf[ts..te]).is_empty() {
                    break;
                }
            }
            return Ok(());
        }
        // Konsumsi size byte + CRLF penutup.
        while buf.len() - *pos < size + 2 {
            if buf.len() >= MAX_BUF {
                return Err(());
            }
            let n = stream.read_buf(buf).await.map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
        }
        if buf.get(*pos + size) != Some(&b'\r') || buf.get(*pos + size + 1) != Some(&b'\n') {
            return Err(());
        }
        *pos += size + 2;
    }
}

/// Baca satu baris (sampai \n inklusif); kembalikan (start, end) dalam buf.
/// Tumbuhkan buffer bila perlu; EOF prematur → Err.
async fn read_line(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    pos: &mut usize,
) -> Result<(usize, usize), ()> {
    loop {
        if let Some(i) = buf[*pos..].iter().position(|&b| b == b'\n') {
            let s = *pos;
            let e = s + i + 1;
            *pos = e;
            return Ok((s, e));
        }
        if buf.len() >= MAX_BUF {
            return Err(());
        }
        let n = stream.read_buf(buf).await.map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
    }
}

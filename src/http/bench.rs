use std::net::{SocketAddr, ToSocketAddrs};

use crate::config::{App, HttpConfig, HttpMode};
use crate::process;
use crate::rt;
use crate::stats::{self, BenchResult};
use super::worker::{self, MethodKind, TargetState};

pub fn run_http(config: crate::config::Config) {
    stats::print_system_info();

    // Share-nothing: tidak ada runtime global. Setiap shard benchmark
    // menjalankan `current_thread` runtime miliknya sendiri di OS thread
    // terpisah (tidak ada work-stealing).
    let mut all_results = Vec::new();
    for app in config.app {
        all_results.extend(run_all(vec![app]));
    }

    for r in &all_results {
        stats::print_result(r);
    }
}

fn run_all(apps: Vec<App>) -> Vec<BenchResult> {
    let mut all_results = Vec::new();

    for app in apps {
        // Start server under test (sync, tanpa runtime).
        let handle = if app.command.is_some() {
            process::spawn(&app)
        } else {
            None
        };

        let http_list = match app.http {
            Some(list) if !list.is_empty() => list,
            _ => {
                if let Some(h) = handle { h.kill(); }
                continue;
            }
        };

        let label = app.command
            .as_ref()
            .map(|c| c.first.as_str())
            .unwrap_or_default();

        // Semua target satu app jalan CONCURRENT (seperti semula),
        // tanpa thread baru: satu scope dipakai bersama (lihat benchmark_app).
        // `label` hanya dipinjam; owned String dibangun sekali per hasil.
        // Thread dipakai bersama satu app: ambil dari app.num_thread.
        // Implementasi klien (hyper/raw) per app: app.http_mode.
        let n_threads = rt::resolve(app.num_thread);
        let mode = app.http_mode;
        all_results.extend(benchmark_app(http_list, label, n_threads, mode));

        if let Some(h) = handle { h.kill(); }
    }

    all_results
}

/// Persiapan per-target yang dihitung sekali di main thread.
/// Dibagi ke raw.rs (pub(super)).
pub(super) struct Prep {
    pub(super) deadline: std::time::Instant,
    /// Konkurrensi shard per OS thread, len == n_threads.
    pub(super) shards: Vec<usize>,
}

/// Target yang lolos filter + alamat sudah resolve (sekali, cold).
/// Dibagi ke raw.rs (pub(super)).
pub(super) struct Live {
    pub(super) cfg: HttpConfig,
    pub(super) addr: SocketAddr,
}

/// Hasil shard satu thread untuk satu target.
/// Dibagi ke raw.rs (pub(super)).
#[derive(Default)]
pub(super) struct TargetOut {
    pub(super) lat: Vec<u64>,
    pub(super) sent: u64,
    /// Waktu task terakhir target ini selesai di thread ini.
    pub(super) finished: Option<std::time::Instant>,
}

/// Benchmark semua target HTTP milik satu app secara CONCURRENT tanpa
/// thread baru: SATU `thread::scope` (N OS threads) dipakai bersama untuk
/// semua target. Tiap thread membangun koneksi + worker untuk tiap target
/// dan menjalankannya sekaligus via `LocalSet` + `spawn_local`.
/// N berasal dari app.num_thread (default = jumlah core).
///
/// Transport: hyper HTTP/1 langsung (tanpa reqwest). Mendukung skema
/// `http://` saja; `https://` dan mode `http2`-only dilewati dengan pesan
/// yang jelas (bukan diam-diam).
fn benchmark_app(
    targets: Vec<HttpConfig>,
    label: &str,
    n_threads: usize,
    mode: HttpMode,
) -> Vec<BenchResult> {
    // Filter + resolve sekali (cold). Target tak didukung dilewati LOUD.
    let mut live: Vec<Live> = Vec::with_capacity(targets.len());
    for cfg in targets.into_iter().filter(|c| c.max_concurrent > 0) {
        match resolve_target(&cfg) {
            Some(addr) => live.push(Live { cfg, addr }),
            None => {}
        }
    }
    if live.is_empty() {
        return Vec::new();
    }

    let n_threads = n_threads.max(1);

    for l in &live {
        println!(
            "\n Benchmarking {} {} on {} | concurrency={} | duration={}s (share-nothing, {} threads, concurrent)",
            l.cfg.method.to_uppercase(),
            match mode {
                HttpMode::Hyper => "hyper-direct",
                HttpMode::Raw => "raw-tcp",
            },
            l.cfg.url, l.cfg.max_concurrent, l.cfg.max_duration, n_threads,
        );
    }

    let start = std::time::Instant::now();

    let preps: Vec<Prep> = live
        .iter()
        .map(|l| Prep {
            deadline: start + std::time::Duration::from_secs(l.cfg.max_duration),
            shards: rt::shard(l.cfg.max_concurrent as usize, n_threads),
        })
        .collect();

    // --- Satu fan-out untuk semua target ---
    // scope meminjam `live`/`preps` tanpa clone; keduanya hidup sampai
    // join selesai, baru `live` dipindah ke hasil.
    let mut acc: Vec<TargetOut> = (0..live.len()).map(|_| TargetOut::default()).collect();
    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(n_threads);
        for t in 0..n_threads {
            // Lewati thread yang tidak kebagian tugas sama sekali.
            if preps.iter().all(|p| p.shards[t] == 0) {
                continue;
            }
            let live = &live;
            let preps = &preps;
            handles.push(s.spawn(move || -> Vec<TargetOut> {
                let rt = rt::build_current_thread_rt();
                rt.block_on(thread_main(live, preps, t, mode))
            }));
        }

        for h in handles {
            match h.join() {
                Ok(v) => {
                    for (a, o) in acc.iter_mut().zip(v) {
                        a.sent += o.sent;
                        a.lat.extend(o.lat);
                        a.finished = match (a.finished, o.finished) {
                            (Some(x), Some(y)) => Some(x.max(y)),
                            (Some(x), None) => Some(x),
                            (None, Some(y)) => Some(y),
                            (None, None) => None,
                        };
                    }
                }
                Err(_) => eprintln!("warning: http shard thread panicked; partial results kept"),
            }
        }
    });

    // Satu hasil per target, url dipindah (tanpa clone).
    live.into_iter()
        .enumerate()
        .map(|(i, l)| {
            let mut a = std::mem::take(&mut acc[i]);
            a.lat.sort_unstable();
            // Durasi = selesainya task terakhir target ini; fallback ke
            // deadline bila target tak pernah jalan.
            let end = a.finished.unwrap_or(preps[i].deadline);
            BenchResult {
                label: label.into(),
                url: l.cfg.url,
                duration: end.saturating_duration_since(start),
                times_ns: a.lat,
                total_sent: a.sent,
            }
        })
        .collect()
}

/// Potongan URL tanpa alokasi (pinjaman dari string config).
/// Dibagi ke raw.rs (pub(super)).
pub(super) struct UrlParts<'a> {
    pub(super) host: &'a str,
    pub(super) port: u16,
    pub(super) authority: &'a str, // host[:port] — untuk header Host
    pub(super) path: &'a str,      // origin-form, "/" bila kosong
}

/// Pecah `http://host[:port][/path]` manual: tanpa parse + alokasi penuh.
/// Kembalikan None untuk skema selain http (https tidak didukung di
/// mode hyper-direct maupun raw) atau bentuk tak valid.
/// Dibagi ke raw.rs (pub(super)).
pub(super) fn split_url(url: &str) -> Option<UrlParts<'_>> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return None;
    }
    let (host, port) = if let Some(after_bracket) = authority.strip_prefix('[') {
        // IPv6 [::1]:port
        let end = after_bracket.find(']')?;
        let port_str = after_bracket[end + 1..].strip_prefix(':').unwrap_or("");
        let port = if port_str.is_empty() { 80 } else { port_str.parse().ok()? };
        (&after_bracket[..end], port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        // host:port (IPv6 tanpa kurung tidak valid di URL — gagal loud di bawah)
        (h, if p.is_empty() { 80 } else { p.parse().ok()? })
    } else {
        (authority, 80)
    };
    if host.is_empty() {
        return None;
    }
    Some(UrlParts { host, port, authority, path })
}

/// Validasi + resolve target SEKALI (cold, main thread): mode, skema,
/// DNS. Gagal = pesan jelas + target dilewati (tanpa hasil).
fn resolve_target(cfg: &HttpConfig) -> Option<SocketAddr> {
    let has = |s: &str| cfg.mode.iter().any(|m| m == s);
    if has("http2") && !has("http3") && !has("http1") {
        eprintln!(
            "error: http2-only mode unsupported in hyper-direct mode; skipping {}",
            cfg.url
        );
        return None;
    }

    let parts = match split_url(&cfg.url) {
        Some(p) => p,
        None => {
            eprintln!(
                "error: unsupported URL (want http://host[:port][/path], no https): {}",
                cfg.url
            );
            return None;
        }
    };

    // Resolve blocking SEKALI per target (cold) — lebih hemat daripada
    // resolve per koneksi baru seperti klien pooling pada umumnya.
    match (parts.host, parts.port).to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(addr) => Some(addr),
            None => {
                eprintln!("error: DNS resolved nothing for {}: {}", cfg.url, parts.host);
                None
            }
        },
        Err(e) => {
            eprintln!("error: cannot resolve {}: {e}", cfg.url);
            None
        }
    }
}

/// Satu OS thread: menjalankan shard dari SEMUA target sekaligus dalam
/// satu `LocalSet`. Dispatch SEKALI ke implementasi klien sesuai
/// app.http_mode (hyper dipertahankan, raw = TCP mentah).
async fn thread_main(
    live: &[Live],
    preps: &[Prep],
    t: usize,
    mode: HttpMode,
) -> Vec<TargetOut> {
    match mode {
        HttpMode::Hyper => hyper_drive(live, preps, t).await,
        HttpMode::Raw => super::raw::drive(live, preps, t).await,
    }
}

/// Drive mode hyper: tanpa Arc / clone value — tiap target di-Box sekali
/// (TargetState) sebagai raw pointer, manual drop setelah semua join.
/// `path`/`authority`/body hanya pinjaman dari `cfg` (lihat SAFETY).
async fn hyper_drive(
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

            let mut state_ptrs: Vec<*const TargetState> = Vec::with_capacity(n);
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

                // Sudah difilter di benchmark_app; di sini defensif saja.
                let parts = match split_url(&cfg.url) {
                    Some(p) => p,
                    None => {
                        eprintln!("error: unsupported URL, skipping target shard: {}", cfg.url);
                        continue;
                    }
                };

                // Validasi dingin sekali per thread: path untuk Uri,
                // authority untuk HeaderValue::from_static (yang panic
                // bila invalid — jangan sampai terjadi di hot path).
                if parts.path.parse::<http::Uri>().is_err()
                    || parts.authority.parse::<http::header::HeaderValue>().is_err()
                {
                    eprintln!("error: invalid request target, skipping shard: {}", cfg.url);
                    continue;
                }

                // SAFETY: `cfg` dipinjam dari frame benchmark_app dan dijamin
                // hidup sampai seluruh shard join (thread::scope), yaitu
                // sebelum `live` dipindah ke hasil. Hanya dibaca.
                let path: &'static str = unsafe { &*(parts.path as *const str) };
                let authority: &'static str =
                    unsafe { &*(parts.authority as *const str) };
                let payload: &'static [u8] = match cfg.payload.as_ref() {
                    Some(p) => unsafe { &*(p.val.as_bytes() as *const [u8]) },
                    None => b"".as_slice(),
                };

                // Dispatch method SEKALI per target per thread.
                // GET/DELETE mengabaikan payload (seperti semula).
                let (method, body): (MethodKind, &'static [u8]) = match cfg.method.as_str() {
                    "get" => (MethodKind::Get, b"".as_slice()),
                    "delete" => (MethodKind::Delete, b"".as_slice()),
                    "post" => (MethodKind::Post, payload),
                    "put" => (MethodKind::Put, payload),
                    unknown => {
                        eprintln!(
                            "error: unsupported HTTP method '{unknown}'; skipping target shard"
                        );
                        continue;
                    }
                };

                // Satu Box per target per thread (dipindah, bukan clone).
                let state_ptr: *const TargetState =
                    Box::into_raw(Box::new(TargetState {
                        addr: l.addr,
                        path,
                        authority,
                        method,
                        body,
                        timeout: std::time::Duration::from_secs(cfg.timeout),
                    }));
                state_ptrs.push(state_ptr);

                let before = pend.len();
                for _ in 0..conc {
                    pend.push(Pend {
                        idx: i,
                        h: tokio::task::spawn_local(worker::run(state_ptr, prep.deadline)),
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
                unsafe { drop(Box::from_raw(ptr as *mut TargetState)) };
            }

            outs
        })
        .await
}

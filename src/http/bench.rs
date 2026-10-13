use crate::config::HttpConfig;
use crate::process;
use crate::rt;
use crate::stats::{self, BenchResult};
use super::worker::{self, Ctx, Delete, Get, JsonBody, NoBody, Post, Put};
use crate::config::App;

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
        all_results.extend(benchmark_app(http_list, label));

        if let Some(h) = handle { h.kill(); }
    }

    all_results
}

/// Persiapan per-target yang dihitung sekali di main thread.
struct Prep {
    deadline: std::time::Instant,
    http2_only: bool,
    /// Konkurrensi shard per OS thread, len == n_threads.
    shards: Vec<usize>,
}

/// Hasil shard satu thread untuk satu target.
#[derive(Default)]
struct TargetOut {
    lat: Vec<u64>,
    sent: u64,
    /// Waktu task terakhir target ini selesai di thread ini.
    finished: Option<std::time::Instant>,
}

/// Benchmark semua target HTTP milik satu app secara CONCURRENT tanpa
/// thread baru: SATU `thread::scope` (N OS threads) dipakai bersama untuk
/// semua target. Tiap thread membangun client + worker untuk tiap target
/// dan menjalankannya sekaligus via `LocalSet` + `spawn_local`.
fn benchmark_app(targets: Vec<HttpConfig>, label: &str) -> Vec<BenchResult> {
    // Target dengan concurrency 0 dilewati (seperti semula: tanpa hasil).
    let targets: Vec<HttpConfig> = targets
        .into_iter()
        .filter(|c| c.max_concurrent > 0)
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }

    // Satu pool thread untuk semua target: tanpa thread baru.
    let total_conc: usize = targets.iter().map(|c| c.max_concurrent as usize).sum();
    let n_threads = rt::num_threads(total_conc.max(1));

    for c in &targets {
        println!(
            "\n Benchmarking {} {} on {} | concurrency={} | duration={}s (share-nothing, {} threads, concurrent)",
            c.method.to_uppercase(), c.mode.join("+"),
            c.url, c.max_concurrent, c.max_duration, n_threads,
        );
    }

    let start = std::time::Instant::now();

    let preps: Vec<Prep> = targets
        .iter()
        .map(|c| {
            let has = |s: &str| c.mode.iter().any(|m| m == s);
            Prep {
                deadline: start + std::time::Duration::from_secs(c.max_duration),
                http2_only: has("http2") && !has("http3") && !has("http1"),
                shards: rt::shard(c.max_concurrent as usize, n_threads),
            }
        })
        .collect();

    // --- Satu fan-out untuk semua target ---
    // scope meminjam `targets`/`preps` tanpa clone; keduanya hidup sampai
    // join selesai, baru `targets` dipindah ke hasil.
    let mut acc: Vec<TargetOut> = (0..targets.len()).map(|_| TargetOut::default()).collect();
    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(n_threads);
        for t in 0..n_threads {
            // Lewati thread yang tidak kebagian tugas sama sekali.
            if preps.iter().all(|p| p.shards[t] == 0) {
                continue;
            }
            let targets = &targets;
            let preps = &preps;
            handles.push(s.spawn(move || -> Vec<TargetOut> {
                let rt = rt::build_current_thread_rt();
                rt.block_on(thread_main(targets, preps, t))
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
    targets
        .into_iter()
        .enumerate()
        .map(|(i, cfg)| {
            let mut a = std::mem::take(&mut acc[i]);
            a.lat.sort_unstable();
            // Durasi = selesainya task terakhir target ini; fallback ke
            // deadline bila target tak pernah jalan (mis. client gagal total).
            let end = a.finished.unwrap_or(preps[i].deadline);
            BenchResult {
                label: label.into(),
                url: cfg.url,
                duration: end.saturating_duration_since(start),
                times_ns: a.lat,
                total_sent: a.sent,
            }
        })
        .collect()
}

/// Satu OS thread: menjalankan shard dari SEMUA target sekaligus dalam
/// satu `LocalSet`. Tanpa Arc / clone value: tiap target di-Box sekali
/// (Ctx + body) sebagai raw pointer, manual drop setelah semua join.
/// `url`/payload hanya pinjaman dari `cfg` (lihat SAFETY di bawah).
async fn thread_main(
    targets: &[HttpConfig],
    preps: &[Prep],
    t: usize,
) -> Vec<TargetOut> {
    let n = targets.len();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            struct Pend {
                idx: usize,
                h: tokio::task::JoinHandle<(Vec<u64>, u64)>,
            }

            let mut ctx_ptrs: Vec<*const Ctx> = Vec::with_capacity(n);
            let mut nobody_ptrs: Vec<*const NoBody> = Vec::new();
            let mut json_ptrs: Vec<*const JsonBody> = Vec::new();
            let mut pend: Vec<Pend> = Vec::new();
            let mut remaining = vec![0usize; n];
            let mut outs: Vec<TargetOut> =
                (0..n).map(|_| TargetOut::default()).collect();

            for (i, (cfg, prep)) in targets.iter().zip(preps.iter()).enumerate() {
                let conc = prep.shards[t];
                if conc == 0 {
                    continue;
                }

                let client = match build_client(cfg, conc, prep, t) {
                    Some(c) => c,
                    None => continue, // TargetOut tetap kosong; fallback durasi di akhir.
                };

                // SAFETY: `cfg` dipinjam dari frame benchmark_app dan dijamin
                // hidup sampai seluruh shard join (thread::scope), yaitu
                // sebelum `targets` dipindah ke hasil. Hanya dibaca.
                let url: &'static str = unsafe { &*(cfg.url.as_str() as *const str) };
                let body_str: &'static str = match cfg.payload.as_ref() {
                    Some(p) => unsafe { &*(p.val.as_str() as *const str) },
                    None => "{}",
                };

                // Satu Box per target per thread (dipindah, bukan clone).
                let ctx_ptr: *const Ctx =
                    Box::into_raw(Box::new(Ctx { client, url }));
                ctx_ptrs.push(ctx_ptr);

                // Dispatch method SEKALI per target per thread.
                let before = pend.len();
                match cfg.method.as_str() {
                    "get" => {
                        let b: *const NoBody = Box::into_raw(Box::new(NoBody));
                        nobody_ptrs.push(b);
                        for _ in 0..conc {
                            pend.push(Pend {
                                idx: i,
                                h: tokio::task::spawn_local(worker::run::<Get, NoBody>(
                                    ctx_ptr,
                                    b,
                                    prep.deadline,
                                )),
                            });
                        }
                    }
                    "delete" => {
                        let b: *const NoBody = Box::into_raw(Box::new(NoBody));
                        nobody_ptrs.push(b);
                        for _ in 0..conc {
                            pend.push(Pend {
                                idx: i,
                                h: tokio::task::spawn_local(worker::run::<Delete, NoBody>(
                                    ctx_ptr,
                                    b,
                                    prep.deadline,
                                )),
                            });
                        }
                    }
                    "post" => {
                        let b: *const JsonBody =
                            Box::into_raw(Box::new(JsonBody::new(body_str)));
                        json_ptrs.push(b);
                        for _ in 0..conc {
                            pend.push(Pend {
                                idx: i,
                                h: tokio::task::spawn_local(worker::run::<Post, JsonBody>(
                                    ctx_ptr,
                                    b,
                                    prep.deadline,
                                )),
                            });
                        }
                    }
                    "put" => {
                        let b: *const JsonBody =
                            Box::into_raw(Box::new(JsonBody::new(body_str)));
                        json_ptrs.push(b);
                        for _ in 0..conc {
                            pend.push(Pend {
                                idx: i,
                                h: tokio::task::spawn_local(worker::run::<Put, JsonBody>(
                                    ctx_ptr,
                                    b,
                                    prep.deadline,
                                )),
                            });
                        }
                    }
                    unknown => {
                        eprintln!(
                            "error: unsupported HTTP method '{unknown}'; skipping target shard"
                        );
                    }
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
            for ptr in ctx_ptrs {
                unsafe { drop(Box::from_raw(ptr as *mut Ctx)) };
            }
            for ptr in nobody_ptrs {
                unsafe { drop(Box::from_raw(ptr as *mut NoBody)) };
            }
            for ptr in json_ptrs {
                unsafe { drop(Box::from_raw(ptr as *mut JsonBody)) };
            }

            outs
        })
        .await
}

/// Client per thread per target (dibangun fresh, bukan clone).
fn build_client(
    cfg: &HttpConfig,
    conc: usize,
    prep: &Prep,
    t: usize,
) -> Option<reqwest::Client> {
    let dur = std::time::Duration::from_secs(cfg.max_duration);

    let mut builder = reqwest::Client::builder()
        .pool_max_idle_per_host(conc)
        .pool_idle_timeout(dur)
        .tcp_keepalive(dur)
        .timeout(std::time::Duration::from_secs(cfg.timeout))
        .tcp_nodelay(true);

    builder = if prep.http2_only {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    };

    // Content-type JSON dipasang SEKALI di sini (cold, tanpa alloc berulang
    // via from_static). Hot path per request jadi tanpa .header() — reqwest
    // hanya clone HeaderValue (O(1), tanpa alloc) saat merge defaults.
    if cfg.method.as_str() == "post" || cfg.method.as_str() == "put" {
        let mut defaults = reqwest::header::HeaderMap::new();
        defaults.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        builder = builder.default_headers(defaults);
    }

    match builder.build() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("error: [thread {t}] failed to build HTTP client for {}: {e}", cfg.url);
            None
        }
    }
}

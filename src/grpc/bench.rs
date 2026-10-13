use crate::config::GrpcConfig;
use crate::rt;
use crate::stats::{self, BenchResult};
use crate::grpc::proto;

pub fn run_grpc(config: crate::config::Config) {
    stats::print_system_info();

    // Share-nothing: tanpa runtime global. Tiap target di-shard ke
    // beberapa OS thread dengan `current_thread` runtime masing-masing.
    for app in config.app {
        let n_threads = rt::resolve(app.num_thread);
        let grpc_list = match app.grpc {
            Some(l) if !l.is_empty() => l,
            _ => continue,
        };
        for cfg in grpc_list {
            let result = match cfg.mode.as_str() {
                "ping"          => benchmark_ping(cfg, n_threads),
                "2_way_stream"  => benchmark_stream(cfg, n_threads),
                other => {
                    eprintln!("error: unknown gRPC mode '{other}'; skipping");
                    continue;
                }
            };
            stats::print_result(&result);
        }
    }
}

/// Pinjaman url config yang diperpanjang ke 'static: dibagikan ke tiap
/// thread/task sebagai salinan 16 byte (Copy) — tanpa clone, tanpa Arc.
/// SAFETY: pemanggil menjamin `cfg` hidup sampai seluruh shard join
/// (thread::scope), sebelum `cfg.url` dipindah ke hasil.
macro_rules! shard_url {
    ($cfg:expr) => {{
        unsafe { &*($cfg.url.as_str() as *const str) }
    }};
}

fn benchmark_ping(cfg: GrpcConfig, n_threads: usize) -> BenchResult {
    let n_threads = n_threads.max(1);
    println!(
        "\n Benchmarking gRPC ping on {} | concurrency={} | duration={}s (share-nothing, {} threads)",
        cfg.url, cfg.max_concurrent, cfg.max_duration, n_threads,
    );

    let conc = cfg.max_concurrent as usize;
    if conc == 0 {
        return empty_result("grpc-ping", &cfg.url);
    }

    let dur = std::time::Duration::from_secs(cfg.max_duration);
    let start = std::time::Instant::now();
    let deadline = start + dur;

    let shards = rt::shard(conc, n_threads);
    let url: &'static str = shard_url!(cfg);

    let (mut times_ns, total_sent) = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(shards.len());
        for shard_conc in shards {
            if shard_conc == 0 {
                continue;
            }
            handles.push(s.spawn(move || -> (Vec<u64>, u64) {
                let rt = rt::build_current_thread_rt();
                rt.block_on(ping_shard_main(url, shard_conc, deadline))
            }));
        }

        let mut times_ns: Vec<u64> = Vec::with_capacity(1 << 14);
        let mut total_sent: u64 = 0;
        for h in handles {
            match h.join() {
                Ok((mut lat, sent)) => {
                    total_sent += sent;
                    times_ns.append(&mut lat);
                }
                Err(_) => eprintln!("warning: grpc-ping shard thread panicked; partial results kept"),
            }
        }
        (times_ns, total_sent)
    });

    times_ns.sort_unstable();

    BenchResult {
        label: "grpc-ping".into(),
        url: cfg.url,
        duration: start.elapsed(),
        times_ns,
        total_sent,
    }
}

fn benchmark_stream(cfg: GrpcConfig, n_threads: usize) -> BenchResult {
    let n_threads = n_threads.max(1);
    println!(
        "\n Benchmarking gRPC bidirectional stream on {} | concurrency={} | duration={}s (share-nothing, {} threads)",
        cfg.url, cfg.max_concurrent, cfg.max_duration, n_threads,
    );

    let conc = cfg.max_concurrent as usize;
    if conc == 0 {
        return empty_result("grpc-stream", &cfg.url);
    }

    let dur = std::time::Duration::from_secs(cfg.max_duration);
    let start = std::time::Instant::now();
    let deadline = start + dur;

    let shards = rt::shard(conc, n_threads);
    let url: &'static str = shard_url!(cfg);

    let (mut times_ns, total_sent) = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(shards.len());
        for shard_conc in shards {
            if shard_conc == 0 {
                continue;
            }
            handles.push(s.spawn(move || -> (Vec<u64>, u64) {
                let rt = rt::build_current_thread_rt();
                rt.block_on(stream_shard_main(url, shard_conc, deadline))
            }));
        }

        let mut times_ns: Vec<u64> = Vec::with_capacity(1 << 14);
        let mut total_sent: u64 = 0;
        for h in handles {
            match h.join() {
                Ok((mut lat, s)) => {
                    total_sent += s;
                    times_ns.append(&mut lat);
                }
                Err(_) => eprintln!("warning: grpc-stream shard thread panicked; partial results kept"),
            }
        }
        (times_ns, total_sent)
    });

    times_ns.sort_unstable();

    BenchResult {
        label: "grpc-stream".into(),
        url: cfg.url,
        duration: start.elapsed(),
        times_ns,
        total_sent,
    }
}

/// Satu shard via `LocalSet` + `spawn_local`: task terikat di thread ini,
/// tanpa syarat `Send`. Tanpa clone value — url hanya salinan pointer.
async fn ping_shard_main(
    url: &'static str,
    shard_conc: usize,
    deadline: std::time::Instant,
) -> (Vec<u64>, u64) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut handles = Vec::with_capacity(shard_conc);
            for _ in 0..shard_conc {
                handles.push(tokio::task::spawn_local(ping_worker(url, deadline)));
            }

            let mut latencies = Vec::with_capacity(1024 * shard_conc.max(1));
            let mut sent: u64 = 0;
            for h in handles {
                match h.await {
                    Ok((mut lat, s)) => {
                        sent += s;
                        latencies.append(&mut lat);
                    }
                    Err(_) => eprintln!("warning: grpc-ping worker task panicked"),
                }
            }
            (latencies, sent)
        })
        .await
}

/// Sama seperti ping: lokal, tanpa clone value.
async fn stream_shard_main(
    url: &'static str,
    shard_conc: usize,
    deadline: std::time::Instant,
) -> (Vec<u64>, u64) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut handles = Vec::with_capacity(shard_conc);
            for _ in 0..shard_conc {
                handles.push(tokio::task::spawn_local(stream_worker(url, deadline)));
            }

            let mut latencies = Vec::with_capacity(1024 * shard_conc.max(1));
            let mut sent: u64 = 0;
            for h in handles {
                match h.await {
                    Ok((mut lat, s)) => {
                        sent += s;
                        latencies.append(&mut lat);
                    }
                    Err(_) => eprintln!("warning: grpc-stream worker task panicked"),
                }
            }
            (latencies, sent)
        })
        .await
}

async fn ping_worker(url: &'static str, deadline: std::time::Instant) -> (Vec<u64>, u64) {
    let mut client =
        match proto::echo::echo_client::EchoClient::connect(url).await {
            Ok(c)  => c,
            Err(e) => {
                eprintln!("error: gRPC connect to '{url}' failed: {e}");
                return (Vec::new(), 0);
            }
        };

    let mut latencies = Vec::with_capacity(1024);
    let mut sent: u64 = 0;

    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();
        // prost mewajibkan owned String per request. Isi dikosongkan
        // (String::new = tanpa alokasi): yang diukur adalah latency
        // transport, bukan isi payload. Jangan isi literal di sini —
        // itu 1 alokasi kecil per request.
        let req = tonic::Request::new(proto::echo::PingRequest { message: String::new() });
        match client.ping(req).await {
            Ok(_) => {
                sent += 1;
                latencies.push(t0.elapsed().as_nanos() as u64);
            }
            Err(e) => {
                eprintln!("warning: gRPC ping error: {e}");
                sent += 1;
            }
        }
    }

    (latencies, sent)
}

async fn stream_worker(url: &'static str, deadline: std::time::Instant) -> (Vec<u64>, u64) {
    let mut client =
        match proto::stream::stream_client::StreamClient::connect(url).await {
            Ok(c)  => c,
            Err(e) => {
                eprintln!("error: gRPC connect to '{url}' failed: {e}");
                return (Vec::new(), 0);
            }
        };

    // repeat_with: tiap item dibangun dari closure tanpa syarat Clone
    // (tidak seperti repeat). Message dikosongkan: tanpa alokasi —
    // lihat komentar di ping_worker.
    let outbound = futures_util::stream::repeat_with(|| proto::stream::StreamRequest {
        message: String::new(),
    });

    let response = match client.chat(outbound).await {
        Ok(r)  => r,
        Err(e) => {
            eprintln!("error: gRPC stream setup failed: {e}");
            return (Vec::new(), 0);
        }
    };

    let (_, mut rx_stream, _) = response.into_parts();

    let mut latencies = Vec::with_capacity(1024);
    let mut sent: u64 = 0;

    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();
        match rx_stream.message().await {
            Ok(Some(_)) => {
                sent += 1;
                latencies.push(t0.elapsed().as_nanos() as u64);
            }
            Ok(None) => break, // server closed stream
            Err(e) => {
                eprintln!("warning: gRPC stream recv error: {e}");
                sent += 1;
            }
        }
    }

    (latencies, sent)
}

fn empty_result(label: &str, url: &str) -> BenchResult {
    BenchResult {
        label: label.into(),
        url: url.into(),
        duration: std::time::Duration::ZERO,
        times_ns: Vec::new(),
        total_sent: 0,
    }
}

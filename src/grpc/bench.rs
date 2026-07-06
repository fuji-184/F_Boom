use tokio_util::sync::CancellationToken;

use crate::config::GrpcConfig;
use crate::stats::{self, BenchResult};
use crate::grpc::proto;

#[derive(Copy, Clone)]
struct Sample {
    latency_ns: u64,
    sent: u64,
}

impl Sample {
    fn ok(ns: u64) -> Self { Self { latency_ns: ns, sent: 1 } }
    fn fail()      -> Self { Self { latency_ns: 0,  sent: 1 } }
    fn is_ok(self) -> bool { self.latency_ns > 0 }
}

pub fn run_grpc(config: crate::config::Config) {
    stats::print_system_info();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(async {
        for app in config.app {
            let grpc_list = match app.grpc {
                Some(l) if !l.is_empty() => l,
                _ => continue,
            };
            for cfg in grpc_list {
                let result = match cfg.mode.as_str() {
                    "ping"          => benchmark_ping(cfg).await,
                    "2_way_stream"  => benchmark_stream(cfg).await,
                    other => {
                        eprintln!("error: unknown gRPC mode '{other}'; skipping");
                        continue;
                    }
                };
                stats::print_result(&result);
            }
        }
    });
}

async fn benchmark_ping(cfg: GrpcConfig) -> BenchResult {
    println!(
        "\n Benchmarking gRPC ping on {} | concurrency={} | duration={}s",
        cfg.url, cfg.max_concurrent, cfg.max_duration
    );

    let s = setup(&cfg);
    let url_str = cfg.url.clone();

    for _ in 0..cfg.max_concurrent as usize {
        let tok = s.token.clone();
        let url = cfg.url.clone();
        let stx = s.tx.clone();

        tokio::spawn(async move {
            let mut client =
                match proto::echo::echo_client::EchoClient::connect(url.clone()).await {
                    Ok(c)  => c,
                    Err(e) => {
                        eprintln!("error: gRPC connect to '{url}' failed: {e}");
                        return;
                    }
                };

            let payload = proto::echo::PingRequest { message: "ping".into() };

            loop {
                if tok.is_cancelled() { break; }

                let t0  = std::time::Instant::now();
                let req = tonic::Request::new(payload.clone());
                let sample = match client.ping(req).await {
                    Ok(_)  => Sample::ok(t0.elapsed().as_nanos() as u64),
                    Err(e) => {
                        eprintln!("warning: gRPC ping error: {e}");
                        Sample::fail()
                    }
                };
                if stx.send_async(sample).await.is_err() { break; }
            }
        });
    }

    collect(s, "grpc-ping", &url_str).await
}

async fn benchmark_stream(cfg: GrpcConfig) -> BenchResult {
    println!(
        "\n Benchmarking gRPC bidirectional stream on {} | concurrency={} | duration={}s",
        cfg.url, cfg.max_concurrent, cfg.max_duration
    );

    let s = setup(&cfg);
    let url_str = cfg.url.clone();

    for _ in 0..cfg.max_concurrent as usize {
        let tok = s.token.clone();
        let url = cfg.url.clone();
        let stx = s.tx.clone();

        tokio::spawn(async move {
            let mut client =
                match proto::stream::stream_client::StreamClient::connect(url.clone()).await {
                    Ok(c)  => c,
                    Err(e) => {
                        eprintln!("error: gRPC connect to '{url}' failed: {e}");
                        return;
                    }
                };

            let payload = proto::stream::StreamRequest { message: "hello".into() };
            let outbound = futures_util::stream::repeat(payload);

            let response = match client.chat(outbound).await {
                Ok(r)  => r,
                Err(e) => {
                    eprintln!("error: gRPC stream setup failed: {e}");
                    return;
                }
            };

            let (_, mut rx_stream, _) = response.into_parts();

            loop {
                if tok.is_cancelled() { break; }

                let t0 = std::time::Instant::now();
                let sample = match rx_stream.message().await {
                    Ok(Some(_)) => Sample::ok(t0.elapsed().as_nanos() as u64),
                    Ok(None)    => break, // server closed stream
                    Err(e)      => {
                        eprintln!("warning: gRPC stream recv error: {e}");
                        Sample::fail()
                    }
                };
                if stx.send_async(sample).await.is_err() { break; }
            }
        });
    }

    collect(s, "grpc-stream", &url_str).await
}

struct BenchSetup {
    token: CancellationToken,
    start: std::time::Instant,
    tx:    flume::Sender<Sample>,
    rx:    flume::Receiver<Sample>,
}

fn setup(cfg: &GrpcConfig) -> BenchSetup {
    let conc  = cfg.max_concurrent as usize;
    let dur   = std::time::Duration::from_secs(cfg.max_duration);
    let start = std::time::Instant::now();
    let token = CancellationToken::new();

    {
        let t        = token.clone();
        let deadline = start + dur;
        tokio::spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            t.cancel();
        });
    }

    let (tx, rx) = flume::bounded::<Sample>(conc * 4);
    BenchSetup { token, start, tx, rx }
}

async fn collect(setup: BenchSetup, label: &str, url: &str) -> BenchResult {
    // Dropping tx here → rx drains and closes once all spawned workers also drop theirs
    drop(setup.tx);

    let mut times_ns:   Vec<u64> = Vec::with_capacity(1 << 14);
    let mut total_sent: u64 = 0;

    while let Ok(s) = setup.rx.recv_async().await {
        total_sent += s.sent;
        if s.is_ok() { times_ns.push(s.latency_ns); }
    }

    times_ns.sort_unstable();

    BenchResult {
        label:      label.into(),
        url:        url.into(),
        duration:   setup.start.elapsed(),
        times_ns,
        total_sent,
    }
}
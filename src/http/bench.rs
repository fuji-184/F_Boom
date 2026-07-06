use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::config::{HttpConfig, Payload};
use crate::process;
use crate::stats::{self, BenchResult};
use super::worker::{self, Ctx, Delete, Get, JsonBody, NoBody, Post, Put, Sample};
use crate::config::App;

pub fn run_http(config: crate::config::Config) {
    stats::print_system_info();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let results = rt.block_on(run_all(config.app));

    for r in &results {
        stats::print_result(r);
    }
}

async fn run_all(apps: Vec<App>) -> Vec<BenchResult> {
    let mut all_results = Vec::new();

    for app in apps {
        // Optionally start the server under test
        let handle = if app.command.is_some() {
            process::spawn(&app)
        } else {
            None
        };

        let http_list = match app.http {
            Some(list) if !list.is_empty() => list,
            _ => {
                if let Some(h) = handle { h.kill().await; }
                continue;
            }
        };

        // Run all HTTP targets for this app concurrently
        let label = app.command
            .as_ref()
            .map(|c| c.first.clone())
            .unwrap_or_default();

        let futs: Vec<_> = http_list
            .into_iter()
            .map(|cfg| benchmark_one(cfg, label.clone()))
            .collect();

        let results = futures_util::future::join_all(futs).await;
        all_results.extend(results.into_iter().flatten());

        if let Some(h) = handle { h.kill().await; }
    }

    all_results
}

async fn benchmark_one(cfg: HttpConfig, label: String) -> Option<BenchResult> {
    println!(
        "\n Benchmarking {} {} on {} | concurrency={} | duration={}s",
        cfg.method.to_uppercase(), cfg.mode.join("+"),
        cfg.url, cfg.max_concurrent, cfg.max_duration
    );

    let conc = cfg.max_concurrent as usize;
    let dur  = std::time::Duration::from_secs(cfg.max_duration);

    let mut builder = reqwest::Client::builder()
        .pool_max_idle_per_host(conc)
        .pool_idle_timeout(dur)
        .tcp_keepalive(dur)
        .timeout(std::time::Duration::from_secs(cfg.timeout))
        .tcp_nodelay(true);

    let has = |s: &str| cfg.mode.iter().any(|m| m == s);

    builder = match (has("http2"), has("http3"), has("http1")) {
        (true, false, false) => builder.http2_prior_knowledge(),
        _ => builder.http1_only(),
    };

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to build HTTP client for {}: {e}", cfg.url);
            return None;
        }
    };

    // Queue depth = 2× workers so producer is never starved
    let queue_depth = conc * 2;
    let (work_tx, work_rx) = flume::bounded::<()>(queue_depth);
    let (sample_tx, sample_rx) = flume::bounded::<Sample>(queue_depth);

    let token = CancellationToken::new();
    let ctx   = Arc::new(Ctx { client, url: cfg.url.clone() });

    let start    = std::time::Instant::now();
    let deadline = start + dur;

    {
        let t = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            t.cancel();
        });
    }

    {
        let t  = token.clone();
        let tx = work_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = t.cancelled() => break,
                    r = tx.send_async(()) => { if r.is_err() { break; } }
                }
            }
            // tx dropped here → workers see channel closed
        });
    }
    drop(work_tx); // only producer owns a sender clone above

    let mut worker_handles = Vec::with_capacity(conc);

    for _ in 0..conc {
        let tok  = token.clone();
        let rx   = work_rx.clone();
        let stx  = sample_tx.clone();
        let ctx  = ctx.clone();
        let meth = cfg.method.clone();
        let pay  = cfg.payload.clone();

        worker_handles.push(tokio::spawn(async move {
            dispatch_worker(tok, rx, ctx, stx, meth, pay).await;
        }));
    }

    drop(work_rx);
    drop(sample_tx); // last sender, when all workers drop theirs → rx closes

    let mut times_ns:  Vec<u64> = Vec::with_capacity(1 << 16);
    let mut total_sent: u64 = 0;

    while let Ok(s) = sample_rx.recv_async().await {
        total_sent += s.sent;
        if s.is_ok() {
            times_ns.push(s.latency_ns);
        }
    }

    for h in worker_handles { let _ = h.await; }

    times_ns.sort_unstable();

    Some(BenchResult {
        label,
        url:        cfg.url,
        duration:   start.elapsed(),
        times_ns,
        total_sent,
    })
}

async fn dispatch_worker(
    tok:  CancellationToken,
    rx:   flume::Receiver<()>,
    ctx:  Arc<Ctx>,
    tx:   flume::Sender<Sample>,
    method: String,
    payload: Option<Payload>,
) {
    match method.as_str() {
        "get" => worker::run::<Get, NoBody>(tok, rx, ctx, tx, NoBody).await,
        "delete" => worker::run::<Delete, NoBody>(tok, rx, ctx, tx, NoBody).await,
        "post" => {
            let body = json_body(payload);
            worker::run::<Post, JsonBody>(tok, rx, ctx, tx, body).await;
        }
        "put" => {
            let body = json_body(payload);
            worker::run::<Put, JsonBody>(tok, rx, ctx, tx, body).await;
        }
        unknown => {
            eprintln!("error: unsupported HTTP method '{unknown}'; skipping worker");
        }
    }
}

fn json_body(payload: Option<Payload>) -> JsonBody {
    let json = payload
        .map(|p| p.val)
        .unwrap_or_else(|| "{}".to_owned());
    JsonBody::new(json)
}
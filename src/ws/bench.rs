use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::config::WsConfig;
use crate::stats::{self, BenchResult};

#[derive(Copy, Clone)]
struct Sample {
    latency_ns: u64, // 0 = failure
    sent: u64,
}

impl Sample {
    fn ok(ns: u64) -> Self { Self { latency_ns: ns, sent: 1 } }
    fn fail()      -> Self { Self { latency_ns: 0,  sent: 1 } }
    fn is_ok(self) -> bool { self.latency_ns > 0 }
}

trait WsPayload: Send + 'static {
    fn message(val: &str) -> Message;
}

struct Ping;
struct Text;

impl WsPayload for Ping {
    fn message(val: &str) -> Message { Message::Ping(val.as_bytes().to_vec().into()) }
}
impl WsPayload for Text {
    fn message(val: &str) -> Message { Message::Text(val.to_owned().into()) }
}

pub fn run_ws(config: crate::config::Config) {
    stats::print_system_info();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(async {
        for app in config.app {
            let ws_list = match app.ws {
                Some(l) if !l.is_empty() => l,
                _ => continue,
            };
            for cfg in ws_list {
                let result = match cfg.payload.kind.as_str() {
                    "ping" => benchmark::<Ping>(cfg).await,
                    "text" => benchmark::<Text>(cfg).await,
                    other  => {
                        eprintln!("error: unknown WebSocket payload type '{other}'; skipping");
                        continue;
                    }
                };
                stats::print_result(&result);
            }
        }
    });
}

async fn benchmark<P: WsPayload>(cfg: WsConfig) -> BenchResult {
    println!(
        "\n Benchmarking WebSocket on {} | concurrency={} | duration={}s",
        cfg.url, cfg.max_concurrent, cfg.max_duration
    );

    let token    = CancellationToken::new();
    let dur      = std::time::Duration::from_secs(cfg.max_duration);
    let start    = std::time::Instant::now();
    let deadline = start + dur;

    // Canceller task
    {
        let t = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            t.cancel();
        });
    }

    // Pre build the message once, clone is O(1) for most message types
    let msg = P::message(&cfg.payload.val);

    let conc = cfg.max_concurrent as usize;
    let (tx, rx) = flume::bounded::<Sample>(conc * 4);

    for _ in 0..conc {
        let tok = token.clone();
        let url = cfg.url.clone();
        let msg = msg.clone();
        let stx = tx.clone();

        tokio::spawn(async move {
            let ws_stream = match tokio_tungstenite::connect_async(&url).await {
                Ok((s, _)) => s,
                Err(e) => {
                    eprintln!("error: WebSocket connect to '{url}' failed: {e}");
                    return;
                }
            };

            let (mut writer, mut reader) = ws_stream.split();

            loop {
                if tok.is_cancelled() { break; }

                let t0 = std::time::Instant::now();

                if let Err(e) = writer.send(msg.clone()).await {
                    eprintln!("warning: WebSocket send error: {e}");
                    break;
                }

                let sample = match reader.next().await {
                    Some(Ok(Message::Pong(_))) => Sample::ok(t0.elapsed().as_nanos() as u64),
                    Some(Ok(Message::Text(_))) => Sample::ok(t0.elapsed().as_nanos() as u64),
                    Some(Ok(_))   => Sample::fail(),
                    Some(Err(e))  => {
                        eprintln!("warning: WebSocket read error: {e}");
                        break;
                    }
                    None => break,
                };

                if stx.send_async(sample).await.is_err() { break; }
            }
        });
    }

    drop(tx); // main sender, workers drop theirs on exit → rx closes

    let mut times_ns:  Vec<u64> = Vec::with_capacity(1 << 14);
    let mut total_sent: u64 = 0;

    while let Ok(s) = rx.recv_async().await {
        total_sent += s.sent;
        if s.is_ok() { times_ns.push(s.latency_ns); }
    }

    times_ns.sort_unstable();

    BenchResult {
        label:      "ws".into(),
        url:        cfg.url,
        duration:   start.elapsed(),
        times_ns,
        total_sent,
    }
}
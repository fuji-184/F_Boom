use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::config::WsConfig;
use crate::rt;
use crate::stats::{self, BenchResult};

// Trait lokal: tanpa bound Send/Sync — semua task via spawn_local.
trait WsPayload {
    // Bangun owned Message per pengiriman dari potongan 'static bersama.
    // tungstenite 0.24 memiliki payload owned (Vec<u8>/String), jadi tiap
    // send wajib materialisasi 1 nilai kecil dari slice statis ini.
    // Tanpa clone dari value lain, tanpa Arc/refcount.
    fn build(s: &'static str) -> Message;
}

struct Ping;
struct Text;

impl WsPayload for Ping {
    fn build(s: &'static str) -> Message { Message::Ping(s.as_bytes().to_vec()) }
}
impl WsPayload for Text {
    fn build(s: &'static str) -> Message { Message::Text(String::from(s)) }
}

pub fn run_ws(config: crate::config::Config) {
    stats::print_system_info();

    // Share-nothing: tanpa runtime global. Tiap target di-shard ke
    // beberapa OS thread, masing-masing dengan `current_thread` runtime.
    for app in config.app {
        let n_threads = rt::resolve(app.num_thread);
        let ws_list = match app.ws {
            Some(l) if !l.is_empty() => l,
            _ => continue,
        };
        for cfg in ws_list {
            let result = match cfg.payload.kind.as_str() {
                "ping" => benchmark::<Ping>(cfg, n_threads),
                "text" => benchmark::<Text>(cfg, n_threads),
                other  => {
                    eprintln!("error: unknown WebSocket payload type '{other}'; skipping");
                    continue;
                }
            };
            stats::print_result(&result);
        }
    }
}

fn benchmark<P: WsPayload + 'static>(cfg: WsConfig, n_threads: usize) -> BenchResult {
    let n_threads = n_threads.max(1);
    println!(
        "\n Benchmarking WebSocket on {} | concurrency={} | duration={}s (share-nothing, {} threads)",
        cfg.url, cfg.max_concurrent, cfg.max_duration, n_threads,
    );

    let conc = cfg.max_concurrent as usize;
    if conc == 0 {
        return BenchResult {
            label: "ws".into(),
            url: cfg.url,
            duration: std::time::Duration::ZERO,
            times_ns: Vec::new(),
            total_sent: 0,
        };
    }

    let dur = std::time::Duration::from_secs(cfg.max_duration);
    let shards = rt::shard(conc, n_threads);

    let start = std::time::Instant::now();
    let deadline = start + dur;

    // SAFETY: `cfg` dimiliki frame ini dan dijamin hidup sampai seluruh
    // shard join (thread::scope), yaitu sebelum `cfg.url` dipindah ke
    // hasil. Tidak ada mutasi selama itu. Referensi yang diperpanjang
    // hanya dibaca (url + payload), tanpa write bersama.
    let url: &'static str = unsafe { &*(cfg.url.as_str() as *const str) };
    let payload: &'static str = unsafe { &*(cfg.payload.val.as_str() as *const str) };
    // &'static str adalah Copy: dibagikan ke tiap thread/task sebagai
    // salinan 16 byte — tanpa clone heap, tanpa Arc, tanpa Box/drop.

    let (mut times_ns, total_sent) = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(shards.len());

        for shard_conc in shards {
            if shard_conc == 0 {
                continue;
            }
            handles.push(s.spawn(move || -> (Vec<u64>, u64) {
                let rt = rt::build_current_thread_rt();
                rt.block_on(shard_main::<P>(url, payload, shard_conc, deadline))
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
                Err(_) => eprintln!("warning: ws shard thread panicked; partial results kept"),
            }
        }
        (times_ns, total_sent)
    });

    times_ns.sort_unstable();

    BenchResult {
        label:      "ws".into(),
        url:        cfg.url,
        duration:   start.elapsed(),
        times_ns,
        total_sent,
    }
}

/// Satu shard: berjalan penuh di dalam `current_thread` runtime milik
/// OS thread-nya via `LocalSet` + `spawn_local`. Semua koneksi & task
/// terikat di thread ini, tidak pernah migrasi. Tanpa clone value.
async fn shard_main<P: WsPayload + 'static>(
    url: &'static str,
    payload: &'static str,
    shard_conc: usize,
    deadline: std::time::Instant,
) -> (Vec<u64>, u64) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let mut handles = Vec::with_capacity(shard_conc);
            for _ in 0..shard_conc {
                handles.push(tokio::task::spawn_local(ws_worker::<P>(url, payload, deadline)));
            }

            let mut latencies = Vec::with_capacity(1024 * shard_conc.max(1));
            let mut sent: u64 = 0;
            for h in handles {
                match h.await {
                    Ok((mut lat, s)) => {
                        sent += s;
                        latencies.append(&mut lat);
                    }
                    Err(_) => eprintln!("warning: ws worker task panicked"),
                }
            }
            (latencies, sent)
        })
        .await
}

/// Satu koneksi WS yang self-drive sampai deadline. Buffer lokal,
/// tanpa channel global, tanpa clone dari value lain.
async fn ws_worker<P: WsPayload + 'static>(
    url: &'static str,
    payload: &'static str,
    deadline: std::time::Instant,
) -> (Vec<u64>, u64) {
    let ws_stream = match tokio_tungstenite::connect_async(url).await {
        Ok((s, _)) => s,
        Err(e) => {
            eprintln!("error: WebSocket connect to '{url}' failed: {e}");
            return (Vec::new(), 0);
        }
    };

    let (mut writer, mut reader) = ws_stream.split();

    let mut latencies = Vec::with_capacity(1024);
    let mut sent: u64 = 0;

    while std::time::Instant::now() < deadline {
        let t0 = std::time::Instant::now();

        if let Err(e) = writer.send(P::build(payload)).await {
            eprintln!("warning: WebSocket send error: {e}");
            break;
        }

        match reader.next().await {
            Some(Ok(Message::Pong(_))) | Some(Ok(Message::Text(_))) => {
                sent += 1;
                latencies.push(t0.elapsed().as_nanos() as u64);
            }
            Some(Ok(_)) => {
                sent += 1; // dihitung terkirim tapi gagal (latency 0)
            }
            Some(Err(e)) => {
                eprintln!("warning: WebSocket read error: {e}");
                break;
            }
            None => break,
        }
    }

    (latencies, sent)
}

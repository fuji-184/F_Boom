use std::clone;

use clap::builder::Str;
use futures_util::{SinkExt, StreamExt};
use prost::Message;
use serde::de::Expected;

struct WsHasil {
    time: Option<tokio::time::Duration>,
    total_send: u64,
}

pub fn run_ws(config: crate::config_reader::Config) {
    crate::features::system_info();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        let mut handles = vec![];

        //  let payload = Box::leak(Box::new("tes".to_string());

        for app in config.app.unwrap().into_iter() {
            for val in app.ws.unwrap().into_iter() {
                match val.payload.r#type.as_str() {
                    "ping" => {
                        let handle = tokio::task::spawn(async move {
                            ws::<Ping>(val).await;
                        });
                        handles.push(handle);
                    }

                    "text" => {
                        let handle = tokio::task::spawn(async move {
                            ws::<Text>(val).await;
                        });
                        handles.push(handle);
                    }
                    _ => return,
                }
            }
        }

        for val in handles.into_iter() {
            val.await.unwrap();
        }
    });
}

trait Payload {
    fn message(input: String) -> tokio_tungstenite::tungstenite::Message;
}

struct Ping(String);
impl Payload for Ping {
    fn message(input: String) -> tokio_tungstenite::tungstenite::Message {
        tokio_tungstenite::tungstenite::Message::Ping(input.into())
    }
}

struct Text();
impl Payload for Text {
    fn message(input: String) -> tokio_tungstenite::tungstenite::Message {
        tokio_tungstenite::tungstenite::Message::Text(input.into())
    }
}

async fn ws<P: Payload + Send + 'static>(ws_config: crate::config_reader::Ws) {
    println!(
        "Benchmarking WebSocket on {} with {} max conns for {} seconds",
        ws_config.url, ws_config.max_concurrent, ws_config.max_duration
    );

    let token = tokio_util::sync::CancellationToken::new();

    let (s_result, r_result) = flume::unbounded::<WsHasil>();

    let start = tokio::time::Instant::now();

    let deadline = start + tokio::time::Duration::from_secs(ws_config.max_duration as u64);
    let sleep_token = token.clone();
    tokio::task::spawn(async move {
        tokio::time::sleep_until(deadline).await;
        sleep_token.cancel();
    });

    let msg = P::message(ws_config.payload.val);

    for _ in 0..ws_config.max_concurrent {
        let conn_token = token.clone();
        let s_result_ref = s_result.clone();
        let url = ws_config.url.clone();
        let msg = msg.clone();

        tokio::task::spawn(async move {
            let (ws_stream, _) = tokio_tungstenite::connect_async(url)
                .await
                .expect("failed to connect to ws server");
            let (mut w, mut r) = ws_stream.split();

            loop {
                if conn_token.is_cancelled() {
                    // drop(s_result_ref);
                    break;
                }

                let ws_start = tokio::time::Instant::now();

                w.send(msg.clone())
                    .await
                    .expect("failed to send data to ws server");

                if let Some(val) = r.next().await {
                    let data = match val {
                        Ok(tokio_tungstenite::tungstenite::Message::Pong(_)) => {
                            let time = ws_start.elapsed();
                            WsHasil {
                                time: Some(time),
                                total_send: 1,
                            }
                        }
                        _ => WsHasil {
                            time: None,
                            total_send: 1,
                        },
                    };

                    let _ = s_result_ref.send_async(data).await;
                }
            }
        });
    }

    let mut times = vec![];
    let mut total_send = 0;

    drop(s_result);

    while let Ok(val) = r_result.recv_async().await {
        if let Some(val) = val.time {
            times.push(val);
        }
        total_send += val.total_send;
    }

    let time = start.elapsed();

    let hasil = crate::http::Hasil {
        times: times,
        total_send: total_send,
        duration: time,
        command: String::from("tes"),
        url: String::from(ws_config.url),
    };
    crate::features::stats(hasil);

    //    token.cancelled().await;
}

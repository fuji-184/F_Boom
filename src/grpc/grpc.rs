use crate::grpc::grpc::bench::PingRequest;
use crate::grpc::grpc::bench::StreamRequest;

pub mod bench {
    tonic::include_proto!("bench");
    tonic::include_proto!("stream");
}

struct GrpcHasil {
    time: Option<tokio::time::Duration>,
    total_send: u64,
}

async fn grpc_ping_pong(
    conn_token: tokio_util::sync::CancellationToken,
    s_hasil_ref: flume::Sender<GrpcHasil>,
    url: String,
) {
    let mut client = bench::echo_client::EchoClient::connect(url).await.unwrap();

    let payload = PingRequest {
        message: "hello".into(),
    };

    loop {
        if conn_token.is_cancelled() {
            break;
        }

        let start_time = tokio::time::Instant::now();

        let req = tonic::Request::new(payload.clone());

        let data = match client.ping(req).await {
            Ok(_) => GrpcHasil {
                time: Some(start_time.elapsed()),
                total_send: 1,
            },
            Err(_) => GrpcHasil {
                time: None,
                total_send: 1,
            },
        };

        let _ = s_hasil_ref.send_async(data).await;
    }
}

async fn grpc_bidirectional_stream(
    conn_token: tokio_util::sync::CancellationToken,
    s_hasil_ref: flume::Sender<GrpcHasil>,
    url: String,
) {
    let mut client = bench::stream_client::StreamClient::connect(url)
        .await
        .unwrap();

    let payload = StreamRequest {
        message: "hello".into(),
    };

    let outbound = futures::stream::repeat(payload.clone());

    let response = client.chat(outbound).await.unwrap();
    let (_metadata, mut rx, _extensions) = response.into_parts();

    loop {
        if conn_token.is_cancelled() {
            break;
        }

        let start_time = tokio::time::Instant::now();

        let data = match rx.message().await {
            Ok(Some(_)) => GrpcHasil {
                time: Some(start_time.elapsed()),
                total_send: 1,
            },
            Ok(None) | Err(_) => GrpcHasil {
                time: None,
                total_send: 1,
            },
        };

        let _ = s_hasil_ref.send_async(data).await;
    }
}

async fn grpc(grpc_config: crate::config_reader::Grpc) {
    let mode_info = grpc_config.mode.replace("_", " ");
    println!(
        " Benchmarking GRPC {} on {} with {} max conns for {} seconds",
        mode_info, grpc_config.url, grpc_config.max_concurrent, grpc_config.max_duration
    );

    let (s_hasil, r_hasil) = flume::unbounded::<GrpcHasil>();
    let token = tokio_util::sync::CancellationToken::new();

    let start = tokio::time::Instant::now();
    let deadline = start + tokio::time::Duration::from_secs(grpc_config.max_duration as u64);

    let sleep_token = token.clone();
    tokio::task::spawn(async move {
        tokio::time::sleep_until(deadline).await;
        sleep_token.cancel();
    });

    for _ in 0..grpc_config.max_concurrent {
        let conn_token = token.clone();
        let s_hasil_ref = s_hasil.clone();
        let url = grpc_config.url.clone();
        let mode = grpc_config.mode.clone();
        tokio::task::spawn(async move {
            match mode.as_str() {
                "ping" => grpc_ping_pong(conn_token, s_hasil_ref, url).await,
                "2_way_stream" => grpc_bidirectional_stream(conn_token, s_hasil_ref, url).await,
                _ => return,
            }
        });
    }

    drop(s_hasil);

    let mut times = vec![];
    let mut total_send = 0;

    while let Ok(val) = r_hasil.recv_async().await {
        if let Some(val) = val.time {
            times.push(val);
        }
        total_send += val.total_send;
    }

    let hasil = crate::http::Hasil {
        duration: start.elapsed(),
        times: times,
        total_send: total_send,
        url: String::from("url"),
        command: String::from("tes"),
    };

    crate::features::stats(hasil);
}

pub fn run_grpc(config: crate::config_reader::Config) {
    crate::features::system_info();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        for app in config.app.unwrap().into_iter() {
            for val in app.grpc.unwrap().into_iter() {
                grpc(val).await;
            }
        }
    });
}

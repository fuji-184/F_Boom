use reqwest::RequestBuilder;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::{cli, config_reader::Payload};

fn print_info_http(http_config: &crate::config_reader::Http) {
    println!(
        " Benchmarking on url {} with max concurrent {} for {} seconds\n\n",
        http_config.url, http_config.max_concurrent, http_config.max_duration
    );
}

fn parse_url(url: &str) -> Option<(String, String)> {
    let (default_port, url) = if let Some(rest) = url.strip_prefix("http://") {
        (80, rest)
    } else if let Some(rest) = url.strip_prefix("https://") {
        (443, rest)
    } else {
        (80, url)
    };

    let mut parts = url.splitn(2, '/');
    let host_port = parts.next().unwrap_or("");
    let path = format!("/{}", parts.next().unwrap_or(""));

    let mut hp = host_port.splitn(2, ':');
    let host = hp.next().unwrap_or("").to_string();
    let port = hp
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(default_port);

    let host_with_port = if port == default_port {
        host
    } else {
        format!("{}:{}", host, port)
    };

    Some((host_with_port, path))
}

struct Ctx {
    client: reqwest::Client,
    url: String,
}

pub async fn http(
    start: tokio::time::Instant,
    s: tokio::sync::mpsc::UnboundedSender<crate::Data>,
    http_config: crate::config_reader::Http,
) {
    for _ in 0..5 {
        if tokio::net::TcpStream::connect("127.0.0.1:8080")
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }

    print_info_http(&http_config);

    let mut client = reqwest::Client::builder()
        .pool_max_idle_per_host(http_config.max_concurrent as usize)
        .pool_idle_timeout(tokio::time::Duration::from_secs(
            http_config.max_duration as u64,
        ))
        .tcp_keepalive(tokio::time::Duration::from_secs(
            http_config.max_duration as u64,
        ))
        .timeout(tokio::time::Duration::from_secs(http_config.timeout))
        .tcp_nodelay(true);

    client = if http_config.mode.contains(&String::from("http1"))
        && http_config.mode.contains(&String::from("http2"))
        && http_config.mode.contains(&String::from("http3"))
    {
        client
    } else if http_config.mode.contains(&String::from("http1"))
        && http_config.mode.contains(&String::from("http2"))
    {
        client
    } else if http_config.mode.contains(&String::from("http1"))
        && http_config.mode.contains(&String::from("http3"))
    {
        client
    } else if http_config.mode.contains(&String::from("http2"))
        && http_config.mode.contains(&String::from("http3"))
    {
        client
    } else if http_config.mode.contains(&String::from("http2")) {
        client.http2_prior_knowledge()
    } else if http_config.mode.contains(&String::from("http3")) {
        let mut client = client.http3_prior_knowledge();
        client =
            client.http3_max_idle_timeout(tokio::time::Duration::from_secs(http_config.timeout));
        client = client.http3_send_grease(false);
        client
    } else {
        client.http1_only()
    };

    let client = client.build().unwrap();

    let cancel_token = CancellationToken::new();
    let cancel_child = cancel_token.clone();

    let deadline = tokio::time::Duration::from_secs(http_config.max_duration as u64);
    /*
        let time = client.tes().await;
        let num = if time > 200 {
            num_cpus::get()
        } else {
            num_cpus::get()
        };
    */
    let num = num_cpus::get() * 6;
    let (work_tx, work_rx) = flume::bounded::<()>(num);
    let (results_tx, results_rx) = flume::bounded::<crate::Data>(num);

    let shared = Arc::new(Ctx {
        client,
        url: http_config.url,
    });

    let start_for_cancel = start.clone();
    let deadline_for_cancel = start_for_cancel + deadline;

    tokio::spawn(async move {
        tokio::time::sleep_until(deadline_for_cancel).await;
        cancel_child.cancel();
    });

    let producer_token = cancel_token.clone();
    let producer_work_tx = work_tx.clone();
    tokio::spawn(async move {
        loop {
            if producer_token.is_cancelled() {
                break;
            }

            if start.elapsed() > deadline {
                break;
            }
            tokio::select! {
                biased;

                _ = producer_token.cancelled() => {
                    break;
                }

                res = producer_work_tx.send_async(()) => {
                    if res.is_err() {
                        break;
                    }
                }
            }
        }
        drop(producer_work_tx);
    });

    let mut worker_handles = Vec::with_capacity(num);

    for _ in 0..num {
        let worker_shared = shared.clone();
        let work_rx_clone = work_rx.clone();
        let results_tx_clone = results_tx.clone();
        let worker_token = cancel_token.clone();
        let start_clone = start.clone();
        let deadline_clone = deadline;
        let method = http_config.method.clone();
        let payload = http_config.payload.clone();

        let handle = tokio::spawn(async move {
            match method.as_str() {
                "get" => {
                    worker::<Get, NoBody>(
                        worker_token,
                        work_rx_clone,
                        worker_shared,
                        start_clone,
                        deadline_clone,
                        results_tx_clone,
                        NoBody,
                    )
                    .await
                }
                "post" => {
                    if let Some(val) = payload {
                        if val.r#type == "json" {
                            worker::<Post, Json>(
                                worker_token,
                                work_rx_clone,
                                worker_shared,
                                start_clone,
                                deadline_clone,
                                results_tx_clone,
                                Json(val.val),
                            )
                            .await;
                        }
                    }
                }
                "put" => {
                    if let Some(val) = payload {
                        if val.r#type == "json" {
                            worker::<Put, Json>(
                                worker_token,
                                work_rx_clone,
                                worker_shared,
                                start_clone,
                                deadline_clone,
                                results_tx_clone,
                                Json(val.val),
                            )
                            .await;
                        }
                    }
                }
                "delete" => {
                    worker::<Delete, NoBody>(
                        worker_token,
                        work_rx_clone,
                        worker_shared,
                        start_clone,
                        deadline_clone,
                        results_tx_clone,
                        NoBody,
                    )
                    .await
                }
                _ => return,
            }
        });
        worker_handles.push(handle);
    }

    drop(work_rx);
    drop(results_tx);

    let aggregator_handle = tokio::spawn(async move {
        loop {
            match results_rx.recv_async().await {
                Ok(data) => {
                    if s.send(data).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    break;
                }
            }
        }
    });

    for h in worker_handles {
        let _ = h.await;
    }

    aggregator_handle.await.unwrap();
}

trait HttpMethod {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder;
}

struct Get;
struct Post;
struct Put;
struct Delete;

impl HttpMethod for Get {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        client.get(url)
    }
}
impl HttpMethod for Post {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        client.post(url)
    }
}
impl HttpMethod for Put {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        client.put(url)
    }
}
impl HttpMethod for Delete {
    fn build(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
        client.delete(url)
    }
}

trait HttpBody {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder;
}

struct NoBody;
struct Json(String);

impl HttpBody for NoBody {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req
    }
}

impl HttpBody for Json {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = req.header("content-type", "application/json");
        req.body(self.0.clone())
    }
}

async fn worker<M: HttpMethod, B: HttpBody>(
    worker_token: tokio_util::sync::CancellationToken,
    work_rx_clone: flume::Receiver<()>,
    worker_shared: std::sync::Arc<Ctx>,
    start_clone: tokio::time::Instant,
    deadline_clone: tokio::time::Duration,
    results_tx_clone: flume::Sender<crate::Data>,
    body: B,
) {
    loop {
        tokio::select! {
            _ = worker_token.cancelled() => {
                break;
            }

            maybe_job = work_rx_clone.recv_async() => {
                match maybe_job {
                    Ok(()) => {
                        let request_start = tokio::time::Instant::now();
                        let mut req = M::build(&worker_shared.client, &worker_shared.url);
                        req = body.apply(req);
                        let result = req.send().await;

                       let data =  match result {
                          Ok(res) =>  {
                                   if res.status().is_success() {
                        crate::Data { time: Some(request_start.elapsed()), total_send: 1 }
                            } else {
                         crate::Data { time: None, total_send: 1 }

                                    }},

                        Err(_) =>    crate::Data { time: None, total_send: 1 }
                        };

                        if results_tx_clone.send_async(data).await.is_err() {
                            break;
                        }
                        if start_clone.elapsed() > deadline_clone {
                            break;
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
        }
    }
}

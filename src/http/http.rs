use std::u64;

struct HttpContext {
    s: crossbeam_channel::Sender<crate::Data>,
    url: String,
    semaphore: tokio::sync::Semaphore,
    client: reqwest::Client,
}

fn print_info_http(http_config: &crate::config_reader::Http) {
    println!(
        "Benchmarking on url {} with max concurrent {} for {} seconds\n\n",
        http_config.url, http_config.max_concurrent, http_config.max_duration
    );
}

pub async fn http(
    start: tokio::time::Instant,
    s: crossbeam_channel::Sender<crate::Data>,
    http_config: crate::config_reader::Http,
) {
    print_info_http(&http_config);

    let semaphore = tokio::sync::Semaphore::new(http_config.max_concurrent as usize);
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(http_config.max_concurrent as usize)
        .pool_idle_timeout(tokio::time::Duration::from_secs(40))
        .timeout(tokio::time::Duration::from_secs(http_config.timeout))
        .build()
        .unwrap();

    let mut join_set = tokio::task::JoinSet::new();

    let http_context = std::sync::Arc::new(HttpContext {
        s,
        semaphore,
        url: http_config.url,
        client,
    });

    let http_context_ref = http_context.clone();

    // let url: &'static String = Box::leak(Box::new(http_config.url));

    while start.elapsed().as_secs() <= http_config.max_duration as u64 {
        for _ in 0..http_config.batch_size {
            if start.elapsed().as_secs() >= http_config.max_duration as u64 {
                break;
            }

            let http_context_ref = http_context_ref.clone();

            join_set.spawn(async move {
                let _permit = http_context_ref.semaphore.acquire().await.unwrap();

                let request_start = tokio::time::Instant::now();

                match http_context_ref
                    .client
                    .get(&http_context_ref.url)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        let data = crate::Data {
                            time: Some(request_start.elapsed()),
                            total_send: None,
                        };
                        http_context_ref.s.send(data).unwrap();
                    }
                    _ => {}
                }
                let data = crate::Data {
                    time: None,
                    total_send: Some(1),
                };
                http_context_ref.s.send(data).unwrap();
            });
        }

        while join_set.len() > http_config.max_concurrent as usize {
            if let Some(_) = join_set.try_join_next() {
                continue;
            }
            tokio::task::yield_now().await;
        }
    }

    while let Some(_) = join_set.join_next().await {}
}

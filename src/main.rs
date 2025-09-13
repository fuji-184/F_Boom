use std::sync::{Arc};
use std::time::Instant;
use tokio::sync::Semaphore;
use tokio::time::Duration;
use tokio::task::JoinSet;
use crossbeam_channel::unbounded;
use tokio::runtime::Runtime;
use sysinfo::System;

mod stats;
mod features;
mod run_app;


struct Data {
    time: Option<Duration>,
    total_send: Option<u64>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {

    let (s, r) = unbounded::<Data>();

    let start = Instant::now();

    let runtime = Runtime::new().unwrap();
    runtime.block_on(async {

    let child = run_app::run_app().unwrap();

        let url = "http://127.0.0.1:8080/4";

        let semaphore = Arc::new(Semaphore::new(200));
        let client = Arc::new(
            reqwest::Client::builder()
                .pool_max_idle_per_host(200)
                .pool_idle_timeout(Duration::from_secs(40))
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        );

        let mut join_set = JoinSet::new();
        let batch_size = 100;
        let max_concurrent = 200;

        let s_ref = s.clone();
        
        features::system_info();
        
        let mut s = System::new_all();
        //s.refresh_cpu_all();
        s.refresh_processes_specifics(
          sysinfo::ProcessesToUpdate::All,
          true,
          sysinfo::ProcessRefreshKind::nothing().with_cpu()
        );

        while start.elapsed().as_secs() <= 30 {
            for _ in 0..batch_size {
                if start.elapsed().as_secs() >= 30 {
                    break;
                }

                let client_ref = client.clone();
                let sem_ref = semaphore.clone();
                let s_ref = s_ref.clone();

                join_set.spawn(async move {
                    let _permit = sem_ref.acquire().await.unwrap();

                    let request_start = Instant::now();

                    match client_ref.get(url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            let data = Data {
                                time: Some(request_start.elapsed()),
                                total_send: None,
                            };
                            s_ref.send(data).unwrap();
                        }
                        _ => {}
                    }
                    let data = Data {
                        time: None,
                        total_send: Some(1),
                    };
                    s_ref.send(data).unwrap();
                });
            }

            while join_set.len() > max_concurrent {
                if let Some(_) = join_set.try_join_next() {
                    continue;
                }
                tokio::task::yield_now().await;
            }
        }

        while let Some(_) = join_set.join_next().await {}
        
        stats::cpu_usage(&mut s, &child);


        
    });

    let duration = start.elapsed();

    drop(s);

    let mut times = vec![];
    let mut total_send = 0;

    for val in r.iter() {
        if let Some(val) = val.time {
            times.push(val.as_nanos() as u64);
        } else if let Some(val) = val.total_send {
            total_send += val;
        }
    }

    features::stats(duration, total_send, times);
    
    
    Ok(())
}

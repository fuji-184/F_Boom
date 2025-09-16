struct Context {
    app: tokio::process::Child,
    perf: Option<tokio::process::Child>,
}

pub struct Hasil {
    pub duration: tokio::time::Duration,
    pub times: Vec<u64>,
    pub total_send: u64,
    pub command: String,
    pub url: String,
}

pub fn http_benchmark() {
    crate::features::system_info();

    let (s_hasil, r_hasil) = crossbeam_channel::unbounded::<Hasil>();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let config = crate::config_reader::read_config("./config.toml").await;
        let app_list = config.app.unwrap();

        let mut handles = vec![];
        let mut app_handles: Vec<Context> = vec![];

        for app in app_list.into_iter() {
            let (child, perf) = crate::run_app::run_app(&app).unwrap();

            let http_list = app.http.unwrap();
            let command = app.command.unwrap().first;

            for http in http_list.into_iter() {
                let url = http.url.clone();
                let command = command.clone();
                let s_hasil = s_hasil.clone();
                let handle = tokio::spawn(async move {
                    let (s, r) = crossbeam_channel::unbounded::<crate::Data>();

                    let start = tokio::time::Instant::now();

                    crate::http::http(start, s, http).await;

                    let duration = start.elapsed();

                    let mut times = vec![];
                    let mut total_send = 0;

                    for val in r.iter() {
                        if let Some(val) = val.time {
                            times.push(val.as_nanos() as u64);
                        } else if let Some(val) = val.total_send {
                            total_send += val;
                        }
                    }

                    let hasil = Hasil {
                        duration,
                        times,
                        total_send,
                        command: command,
                        url: url,
                    };
                    s_hasil.send(hasil).unwrap();
                });

                handles.push(handle);
            }

            let ctx = Context {
                app: child,
                perf: perf,
            };
            app_handles.push(ctx);
        }

        for val in handles {
            let _ = val.await;
        }

        for val in app_handles.iter_mut() {
            let pid = val.app.id().unwrap();
            crate::features::memory_usage(pid).await;
            val.app.kill().await.unwrap();
            if let Some(perf) = &mut val.perf {
                perf.wait().await.unwrap();
            }
        }
    });

    drop(s_hasil);

    for val in r_hasil.iter() {
        crate::features::stats(val);
    }
}

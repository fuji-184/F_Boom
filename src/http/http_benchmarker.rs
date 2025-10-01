use std::process::{self, Child};

struct Context {
    app: tokio::process::Child,
    perf: Option<tokio::process::Child>,
}

#[derive(Debug)]
pub struct Hasil {
    pub duration: tokio::time::Duration,
    pub times: Vec<tokio::time::Duration>,
    pub total_send: u64,
    pub command: String,
    pub url: String,
}

pub fn http_benchmark(config: crate::config_reader::Config) {
    crate::features::system_info();

    let mut hasil: Vec<Hasil> = vec![];

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let app_list = config.app.unwrap();

        let mut handles = vec![];
        let mut app_handles: Vec<Context> = vec![];

        for app in app_list.into_iter() {
            let mut process: Option<(tokio::process::Child, Option<tokio::process::Child>)> = None;

            let mut command = String::from("");

            if let Some(ref val) = app.command {
                process = Some(crate::run_app::run_app(&app).unwrap());
                command = val.first.clone();
            }

            let http_list = app.http.unwrap();

            for http in http_list.into_iter() {
                let url = http.url.clone();
                let command = command.clone();
                let handle = tokio::spawn(async move {
                    let (s, mut r) = tokio::sync::mpsc::unbounded_channel::<crate::Data>();

                    let start = tokio::time::Instant::now();

                    crate::http::http(start, s, http).await;

                    let duration = start.elapsed();

                    let mut times = vec![];
                    let mut total_send = 0;

                    while let Some(val) = r.recv().await {
                        if let Some(val) = val.time {
                            times.push(val);
                        }
                        total_send += val.total_send;
                    }

                    let hasil = Hasil {
                        duration,
                        times,
                        total_send,
                        command: command,
                        url: url,
                    };
                    hasil
                });

                handles.push(handle);
            }

            if let Some(val) = process {
                let (child, perf) = val;
                let ctx = Context {
                    app: child,
                    perf: perf,
                };
                app_handles.push(ctx);
            }
        }

        for val in handles {
            let isi = val.await.unwrap();
            hasil.push(isi);
        }

        for val in app_handles.iter_mut() {
            val.app.kill().await.unwrap();
            if let Some(perf) = &mut val.perf {
                perf.wait().await.unwrap();
            }
        }
    });

    for val in hasil.into_iter() {
        crate::features::stats(val);
    }
}

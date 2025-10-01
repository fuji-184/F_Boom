use std::usize;

pub fn run_cli_benchmark(config: crate::config_reader::Config) {
    let app = config.app.unwrap();

    for val in app.into_iter() {
        let cli = val.cli.unwrap();
        let mut times = Vec::with_capacity(cli.max_run as usize);
        let cmd = val.command.unwrap().first;
        println!("benchmarking {}", cmd);
        for _ in 0..cli.max_run {
            let start = std::time::Instant::now();
            std::process::Command::new(&cmd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            let time = start.elapsed();
            times.push(time);
        }

        let times: Vec<usize> = times.iter().map(|val| val.as_nanos() as usize).collect();

        let min = times.iter().min().unwrap();
        let max = times.iter().max().unwrap();
        let avg = times.iter().sum::<usize>() / times.len();
        println!("min: {}, max: {}, avg: {}", min, max, avg);
    }
}

use core::f64;
use std::{process::Stdio, usize};

pub fn run_cli_benchmark(config: crate::config_reader::Config) {
    crate::features::system_info();

    let app = config.app.unwrap();

    for val in app.into_iter() {
        let info = val.command.clone().unwrap().first;
        let cli = val.cli.unwrap();
        let cmd = val.command.unwrap().first;
        println!("\n\n benchmarking {}", cmd);

        let _ = std::process::Command::new(&cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let perf = perf(&info);

        let times = if !cli.tick.is_none() {
            let mut times = Vec::with_capacity(cli.max_run as usize);
            for _ in 0..cli.max_run {
                let mut t = cpu_timer::DeltaTimer::<true>::default();

                t.start();
                std::process::Command::new(&cmd)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap();
                t.stop();

                times.push(t.value());
            }
            Some(
                times
                    .iter()
                    .map(|val| *val as usize)
                    .collect::<Vec<usize>>(),
            )
        } else {
            None
        };
        let mut times2: Vec<f64> = Vec::with_capacity(cli.max_run as usize);

        for _ in 0..cli.max_run {
            let mut t = cpu_timer::DeltaTimer::<false>::default();

            t.start();
            std::process::Command::new(&cmd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            t.stop();

            times2.push(t.value() as f64);
        }

        let times2: Vec<f64> = times2
            .iter()
            .map(|&val| round2(val as f64 / 1_000_000.0))
            .collect();

        println!("\n\n{}", perf);
        // println!("\n\nResult of {} for {} runs:\n", info, cli.max_run);
        min_max_raw(times, &times2);
        if times2.len() > 10 {
            histogram(&times2);
        } else {
            histogram_auto(&times2);
        }
        //  perf(&info);
    }
}

fn perf(path: &str) -> String {
    let output = std::process::Command::new("perf")
        .args(&[
            "stat",
            "-e",
            "cycles,task-clock,context-switches,cpu-migrations,instructions,branches,branch-misses,cache-references,cache-misses,page-faults",
            "--",
            path,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("gagal jalanin perf stat");

    String::from_utf8(output.stderr).unwrap()
}

fn min_max_raw(times: Option<Vec<usize>>, times2: &[f64]) {
    let min2 = times2.iter().cloned().fold(f64::INFINITY, f64::min);
    let max2 = times2.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg2 = times2.iter().sum::<f64>() / times2.len() as f64;

    let (iqr_min, iqr_max, iqr_avg) = iqr(times2);
    let (z_score_min, z_score_max, z_score_avg) = z_score(times2);

    println!(" Min:");

    let times_exist = !times.is_none();
    let times = if times_exist {
        times.unwrap()
    } else {
        Vec::new()
    };

    if times_exist {
        let min = times.iter().min().unwrap();
        println!(
            " tick: {}   raw: {} ms   iqr: {} ms   z_score: {} ms",
            min, min2, iqr_min, z_score_min
        );
    } else {
        println!(
            " raw: {} ms   iqr: {} ms   z_score: {} ms",
            min2, iqr_min, z_score_min
        );
    }

    println!("\n Max:");
    if times_exist {
        let max = times.iter().max().unwrap();
        println!(
            " tick: {}   raw: {} ms   iqr: {} ms   z_score: {} ms",
            max, max2, iqr_max, z_score_max
        );
    } else {
        println!(
            " raw: {} ms   iqr: {} ms   z_score: {} ms",
            max2, iqr_max, z_score_max
        );
    }

    println!("\n Avg:");
    if times_exist {
        let avg = times.iter().sum::<usize>() / times.len();
        println!(
            " tick: {:.2}   raw: {:.2} ms   iqr: {:.2} ms   z_score: {:.2} ms",
            avg, avg2, iqr_avg, z_score_avg
        );
    } else {
        println!(
            " raw: {:.2} ms   iqr: {:.2} ms   z_score: {:.2} ms",
            avg2, iqr_avg, z_score_avg
        );
    }
}

fn iqr(data: &[f64]) -> (f64, f64, f64) {
    let mut sorted = data.to_owned();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let q1 = sorted[(sorted.len() as f64 * 0.25).floor() as usize];
    let q3 = sorted[(sorted.len() as f64 * 0.75).floor() as usize];
    let iqr = q3 - q1;
    let lower_bound = q1 - 1.5 * iqr;
    let upper_bound = q3 + 1.5 * iqr;

    let filtered: Vec<f64> = sorted
        .into_iter()
        .filter(|&x| x >= lower_bound && x <= upper_bound)
        .collect();

    let min_no = filtered.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_no = filtered.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg_no = filtered.iter().sum::<f64>() / filtered.len() as f64;

    (min_no, max_no, avg_no)
}

fn z_score(times2: &[f64]) -> (f64, f64, f64) {
    let mean = times2.iter().sum::<f64>() / times2.len() as f64;
    let variance = times2.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / times2.len() as f64;
    let std_dev = variance.sqrt();

    let z_threshold = 3.0;
    let filtered_z: Vec<f64> = times2
        .iter()
        .cloned()
        .filter(|&x| ((x - mean) / std_dev).abs() <= z_threshold)
        .collect();
    let min_z = filtered_z.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_z = filtered_z.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg_z = filtered_z.iter().sum::<f64>() / filtered_z.len() as f64;

    (min_z, max_z, avg_z)
}

fn histogram(data: &[f64]) {
    let bins = 10;

    let step = 0.1;
    let min = round_down(data.iter().cloned().fold(f64::INFINITY, f64::min), step);
    let max = round_up(data.iter().cloned().fold(f64::NEG_INFINITY, f64::max), step);

    let range = max - min;
    let bin_width = range / bins as f64;

    let mut counts = vec![0usize; bins];

    for &val in data {
        let mut idx = ((val - min) / bin_width).floor() as usize;
        if idx >= bins {
            idx = bins - 1;
        }
        counts[idx] += 1;
    }

    println!("\n Detail:");
    let _ = (0..bins).for_each(|i| {
        let start = min + i as f64 * bin_width;
        let end = start + bin_width;
        println!(" {:.2} - {:.2} ms : {}", start, end, counts[i]);
    });
}

fn freedman_diaconis_bins(data: &[f64]) -> usize {
    let n = data.len();
    if n < 2 {
        return 1;
    }

    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let q1 = sorted[n / 4];
    let q3 = sorted[(3 * n) / 4];
    let iqr = q3 - q1;

    let bin_width = 2.0 * iqr / (n as f64).cbrt();
    if bin_width <= 0.0 {
        return (n as f64).sqrt().ceil() as usize;
    }

    let min = *sorted.first().unwrap();
    let max = *sorted.last().unwrap();

    let bins = ((max - min) / bin_width).ceil() as usize;
    bins.max(1)
}

fn histogram_auto(data: &[f64]) {
    let bins = freedman_diaconis_bins(data);

    let step = 0.1;
    let min = round_down(data.iter().cloned().fold(f64::INFINITY, f64::min), step);
    let max = round_up(data.iter().cloned().fold(f64::NEG_INFINITY, f64::max), step);

    let range = max - min;
    let bin_width = range / bins as f64;

    let mut counts = vec![0usize; bins];

    for &val in data {
        let mut idx = ((val - min) / bin_width).floor() as usize;
        if idx >= bins {
            idx = bins - 1;
        }
        counts[idx] += 1;
    }

    println!("\n Detail:");
    let _ = (0..bins).for_each(|i| {
        let start = min + i as f64 * bin_width;
        let end = start + bin_width;
        println!(" {:.2} - {:.2} ms : {}", start, end, counts[i]);
    });
}

fn round_down(x: f64, step: f64) -> f64 {
    (x / step).floor() * step
}

fn round_up(x: f64, step: f64) -> f64 {
    (x / step).ceil() * step
}

fn round2(val: f64) -> f64 {
    (val * 100.0).round() / 100.0
}

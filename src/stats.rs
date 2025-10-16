use std::collections::HashMap;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub fn req_per_s(success: usize, duration: &std::time::Duration) {
    let req_per_sec = success as f64 / duration.as_secs_f64();
    println!(" Req/s        : {:.2}", req_per_sec);
}

pub fn success_rate(total_send: u64, success: usize) {
    if total_send > 0 {
        println!(
            " Success rate : {:.2}%",
            (success as f64 / total_send as f64) * 100.0
        );
    } else {
        println!(" Success rate : 0.00%");
    }
}

pub fn min_ms(times: &Vec<u64>) {
    let min_time = times
        .iter()
        .min()
        .expect(" no http benchmark value is recorded");
    let min_ms = *min_time as f64 / 1_000_000.0;
    println!(" Min          : {:.2} ms", min_ms);
}

pub fn max_ms(times: &Vec<u64>) {
    let max_time = times.iter().max().unwrap();
    let max_ms = *max_time as f64 / 1_000_000.0;
    println!(" Max          : {:.2} ms", max_ms);
}

pub fn avg_ms(success: usize, times: &Vec<u64>) {
    let total_times = times.iter().sum::<u64>();
    let avg_ms = (total_times as f64 / success as f64) / 1_000_000.0;
    println!(" Avg          : {:.2} ms", avg_ms);
}

pub fn median_ms(success: usize, times: &Vec<u64>) {
    let median_ms = if success % 2 == 0 {
        let mid = success / 2;
        (times[mid - 1] + times[mid]) as f64 / 2.0 / 1_000_000.0
    } else {
        times[success / 2] as f64 / 1_000_000.0
    };
    println!(" Median       : {:.2} ms", median_ms);
}

pub fn mode_or_modus(times: &Vec<u64>) {
    let mut freq: HashMap<u64, usize> = HashMap::new();
    for t in times {
        *freq.entry(*t).or_insert(0) += 1;
    }

    let max_count = freq.values().copied().max().unwrap_or(0);

    let modes: Vec<_> = freq
        .into_iter()
        .filter(|&(_, count)| count == max_count)
        .map(|(val, _)| val as f64 / 1_000_000.0)
        .collect();

    if modes.len() == 1 {
        println!(" Mode/Modus   : {:.2} ms", modes[0]);
    } else {
        println!(" Modes (in ms): {:?}", modes);
    }
}

pub fn p90_p99(success: usize, times: &Vec<u64>) {
    let p90_idx = (0.90 * (success as f64 - 1.0)) as usize;
    let p99_idx = (0.99 * (success as f64 - 1.0)) as usize;
    let p90_ms = times[p90_idx] as f64 / 1_000_000.0;
    let p99_ms = times[p99_idx] as f64 / 1_000_000.0;
    println!(" p90          : {:.2} ms", p90_ms);
    println!(" p99          : {:.2} ms", p99_ms);
}

pub fn grouped_ms(times: &Vec<u64>) {
    let mut buckets = [0u64; 11];

    for t_ns in times.iter() {
        let t_ms = *t_ns / 1_000_000;
        match t_ms {
            0..=10 => buckets[0] += 1,
            11..=20 => buckets[1] += 1,
            21..=30 => buckets[2] += 1,
            31..=40 => buckets[3] += 1,
            41..=50 => buckets[4] += 1,
            51..=70 => buckets[5] += 1,
            71..=100 => buckets[6] += 1,
            101..=150 => buckets[7] += 1,
            151..=200 => buckets[8] += 1,
            201..=300 => buckets[9] += 1,
            _ => buckets[10] += 1,
        }
    }

    println!(" 0-10ms       : {}", buckets[0]);
    println!(" 11-20ms      : {}", buckets[1]);
    println!(" 21-30ms      : {}", buckets[2]);
    println!(" 31-40ms      : {}", buckets[3]);
    println!(" 41-50ms      : {}", buckets[4]);
    println!(" 51-70ms      : {}", buckets[5]);
    println!(" 71-100ms     : {}", buckets[6]);
    println!(" 101-150ms    : {}", buckets[7]);
    println!(" 151-200ms    : {}", buckets[8]);
    println!(" 201-300ms    : {}", buckets[9]);
    println!(" 301ms+       : {}", buckets[10]);
}

pub fn cpu_usage(s: &mut System, child: &tokio::process::Child) {
    s.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu(),
    );
    if let Some(process) = s.process(Pid::from(child.id().unwrap() as usize)) {
        println!(" CPU utilization: {}%", process.cpu_usage());
    }
}

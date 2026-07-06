use std::process::Stdio;

use crate::stats;


pub fn run_cli(config: crate::config::Config) {
    stats::print_system_info();

    for app in config.app {
        let Some(cmd_cfg) = app.command else {
            eprintln!("error: [cli] benchmark requires a [[app.command]] entry; skipping");
            continue;
        };
        let Some(cli_cfg) = app.cli else {
            eprintln!("error: [cli] benchmark requires an [[app.cli]] entry; skipping");
            continue;
        };

        let exe = &cmd_cfg.first;
        println!("\n Benchmarking '{exe}' for {} runs", cli_cfg.max_run);

        // Warm up run (not timed)
        if let Err(e) = std::process::Command::new(exe)
            .args(cmd_cfg.args.as_deref().unwrap_or(&[]))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            eprintln!("error: warmup run of '{exe}' failed: {e}");
            continue;
        }

        // Optional, collect CPU tick timings
        let tick_times: Option<Vec<u64>> = if cli_cfg.tick {
            Some(collect_tick_times(exe, cmd_cfg.args.as_deref().unwrap_or(&[]), cli_cfg.max_run))
        } else {
            None
        };

        // Always collect wall clock timings (nanoseconds)
        let ns_times = collect_wall_times(exe, cmd_cfg.args.as_deref().unwrap_or(&[]), cli_cfg.max_run);

        if ns_times.is_empty() {
            eprintln!("error: no timing data collected for '{exe}'");
            continue;
        }

        // perf stat run
        let perf_output = run_perf(exe, cmd_cfg.args.as_deref().unwrap_or(&[]));
        println!("\n{perf_output}");

        print_cli_stats(exe, &ns_times, tick_times.as_deref());
    }
}


fn collect_wall_times(exe: &str, args: &[String], runs: u32) -> Vec<u64> {
    let mut times = Vec::with_capacity(runs as usize);
    for _ in 0..runs {
        let mut t = cpu_timer::DeltaTimer::<false>::default();
        t.start();
        let ok = std::process::Command::new(exe)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        t.stop();
        if ok {
            // DeltaTimer<false> returns nanoseconds (wall clock)
            times.push(t.value() as u64);
        }
    }
    times
}

fn collect_tick_times(exe: &str, args: &[String], runs: u32) -> Vec<u64> {
    let mut times = Vec::with_capacity(runs as usize);
    for _ in 0..runs {
        let mut t = cpu_timer::DeltaTimer::<true>::default();
        t.start();
        let ok = std::process::Command::new(exe)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        t.stop();
        if ok {
            times.push(t.value() as u64);
        }
    }
    times
}

fn run_perf(exe: &str, args: &[String]) -> String {
    let output = std::process::Command::new("perf")
        .args([
            "stat", "-e",
            "cycles,task-clock,context-switches,cpu-migrations,\
             instructions,branches,branch-misses,cache-references,\
             cache-misses,page-faults",
            "--", exe,
        ])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(o)  => String::from_utf8_lossy(&o.stderr).into_owned(),
        Err(e) => format!("(perf not available: {e})"),
    }
}


fn print_cli_stats(label: &str, ns: &[u64], ticks: Option<&[u64]>) {
    let mut sorted = ns.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();

    println!("\n Result for '{}' ({n} runs):", label);

    let ns_min = *sorted.first().unwrap();
    let ns_max = *sorted.last().unwrap();
    let ns_avg = sorted.iter().sum::<u64>() / n as u64;
    let ns_med = median_u64(&sorted);

    println!(
        " Min    : {:.3} ms{}",
        ns_to_ms(ns_min),
        ticks.map(|t| format!("  (ticks: {})", *t.iter().min().unwrap_or(&0))).unwrap_or_default()
    );
    println!(
        " Max    : {:.3} ms{}",
        ns_to_ms(ns_max),
        ticks.map(|t| format!("  (ticks: {})", *t.iter().max().unwrap_or(&0))).unwrap_or_default()
    );
    println!(
        " Avg    : {:.3} ms{}",
        ns_to_ms(ns_avg),
        ticks.map(|t| format!("  (ticks: {})", t.iter().sum::<u64>() / t.len().max(1) as u64)).unwrap_or_default()
    );
    println!(" Median : {:.3} ms", ns_to_ms(ns_med));

    // IQR filtered
    let (iqr_min, iqr_max, iqr_avg) = iqr_stats(ns);
    println!(" IQR min/max/avg : {:.3} / {:.3} / {:.3} ms",
        ns_to_ms(iqr_min), ns_to_ms(iqr_max), ns_to_ms(iqr_avg));

    // Z-score filtered
    let (z_min, z_max, z_avg) = z_score_stats(ns);
    println!(" Z-score min/max/avg : {:.3} / {:.3} / {:.3} ms",
        ns_to_ms(z_min), ns_to_ms(z_max), ns_to_ms(z_avg));

    print_cli_histogram(&sorted);
}

fn ns_to_ms(ns: u64) -> f64 { ns as f64 / 1_000_000.0 }

fn median_u64(sorted: &[u64]) -> u64 {
    let n = sorted.len();
    if n % 2 == 0 { (sorted[n / 2 - 1] + sorted[n / 2]) / 2 } else { sorted[n / 2] }
}

fn iqr_stats(ns: &[u64]) -> (u64, u64, u64) {
    if ns.is_empty() { return (0, 0, 0); }
    let mut s = ns.to_vec();
    s.sort_unstable();
    let n = s.len();
    let q1 = s[n / 4];
    let q3 = s[(3 * n) / 4];
    let iqr = q3.saturating_sub(q1);
    let lo  = q1.saturating_sub(iqr + iqr / 2);     // q1 - 1.5*iqr (saturating)
    let hi  = q3 + iqr + iqr / 2;                    // q3 + 1.5*iqr
    let filtered: Vec<u64> = s.into_iter().filter(|&x| x >= lo && x <= hi).collect();
    if filtered.is_empty() { return (0, 0, 0); }
    let mn  = *filtered.first().unwrap();
    let mx  = *filtered.last().unwrap();
    let avg = filtered.iter().sum::<u64>() / filtered.len() as u64;
    (mn, mx, avg)
}

fn z_score_stats(ns: &[u64]) -> (u64, u64, u64) {
    if ns.len() < 2 { return (ns.first().copied().unwrap_or(0), ns.first().copied().unwrap_or(0), ns.first().copied().unwrap_or(0)); }
    let mean = ns.iter().sum::<u64>() as f64 / ns.len() as f64;
    let var   = ns.iter().map(|&x| { let d = x as f64 - mean; d * d }).sum::<f64>() / ns.len() as f64;
    let std   = var.sqrt();
    let filtered: Vec<u64> = if std > 0.0 {
        ns.iter().copied().filter(|&x| ((x as f64 - mean) / std).abs() <= 3.0).collect()
    } else {
        ns.to_vec()
    };
    if filtered.is_empty() { return (0, 0, 0); }
    let mn  = *filtered.iter().min().unwrap();
    let mx  = *filtered.iter().max().unwrap();
    let avg = filtered.iter().sum::<u64>() / filtered.len() as u64;
    (mn, mx, avg)
}

fn print_cli_histogram(sorted_ns: &[u64]) {
    if sorted_ns.len() < 2 { return; }
    let bins = freedman_diaconis_bins(sorted_ns).clamp(1, 20);
    let mn   = *sorted_ns.first().unwrap() as f64;
    let mx   = *sorted_ns.last().unwrap()  as f64;
    let rng  = mx - mn;
    if rng <= 0.0 { return; }

    let bin_w = rng / bins as f64;
    let mut counts = vec![0usize; bins];
    for &ns in sorted_ns {
        let idx = (((ns as f64 - mn) / bin_w) as usize).min(bins - 1);
        counts[idx] += 1;
    }

    println!("\n Detail:");
    for (i, &c) in counts.iter().enumerate() {
        let lo = mn + i as f64 * bin_w;
        let hi = lo + bin_w;
        println!("  {:.2} - {:.2} ms : {c}", lo / 1_000_000.0, hi / 1_000_000.0);
    }
}

fn freedman_diaconis_bins(sorted: &[u64]) -> usize {
    let n = sorted.len();
    if n < 2 { return 1; }
    let q1 = sorted[n / 4] as f64;
    let q3 = sorted[(3 * n) / 4] as f64;
    let iqr = q3 - q1;
    if iqr <= 0.0 { return (n as f64).sqrt().ceil() as usize; }
    let bin_w = 2.0 * iqr / (n as f64).cbrt();
    let mn = *sorted.first().unwrap() as f64;
    let mx = *sorted.last().unwrap()  as f64;
    ((mx - mn) / bin_w).ceil() as usize
}
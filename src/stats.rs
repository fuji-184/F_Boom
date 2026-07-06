use std::collections::HashMap;

// All timing samples in nanoseconds, already sorted ascending
pub struct BenchResult {
    // Name of the benchmark (command or URL)
    pub label:      String,
    // Target URL (empty for CLI benchmarks)
    pub url:        String,
    // Wall clock duration of the whole benchmark.
    pub duration:   std::time::Duration,
    // Nanosecond latencies of successful requests, sorted ascending
    pub times_ns:   Vec<u64>,
    // Total requests attempted (success + failure)
    pub total_sent: u64,
}

impl BenchResult {
    pub fn success(&self) -> usize { self.times_ns.len() }
}

pub fn print_result(r: &BenchResult) {
    let n = r.success();

    println!(
        "\n\n Result for '{}' on '{}':\n Success      : {}/{} in {:.2}s",
        r.label, r.url, n, r.total_sent, r.duration.as_secs_f64()
    );

    if n == 0 {
        println!(" (no successful requests recorded)");
        return;
    }

    let t = &r.times_ns;

    print_success_rate(r.total_sent, n);
    print_rps(n, r.duration);
    print_min(t);
    print_max(t);
    print_avg(n, t);
    print_median(n, t);
    print_mode(t);
    print_percentiles(n, t);
    print_histogram(t);
}

fn print_success_rate(total: u64, success: usize) {
    let rate = if total > 0 {
        success as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    println!(" Success rate : {:.2}%", rate);
}

fn print_rps(success: usize, duration: std::time::Duration) {
    println!(" Req/s        : {:.2}", success as f64 / duration.as_secs_f64());
}

fn ns_to_ms(ns: u64) -> f64 { ns as f64 / 1_000_000.0 }

fn print_min(t: &[u64]) {
    println!(" Min          : {:.3} ms", ns_to_ms(*t.first().unwrap()));
}

fn print_max(t: &[u64]) {
    println!(" Max          : {:.3} ms", ns_to_ms(*t.last().unwrap()));
}

fn print_avg(n: usize, t: &[u64]) {
    let sum: u64 = t.iter().sum();
    println!(" Avg          : {:.3} ms", ns_to_ms(sum / n as u64));
}

fn print_median(n: usize, t: &[u64]) {
    let ms = if n % 2 == 0 {
        (t[n / 2 - 1] + t[n / 2]) as f64 / 2.0 / 1_000_000.0
    } else {
        ns_to_ms(t[n / 2])
    };
    println!(" Median       : {:.3} ms", ms);
}

fn print_mode(t: &[u64]) {
    // Group by millisecond bucket to keep the map small
    let mut freq: HashMap<u64, u32> = HashMap::with_capacity(t.len().min(4096));
    for &ns in t {
        *freq.entry(ns / 1_000_000).or_insert(0) += 1;
    }
    let max_count = freq.values().copied().max().unwrap_or(0);
    let mut modes: Vec<u64> = freq
        .into_iter()
        .filter(|&(_, c)| c == max_count)
        .map(|(ms, _)| ms)
        .collect();
    modes.sort_unstable();

    if modes.len() == 1 {
        println!(" Mode         : {} ms", modes[0]);
    } else {
        let strs: Vec<_> = modes.iter().map(|m| format!("{m} ms")).collect();
        println!(" Mode         : [{}]", strs.join(", "));
    }
}

fn print_percentiles(n: usize, t: &[u64]) {
    let p = |pct: f64| ns_to_ms(t[((pct * (n as f64 - 1.0)) as usize).min(n - 1)]);
    println!(" p50          : {:.3} ms", p(0.50));
    println!(" p90          : {:.3} ms", p(0.90));
    println!(" p95          : {:.3} ms", p(0.95));
    println!(" p99          : {:.3} ms", p(0.99));
}

fn print_histogram(t: &[u64]) {
    // Fixed buckets in ms
    const EDGES: &[u64] = &[1, 2, 5, 10, 20, 30, 50, 75, 100, 150, 200, 300, 500, 1000];
    let mut buckets = [0u64; 15]; // 14 edges → 15 buckets

    for &ns in t {
        let ms = ns / 1_000_000;
        let idx = EDGES.partition_point(|&e| ms >= e); 
        buckets[idx] += 1;
    }

    println!("\n Latency distribution:");
    let labels = [
        "  <1ms", "  1ms", "  2ms", "  5ms", " 10ms", " 20ms",
        " 30ms", " 50ms", " 75ms", "100ms", "150ms", "200ms",
        "300ms", "500ms", ">1s  ",
    ];
    let max_count = buckets.iter().copied().max().unwrap_or(1).max(1);
    let bar_width = 40usize;
    for (i, &count) in buckets.iter().enumerate() {
        if count == 0 { continue; }
        let bar_len = (count as usize * bar_width / max_count as usize).max(1);
        let bar: String = std::iter::repeat('#').take(bar_len).collect();
        println!("  {} | {:>6} | {}", labels[i], count, bar);
    }
}

pub fn print_system_info() {
    use sysinfo::System;
    let mut sys = System::new_all();
    sys.refresh_all();

    println!("\n System info:");
    if let Some(v) = System::name()           { println!("  OS        : {v}"); }
    if let Some(v) = System::kernel_version() { println!("  Kernel    : {v}"); }
    if let Some(v) = System::os_version()     { println!("  Version   : {v}"); }
    if let Some(v) = System::host_name()      { println!("  Hostname  : {v}"); }

    if let Some(cpu) = sys.cpus().first() {
        println!(
            "  CPU       : {} arch, {} cores, {} {} @ {} MHz",
            System::cpu_arch().unwrap(),
            sys.cpus().len(),
            cpu.vendor_id(),
            cpu.brand().trim(),
            cpu.frequency(),
        );
    }

    println!("\n Memory:");
    println!("  Total     : {}", fmt_bytes(sys.total_memory()));
    println!("  Used      : {}", fmt_bytes(sys.used_memory()));
    println!("  Available : {}", fmt_bytes(sys.available_memory()));
    println!("  Swap used : {}", fmt_bytes(sys.used_swap()));
}

fn fmt_bytes(bytes: u64) -> String {
    let gb = bytes as f64 / (1 << 30) as f64;
    if gb >= 1.0 { return format!("{gb:.2} GB"); }
    let mb = bytes as f64 / (1 << 20) as f64;
    if mb >= 1.0 { return format!("{mb:.2} MB"); }
    format!("{:.2} KB", bytes as f64 / 1024.0)
}
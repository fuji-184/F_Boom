use crate::stats;
use sysinfo::{Pid, System};
use tokio::fs;
use tokio::time::{Duration, interval};

pub fn stats(mut hasil: crate::http::Hasil) {
    hasil.times.sort();
    let success = hasil.times.len();

    println!(
        "\n\nResult of {} on url {}:\nSuccess      : {}/{} in {:.2} seconds",
        hasil.command,
        hasil.url,
        success,
        hasil.total_send,
        hasil.duration.as_secs_f64()
    );

    println!("success: {}", hasil.times.len());

    let mut times = vec![];
    for val in hasil.times.iter() {
        let time = val.as_nanos() as u64;
        times.push(time);
    }

    drop(hasil.times);

    stats::success_rate(hasil.total_send, success);
    stats::req_per_s(success, &hasil.duration);
    stats::min_ms(&times);
    stats::max_ms(&times);
    stats::avg_ms(success, &times);
    stats::median_ms(success, &times);
    stats::mode_or_modus(&times);
    stats::p90_p99(success, &times);
    stats::grouped_ms(&times);
}

pub fn system_info() {
    let mut sys = System::new_all();

    sys.refresh_all();

    //std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);

    if let Some(val) = System::name() {
        println!("\n\nUsing machine:\nSystem name    : {}", val);
    }

    if let Some(val) = System::kernel_version() {
        println!("Kernel version : {}", val);
    }

    if let Some(val) = System::os_version() {
        println!("OS version     : {}", val);
    }

    if let Some(val) = System::host_name() {
        println!("Host name      : {}", val);
    }

    for (_, cpu) in sys.cpus().iter().enumerate() {
        println!(
            "CPU            : {}, {} cores {} {} {} mhz",
            System::cpu_arch(),
            sys.cpus().len(),
            cpu.vendor_id(),
            cpu.brand(),
            cpu.frequency(),
        );
        break;
    }

    /*
    println!("CPU usage (global): {:.2}%", sys.global_cpu_usage());

    for (i, cpu) in sys.cpus().iter().enumerate() {
        println!("CPU {} usage: {:.2}%", i, cpu.cpu_usage());
    }
    */

    println!("\n\nRAM info : \n");
    println!("Total RAM     : {}", format_memory(sys.total_memory()));
    println!("Used RAM      : {}", format_memory(sys.used_memory()));
    println!("Available RAM : {}", format_memory(sys.available_memory()));
    println!("Free RAM      : {}", format_memory(sys.free_memory()));

    println!("Total Swap    : {}", format_memory(sys.total_swap()));
    println!("Used Swap     : {}\n\n", format_memory(sys.used_swap()));
}

fn format_memory(bytes: u64) -> String {
    let kb = bytes as f64 / 1024.0;
    let mb = kb / 1024.0;
    let gb = mb / 1024.0;

    if gb >= 1.0 {
        format!("{:.2} GB", gb)
    } else if mb >= 1.0 {
        format!("{:.2} MB", mb)
    } else {
        format!("{:.2} KB", kb)
    }
}

async fn monitor_ram_proc(pid: u32) {
    let mut ticker = interval(Duration::from_secs(5));

    loop {
        ticker.tick().await;
        let command = format!("/proc/{}/status", pid);
        if let Ok(content) = fs::read_to_string(command).await {
            for line in content.lines() {
                if line.starts_with("VmRSS:") {
                    println!("[ram] {}", line); // contoh: "VmRSS:   123456 kB"
                }
            }
        } else {
            eprintln!("[ram] process {} sudah mati", pid);
            break;
        }
    }
}

pub async fn memory_usage(pid: u32) -> String {
    let path = format!("/proc/{}/smaps", pid);
    let output = tokio::process::Command::new("grep")
        .args(["-E", r"\[heap\]|\[stack\]", "-A", "15", &path])
        .output()
        .await
        .unwrap();

    let output2 = tokio::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid,ppid,cmd,%mem,rss,vsz"])
        .output()
        .await
        .unwrap();
    unsafe {
        format!(
            "{}\n{}",
            String::from_utf8_unchecked(output.stdout),
            String::from_utf8_unchecked(output2.stdout)
        )
    }
}

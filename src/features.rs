use crate::stats;
use sysinfo::{System, Pid};

pub fn stats(duration: std::time::Duration, total_send: u64, mut times: Vec<u64>){
    times.sort();
    let success = times.len();
    
    println!(
        "Success      : {}/{} in {:.2} seconds",
        success,
        total_send,
        duration.as_secs_f64()
    );
    
    stats::success_rate(total_send, success);
    stats::req_per_s(success, &duration);
    stats::min_ms(&times);
    stats::max_ms(&times);
    stats::avg_ms(success, &times);
    stats::median_ms(success, &times);
    stats::mode_or_modus(&times);
    stats::p90_p99(success, &times);
    stats::grouped_ms(&times);
}

pub fn system_info(){
  let mut sys = System::new_all();

    sys.refresh_all();
    
    //std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    
    if let Some(val) = System::name() {
      println!("\n\nSystem name    : {}", val);
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
      println!("CPU            : {}, {} cores {} {} {} mhz",
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
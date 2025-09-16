#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod config_reader;
mod features;
mod http;
mod run_app;
mod stats;

pub struct Data {
    time: Option<std::time::Duration>,
    total_send: Option<u64>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    http::http_benchmark();
    println!("\n");
    Ok(())
}

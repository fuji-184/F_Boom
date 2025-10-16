#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;
mod config_reader;
mod features;
mod grpc;
mod http;
mod run_app;
mod stats;
mod ws;

pub struct Data {
    time: Option<std::time::Duration>,
    total_send: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = clap::Command::new("f_boom")
        .version("0.1")
        .author("Fuji")
        .about("App boombarder")
        .subcommand(
            clap::Command::new("http").about("run with config").arg(
                clap::Arg::new("config_path")
                    .value_name("config_path")
                    .required(true)
                    .help("The path of the config"),
            ),
        )
        .subcommand(
            clap::Command::new("ws").about("run with config").arg(
                clap::Arg::new("config_path")
                    .value_name("config_path")
                    .required(true)
                    .help("The path of the config"),
            ),
        )
        .subcommand(
            clap::Command::new("grpc").about("run with config").arg(
                clap::Arg::new("config_path")
                    .value_name("config_path")
                    .required(true)
                    .help("The path of the config"),
            ),
        )
        .subcommand(
            clap::Command::new("cli").about("run with config").arg(
                clap::Arg::new("config_path")
                    .value_name("config_path")
                    .required(true)
                    .help("The path of the config"),
            ),
        )
        .get_matches();

    match matches.subcommand_name() {
        Some("http") => {
            if let Some(init_matches) = matches.subcommand_matches("http") {
                let config_path = init_matches
                    .get_one::<String>("config_path")
                    .unwrap()
                    .to_string();
                let config = crate::config_reader::read_config(&config_path);

                http::http_benchmark(config);

                println!("\n");
            }
        }
        Some("ws") => {
            if let Some(init_matches) = matches.subcommand_matches("ws") {
                let config_path = init_matches
                    .get_one::<String>("config_path")
                    .unwrap()
                    .to_string();
                let config = crate::config_reader::read_config(&config_path);

                ws::run_ws(config);

                println!("\n");
            }
        }
        Some("grpc") => {
            if let Some(init_matches) = matches.subcommand_matches("grpc") {
                let config_path = init_matches
                    .get_one::<String>("config_path")
                    .unwrap()
                    .to_string();
                let config = crate::config_reader::read_config(&config_path);

                grpc::run_grpc(config);

                println!("\n");
            }
        }
        Some("cli") => {
            if let Some(init_matches) = matches.subcommand_matches("cli") {
                let config_path = init_matches
                    .get_one::<String>("config_path")
                    .unwrap()
                    .to_string();
                let config = crate::config_reader::read_config(&config_path);

                cli::run_cli_benchmark(config);

                println!("\n");
            }
        }

        _ => println!("invalid command, use f_boom --help to see all the available commands"),
    }

    Ok(())
}

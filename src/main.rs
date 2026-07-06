#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cli;
mod config;
mod grpc;
mod http;
mod process;
mod stats;
mod ws;

use clap::{Arg, Command};
use std::path::PathBuf;

fn main() {
    let matches = Command::new("f_boom")
        .version("0.1.0")
        .author("Fuji")
        .about("High-performance load generator for HTTP, WebSocket, gRPC, and CLI")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("init").about("Initialize a default config.toml file in the current directory"))
        .subcommand(sub("http",  "Run an HTTP benchmark using a config file"))
        .subcommand(sub("ws",    "Run a WebSocket benchmark using a config file"))
        .subcommand(sub("grpc",  "Run a gRPC benchmark using a config file"))
        .subcommand(sub("cli",   "Run a CLI (process) benchmark using a config file"))
        .get_matches();

    let (subcommand, sub_matches) = matches.subcommand().unwrap();

    if subcommand == "init" {
        const DEFAULT_CONFIG: &str = include_str!("../config.toml");
        let target_file = "config.toml";

        if std::path::Path::new(target_file).exists() {
            eprintln!("error: config.toml already exists in the current directory, rename it to fix the duplicated filename");
            std::process::exit(1);
        }

        match std::fs::write(target_file, DEFAULT_CONFIG) {
            Ok(_) => {
                println!("Successfully initialized default config.toml in the current directory");
            }
            Err(e) => {
                eprintln!("error: failed to write config.toml: {e}");
                std::process::exit(1);
            }
        }
        return; 
    }

    let config_path = PathBuf::from(
        sub_matches
            .get_one::<String>("config")
            .expect("config path is required"),
    );

    let config = match config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    match subcommand {
        "http" => http::run_http(config),
        "ws"   => ws::run_ws(config),
        "grpc" => grpc::run_grpc(config),
        "cli"  => cli::run_cli(config),
        _      => unreachable!(), // Clap guarantees all condition is handled
    }

    println!();
}

fn sub(name: &'static str, about: &'static str) -> Command {
    Command::new(name).about(about).arg(
        Arg::new("config")
            .value_name("CONFIG")
            .required(true)
            .help("Path to the TOML config file"),
    )
}
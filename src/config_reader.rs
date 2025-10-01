use serde::Deserialize;

#[derive(Deserialize, Debug, PartialEq)]
pub struct Config {
    pub app: Option<Vec<App>>,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct App {
    pub http: Option<Vec<Http>>,
    pub ws: Option<Vec<Ws>>,
    pub grpc: Option<Vec<Grpc>>,
    pub perf: Option<bool>,
    pub command: Option<Command>,
    pub terminal: bool,
    pub cli: Option<Cli>,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Http {
    pub url: String,
    pub max_concurrent: i32,
    pub max_duration: i64,
    pub timeout: u64,
    pub mode: Vec<String>,
    pub method: String,
    pub payload: Option<Payload>,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Payload {
    pub r#type: String,
    pub val: String,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Ws {
    pub url: String,
    pub max_concurrent: i32,
    pub max_duration: i64,
    pub payload: WsPayload,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct WsPayload {
    pub r#type: String,
    pub val: String,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Grpc {
    pub url: String,
    pub max_concurrent: i32,
    pub max_duration: i64,
    pub mode: String,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Command {
    pub first: String,
    pub args: Option<Vec<String>>,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Cli {
    pub max_run: i32,
    pub max_duration: i64,
}

pub fn read_config(path: &str) -> Config {
    let isi = std::fs::read_to_string(path).unwrap();
    let config: Config = toml::from_str(&isi).unwrap();
    config
}

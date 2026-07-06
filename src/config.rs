use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub app: Vec<App>,
}

#[derive(Debug, Deserialize)]
pub struct App {
    pub command:  Option<Command>,
    pub terminal: Option<bool>,
    pub perf:     Option<bool>,
    pub http:     Option<Vec<HttpConfig>>,
    pub ws:       Option<Vec<WsConfig>>,
    pub grpc:     Option<Vec<GrpcConfig>>,
    pub cli:      Option<CliConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpConfig {
    pub url:            String,
    pub max_concurrent: u32,
    pub max_duration:   u64,
    #[serde(default = "default_timeout")]
    pub timeout:        u64,
    #[serde(default = "default_http_mode")]
    pub mode:           Vec<String>,
    #[serde(default = "default_get")]
    pub method:         String,
    pub payload:        Option<Payload>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WsConfig {
    pub url:            String,
    pub max_concurrent: u32,
    pub max_duration:   u64,
    pub payload:        WsPayload,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GrpcConfig {
    pub url:            String,
    pub max_concurrent: u32,
    pub max_duration:   u64,
    pub mode:           String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CliConfig {
    pub max_run:     u32,
    pub max_duration: u64,
    /// If true, also measure in CPU ticks
    #[serde(default)]
    pub tick:        bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Command {
    pub first: String,
    pub args:  Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Payload {
    #[serde(rename = "type")]
    pub kind: String,
    pub val:  String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WsPayload {
    #[serde(rename = "type")]
    pub kind: String,
    pub val:  String,
}

fn default_timeout() -> u64 { 30 }
fn default_http_mode() -> Vec<String> { vec!["http1".into()] }
fn default_get() -> String { "get".into() }

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e)    => write!(f, "cannot read config file: {e}"),
            ConfigError::Parse(e) => write!(f, "invalid config syntax: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
    toml::from_str(&text).map_err(ConfigError::Parse)
}
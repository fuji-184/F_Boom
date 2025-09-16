use serde::Deserialize;

#[derive(Deserialize, Debug, PartialEq)]
pub struct Config {
    pub app: Option<Vec<App>>,
}

#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct App {
    pub http: Option<Vec<Http>>,
    pub perf: Option<bool>,
    pub command: Option<Command>,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Http {
    pub url: String,
    pub max_concurrent: i32,
    pub max_duration: i64,
    pub batch_size: i32,
    pub timeout: u64,
}

#[derive(Clone, Deserialize, Debug, PartialEq)]
pub struct Command {
    pub first: String,
    pub args: Option<Vec<String>>,
}

pub async fn read_config(path: &str) -> Config {
    let isi = tokio::fs::read_to_string(path).await.unwrap();
    let config: Config = toml::from_str(&isi).unwrap();
    config
}

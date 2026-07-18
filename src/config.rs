use std::{env, fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::Deserialize;
use url::Url;

use crate::cli::{Cli, TailArgs};

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub web_url: String,
    pub tenant: String,
    pub api_key: Option<String>,
    pub poll_interval_ms: u64,
    pub window_seconds: u64,
    pub buffer_size: usize,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    url: Option<String>,
    web_url: Option<String>,
    tenant: Option<String>,
    api_key: Option<String>,
    poll_interval_ms: Option<u64>,
    window_seconds: Option<u64>,
    buffer_size: Option<usize>,
}

impl Config {
    pub fn load(cli: &Cli, tail: &TailArgs) -> Result<Self> {
        let path = cli.config.clone().or_else(default_config_path);
        let file = match path.as_ref() {
            Some(path) if path.exists() => {
                let text = fs::read_to_string(path)
                    .with_context(|| format!("failed to read config {}", path.display()))?;
                toml::from_str::<FileConfig>(&text)
                    .with_context(|| format!("invalid config {}", path.display()))?
            }
            Some(path) if cli.config.is_some() => {
                bail!("config file does not exist: {}", path.display())
            }
            _ => FileConfig::default(),
        };

        let url = first(cli.url.clone(), env_value("RUSH_URL"), file.url)
            .unwrap_or_else(|| "http://localhost:8080".to_string());
        let web_url = first(cli.web_url.clone(), env_value("RUSH_WEB_URL"), file.web_url)
            .unwrap_or_else(|| "http://localhost:5173".to_string());
        validate_base_url(&url, "API")?;
        validate_base_url(&web_url, "web UI")?;

        let poll_interval_ms = tail
            .poll_interval_ms
            .or_else(|| env_parse("RUSH_POLL_INTERVAL_MS"))
            .or(file.poll_interval_ms)
            .unwrap_or(1000)
            .clamp(250, 60_000);
        let window_seconds = tail
            .window_seconds
            .or_else(|| env_parse("RUSH_WINDOW_SECONDS"))
            .or(file.window_seconds)
            .unwrap_or(300)
            .clamp(10, 7 * 24 * 60 * 60);
        let buffer_size = tail
            .buffer_size
            .or_else(|| env_parse("RUSH_BUFFER_SIZE"))
            .or(file.buffer_size)
            .unwrap_or(5000)
            .clamp(100, 100_000);

        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            web_url: web_url.trim_end_matches('/').to_string(),
            tenant: first(cli.tenant.clone(), env_value("RUSH_TENANT"), file.tenant)
                .unwrap_or_else(|| "default".to_string()),
            api_key: first(cli.api_key.clone(), env_value("RUSH_API_KEY"), file.api_key)
                .filter(|value| !value.trim().is_empty()),
            poll_interval_ms,
            window_seconds,
            buffer_size,
        })
    }
}

fn first<T>(a: Option<T>, b: Option<T>, c: Option<T>) -> Option<T> {
    a.or(b).or(c)
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_value(name).and_then(|value| value.parse().ok())
}

fn validate_base_url(value: &str, label: &str) -> Result<()> {
    let parsed = Url::parse(value).with_context(|| format!("invalid {label} URL: {value}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("{label} URL must be an http(s) URL: {value}")
    }
    Ok(())
}

pub fn default_config_path() -> Option<PathBuf> {
    ProjectDirs::from("com", "RushObservability", "rush")
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_only_http_urls() {
        assert!(validate_base_url("https://rush.example", "API").is_ok());
        assert!(validate_base_url("file:///tmp/socket", "API").is_err());
    }
}

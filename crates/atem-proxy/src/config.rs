use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Upstream ATEM IP or host:port (default port 9910).
    pub atem: String,
    /// Local bind address for client-facing UDP.
    pub bind: SocketAddr,
    /// Enable mDNS announcement as a Blackmagic device.
    pub mdns: bool,
    /// Idle client timeout in milliseconds.
    pub client_idle_ms: u64,
    /// Upstream reconnect base backoff in milliseconds.
    pub reconnect_ms: u64,
    /// Log filter (RUST_LOG compatible).
    pub log: String,
    /// Optional log file path (useful for Windows Service).
    pub log_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            atem: "192.168.1.50".to_string(),
            bind: "0.0.0.0:9910".parse().unwrap(),
            mdns: false,
            client_idle_ms: 5000,
            reconnect_ms: 1000,
            log: "info".to_string(),
            log_file: None,
        }
    }
}

impl Config {
    pub fn load(
        config_path: Option<&Path>,
        atem: Option<String>,
        bind: Option<SocketAddr>,
        mdns: Option<bool>,
        log: Option<String>,
    ) -> Result<Self> {
        let mut cfg = Self::default();

        if let Some(path) = config_path {
            if path.exists() {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading config {}", path.display()))?;
                let file_cfg: Config = toml::from_str(&text)
                    .with_context(|| format!("parsing config {}", path.display()))?;
                cfg = file_cfg;
            }
        } else if let Ok(path) = std::env::var("ATEM_PROXY_CONFIG") {
            let path = PathBuf::from(path);
            if path.exists() {
                let text = std::fs::read_to_string(&path)?;
                cfg = toml::from_str(&text)?;
            }
        }

        // Env overrides (between file and CLI).
        if let Ok(v) = std::env::var("ATEM_PROXY_ATEM") {
            cfg.atem = v;
        }
        if let Ok(v) = std::env::var("ATEM_PROXY_BIND") {
            cfg.bind = v.parse().context("ATEM_PROXY_BIND")?;
        }
        if let Ok(v) = std::env::var("ATEM_PROXY_MDNS") {
            cfg.mdns = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = std::env::var("ATEM_PROXY_LOG") {
            cfg.log = v;
        }

        // CLI overrides
        if let Some(v) = atem {
            cfg.atem = v;
        }
        if let Some(v) = bind {
            cfg.bind = v;
        }
        if let Some(v) = mdns {
            cfg.mdns = v;
        }
        if let Some(v) = log {
            cfg.log = v;
        }

        Ok(cfg)
    }

    pub fn atem_addr(&self) -> Result<SocketAddr> {
        if self.atem.contains(':') {
            Ok(self.atem.parse()?)
        } else {
            Ok(format!("{}:{}", self.atem, atem_protocol::ATEM_UDP_PORT).parse()?)
        }
    }

    #[cfg(windows)]
    pub fn default_windows_config_path() -> PathBuf {
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        base.join("AtemProxy").join("atem-proxy.toml")
    }
}

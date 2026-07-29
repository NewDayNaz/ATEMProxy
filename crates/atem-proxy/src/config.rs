use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    #[default]
    Deny,
    SingleOwner,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatConfig {
    /// SoftAtem just-works profile: enables single-owner locks, media, and mDNS.
    pub softatem: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocksConfig {
    pub mode: LockMode,
}

impl Default for LocksConfig {
    fn default() -> Self {
        Self {
            mode: LockMode::Deny,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    pub enabled: bool,
    pub chunk_delay_ms: u64,
    pub max_upload_mb: u64,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            chunk_delay_ms: 0,
            max_upload_mb: 64,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    pub mdns: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Upstream ATEM IP or host:port (default port 9910).
    pub atem: String,
    /// Local bind address for client-facing UDP.
    pub bind: SocketAddr,
    /// Legacy top-level mDNS flag (merged into discovery.mdns).
    #[serde(default)]
    pub mdns: bool,
    /// Idle client timeout in milliseconds.
    pub client_idle_ms: u64,
    /// Upstream reconnect base backoff in milliseconds.
    pub reconnect_ms: u64,
    /// Log filter (RUST_LOG compatible).
    pub log: String,
    /// Optional log file path (useful for Windows Service).
    pub log_file: Option<PathBuf>,
    pub compat: CompatConfig,
    pub locks: LocksConfig,
    pub media: MediaConfig,
    pub discovery: DiscoveryConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            atem: "192.168.1.50".to_string(),
            bind: "0.0.0.0:9910".parse().expect("static bind"),
            mdns: false,
            client_idle_ms: 5000,
            reconnect_ms: 1000,
            log: "info".to_string(),
            log_file: None,
            compat: CompatConfig::default(),
            locks: LocksConfig::default(),
            media: MediaConfig::default(),
            discovery: DiscoveryConfig::default(),
        }
    }
}

impl Config {
    /// Apply SoftAtem profile and legacy mdns flag after load.
    pub fn normalize(&mut self) {
        if self.mdns {
            self.discovery.mdns = true;
        }
        if self.compat.softatem {
            self.locks.mode = LockMode::SingleOwner;
            self.media.enabled = true;
            self.discovery.mdns = true;
        }
    }

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
        if let Ok(v) = std::env::var("ATEM_PROXY_SOFTATEM") {
            cfg.compat.softatem = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }

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

        cfg.normalize();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softatem_profile_enables_features() {
        let mut cfg = Config::default();
        cfg.compat.softatem = true;
        cfg.normalize();
        assert_eq!(cfg.locks.mode, LockMode::SingleOwner);
        assert!(cfg.media.enabled);
        assert!(cfg.discovery.mdns);
    }
}

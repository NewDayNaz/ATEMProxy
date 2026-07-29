//! Blackmagic-style mDNS announcement for SoftAtem / Companion discovery.

use crate::cache::StateCache;
use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const SERVICE_TYPE: &str = "_blackmagic._tcp.local.";

/// Spawn a task that registers mDNS once upstream product/version are known.
pub fn spawn_mdns_announcer(
    cache: Arc<StateCache>,
    port: u16,
    cancel: CancellationToken,
) -> Result<()> {
    tokio::spawn(async move {
        // Wait until upstream ready with product info (or timeout with defaults).
        let mut product = None;
        let mut version = None;
        for _ in 0..100 {
            if cancel.is_cancelled() {
                return;
            }
            if cache.is_ready() {
                product = cache.product();
                version = cache.version();
                if product.is_some() || version.is_some() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let product_name = product
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "ATEM Proxy".to_string());
        let release = version
            .map(|(maj, min)| format!("{maj}.{min}"))
            .unwrap_or_else(|| "0.0".to_string());

        if let Err(e) = register_service(&product_name, &release, port) {
            warn!(error = %e, "mDNS registration failed; SoftAtem can still connect by IP");
            return;
        }
        info!(%product_name, %release, port, "mDNS announced as Blackmagic AtemSwitcher");

        cancel.cancelled().await;
        // Daemon drops on process exit; explicit shutdown not required for v1.
    });
    Ok(())
}

fn register_service(product_name: &str, release_version: &str, port: u16) -> Result<()> {
    let mdns = ServiceDaemon::new().context("create mDNS daemon")?;
    let ip = local_ipv4().context("no local IPv4 for mDNS")?;
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "atem-proxy".into());
    let host_name = if host.ends_with(".local.") {
        host
    } else if host.ends_with(".local") {
        format!("{host}.")
    } else {
        format!("{host}.local.")
    };

    // SoftAtem/Companion expect class=AtemSwitcher on _blackmagic._tcp
    let mut props = HashMap::new();
    props.insert("txtvers".into(), "1".into());
    props.insert("name".into(), product_name.to_string());
    props.insert("class".into(), "AtemSwitcher".into());
    props.insert("protocol version".into(), "0.0".into());
    props.insert("release version".into(), release_version.to_string());
    props.insert("device version".into(), "0".into());
    props.insert("unique id".into(), format!("atem-proxy-{ip}"));
    props.insert("internal version".into(), "atem-proxy".into());

    let instance = sanitize_instance_name(product_name);
    let service = ServiceInfo::new(SERVICE_TYPE, &instance, &host_name, ip, port, Some(props))
        .context("build ServiceInfo")?;
    mdns.register(service).context("register mDNS service")?;
    // Keep daemon alive by leaking intentionally for process lifetime.
    std::mem::forget(mdns);
    Ok(())
}

fn sanitize_instance_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "ATEM-Proxy".into()
    } else {
        format!("{s} (Proxy)")
    }
}

fn local_ipv4() -> Option<IpAddr> {
    let addrs = if_addrs::get_if_addrs().ok()?;
    addrs
        .into_iter()
        .filter(|a| !a.is_loopback())
        .find_map(|a| match a.ip() {
            IpAddr::V4(v4) => Some(IpAddr::V4(v4)),
            _ => None,
        })
}

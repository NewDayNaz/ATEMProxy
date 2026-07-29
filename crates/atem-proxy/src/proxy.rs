use crate::cache::StateCache;
use crate::config::Config;
use crate::filter::{CommandFilter, FilterPolicy};
use crate::locks::LockBroker;
use crate::mdns::spawn_mdns_announcer;
use crate::server::run_server;
use crate::transfer::TransferLane;
use crate::upstream::spawn_upstream;
use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Shared entrypoint for console, Windows Service, and containers.
pub async fn run_proxy(config: Config, cancel: CancellationToken) -> Result<()> {
    let atem = config.atem_addr()?;
    info!(
        %atem,
        bind = %config.bind,
        softatem = config.compat.softatem,
        locks = ?config.locks.mode,
        media = config.media.enabled,
        mdns = config.discovery.mdns,
        "starting atem-proxy"
    );

    let cache = StateCache::new();
    let locks = LockBroker::new();
    let transfer = TransferLane::new(config.media.clone(), locks.clone());
    let filter = CommandFilter::new(FilterPolicy {
        lock_mode: config.locks.mode,
        locks: locks.clone(),
        transfer: transfer.clone(),
    });
    let upstream = spawn_upstream(
        atem,
        cache.clone(),
        locks,
        cancel.clone(),
        config.reconnect_ms,
    );

    if config.discovery.mdns {
        if let Err(e) = spawn_mdns_announcer(cache.clone(), config.bind.port(), cancel.clone()) {
            tracing::warn!(error = %e, "failed to start mDNS announcer");
        }
    }

    run_server(
        config.bind,
        cache,
        upstream,
        filter,
        transfer,
        config.client_idle_ms,
        cancel,
    )
    .await
}

use crate::cache::StateCache;
use crate::config::Config;
use crate::filter::CommandFilter;
use crate::server::run_server;
use crate::upstream::spawn_upstream;
use anyhow::Result;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Shared entrypoint for console, Windows Service, and containers.
pub async fn run_proxy(config: Config, cancel: CancellationToken) -> Result<()> {
    let atem = config.atem_addr()?;
    info!(%atem, bind = %config.bind, mdns = config.mdns, "starting atem-proxy");

    let cache = StateCache::new();
    let filter = CommandFilter::new();
    let upstream = spawn_upstream(atem, cache.clone(), cancel.clone(), config.reconnect_ms);

    if config.mdns {
        info!("mDNS announce requested (stub: enable once product string known from upstream)");
        // Full Bonjour announce can be added via mdns-sd; clients may connect by IP without it.
    }

    run_server(
        config.bind,
        cache,
        upstream,
        filter,
        config.client_idle_ms,
        cancel,
    )
    .await
}

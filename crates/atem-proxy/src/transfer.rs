//! Exclusive SoftAtem↔ATEM media transfer lane.

use crate::config::MediaConfig;
use crate::locks::LockBroker;
use atem_protocol::{is_transfer_command, lock_store_id, CommandName};
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferClientDecision {
    Forward,
    Drop,
}

#[derive(Debug)]
struct ActiveTransfer {
    owner: SocketAddr,
    store: Option<u16>,
    bytes_forwarded: u64,
    started: Option<Instant>,
}

#[derive(Debug)]
pub struct TransferLane {
    cfg: MediaConfig,
    locks: Arc<LockBroker>,
    active: Mutex<Option<ActiveTransfer>>,
}

impl TransferLane {
    pub fn new(cfg: MediaConfig, locks: Arc<LockBroker>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            locks,
            active: Mutex::new(None),
        })
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn chunk_delay_ms(&self) -> u64 {
        self.cfg.chunk_delay_ms
    }

    pub fn clear(&self) {
        *self.active.lock() = None;
    }

    /// Current transfer owner (receives upstream FT* fan-out).
    pub fn current_owner(&self) -> Option<SocketAddr> {
        self.active.lock().as_ref().map(|a| a.owner)
    }

    /// Decide if client may send a transfer command upstream.
    pub fn client_transfer(
        &self,
        from: SocketAddr,
        name: CommandName,
        body: &[u8],
    ) -> TransferClientDecision {
        if !self.cfg.enabled {
            return TransferClientDecision::Drop;
        }
        if !is_transfer_command(name) {
            return TransferClientDecision::Forward;
        }

        // Must own a lock (typically store 0 / 255).
        if !self.locks.is_owner(from) {
            debug!(%from, command = %name, "transfer from non-lock-owner; dropping");
            return TransferClientDecision::Drop;
        }

        let store = lock_store_id(body);
        let mut g = self.active.lock();
        match g.as_mut() {
            None => {
                *g = Some(ActiveTransfer {
                    owner: from,
                    store,
                    bytes_forwarded: 0,
                    started: Some(Instant::now()),
                });
                info!(%from, ?store, command = %name, "transfer lane opened");
                TransferClientDecision::Forward
            }
            Some(active) if active.owner == from => {
                if matches!(&name.0, b"FTSD" | b"FTSU") {
                    active.store = store.or(active.store);
                    active.started = Some(Instant::now());
                    active.bytes_forwarded = 0;
                }
                if matches!(&name.0, b"FTDa") {
                    active.bytes_forwarded =
                        active.bytes_forwarded.saturating_add(body.len() as u64);
                    let max = self.cfg.max_upload_mb.saturating_mul(1024 * 1024);
                    if max > 0 && active.bytes_forwarded > max {
                        warn!(
                            %from,
                            bytes = active.bytes_forwarded,
                            max,
                            "transfer exceeded max_upload_mb; still forwarding (soft cap)"
                        );
                    }
                }
                if matches!(&name.0, b"FTFD" | b"FTDC" | b"FTDE") {
                    info!(
                        %from,
                        bytes = active.bytes_forwarded,
                        command = %name,
                        "transfer lane completing"
                    );
                    // Keep owner until lock release; clear byte counters
                    if matches!(&name.0, b"FTDC" | b"FTDE") {
                        *g = None;
                    }
                }
                TransferClientDecision::Forward
            }
            Some(active) => {
                debug!(
                    %from,
                    owner = %active.owner,
                    command = %name,
                    "transfer busy with another owner; dropping"
                );
                TransferClientDecision::Drop
            }
        }
    }

    /// Owner that should receive upstream FT* traffic.
    pub fn fanout_owner(&self) -> Option<SocketAddr> {
        if !self.cfg.enabled {
            return None;
        }
        self.current_owner()
            .or_else(|| self.locks.owner_of(0))
            .or_else(|| self.locks.owner_of(255))
    }

    pub fn client_disconnected(&self, addr: SocketAddr) {
        let mut g = self.active.lock();
        if g.as_ref().is_some_and(|a| a.owner == addr) {
            info!(%addr, "clearing transfer lane after owner disconnect");
            *g = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locks::LockBroker;
    use atem_protocol::serialize_command;

    #[test]
    fn non_owner_cannot_transfer() {
        let locks = LockBroker::new();
        let lane = TransferLane::new(
            MediaConfig {
                enabled: true,
                ..Default::default()
            },
            locks,
        );
        let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let body = [0, 0, 0, 1];
        assert_eq!(
            lane.client_transfer(a, CommandName(*b"FTSD"), &body),
            TransferClientDecision::Drop
        );
    }

    #[test]
    fn owner_can_transfer() {
        let locks = LockBroker::new();
        let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let lock_body = [0, 0, 1, 0];
        assert_eq!(
            locks.client_lock_request(a, CommandName(*b"LOCK"), &lock_body),
            crate::locks::LockDecision::Forward
        );
        let lane = TransferLane::new(
            MediaConfig {
                enabled: true,
                ..Default::default()
            },
            locks,
        );
        assert_eq!(
            lane.client_transfer(a, CommandName(*b"FTSD"), &[0, 0]),
            TransferClientDecision::Forward
        );
        let _ = serialize_command;
    }
}

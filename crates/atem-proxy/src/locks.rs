//! Single-owner lock arbitration for SoftAtem media/macro stores.

use atem_protocol::{
    is_lock_request, is_lock_status, lock_request_enabled, lock_store_id, parse_commands,
    synthesize_unlock, CommandName,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockDecision {
    /// Forward framed command upstream.
    Forward,
    /// Drop (busy / denied).
    Drop,
}

#[derive(Debug, Default)]
struct StoreLock {
    owner: Option<SocketAddr>,
    /// Upstream reported locked.
    upstream_locked: bool,
}

#[derive(Debug, Default)]
pub struct LockBroker {
    inner: Mutex<HashMap<u16, StoreLock>>,
}

impl LockBroker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn clear(&self) {
        self.inner.lock().clear();
    }

    pub fn owner_of(&self, store: u16) -> Option<SocketAddr> {
        self.inner.lock().get(&store).and_then(|s| s.owner)
    }

    /// Any store currently owned by this client.
    pub fn stores_owned_by(&self, addr: SocketAddr) -> Vec<u16> {
        self.inner
            .lock()
            .iter()
            .filter_map(|(store, s)| (s.owner == Some(addr)).then_some(*store))
            .collect()
    }

    /// Decide whether a client lock request may be forwarded.
    pub fn client_lock_request(
        &self,
        from: SocketAddr,
        name: CommandName,
        body: &[u8],
    ) -> LockDecision {
        if !is_lock_request(name) {
            return LockDecision::Forward;
        }
        let Some(store) = lock_store_id(body) else {
            warn!(%from, command = %name, "lock request missing store id; dropping");
            return LockDecision::Drop;
        };
        let want_lock = lock_request_enabled(body);
        let mut g = self.inner.lock();
        let entry = g.entry(store).or_default();
        if want_lock {
            match entry.owner {
                None => {
                    entry.owner = Some(from);
                    info!(%from, store, "lock owner assigned (pending upstream)");
                    LockDecision::Forward
                }
                Some(owner) if owner == from => LockDecision::Forward,
                Some(owner) => {
                    debug!(%from, %owner, store, "lock busy; dropping request");
                    LockDecision::Drop
                }
            }
        } else {
            // Unlock
            if entry.owner == Some(from) || entry.owner.is_none() {
                entry.owner = None;
                info!(%from, store, "lock owner cleared (unlock)");
                LockDecision::Forward
            } else {
                debug!(%from, store, "unlock from non-owner; dropping");
                LockDecision::Drop
            }
        }
    }

    /// Ingest upstream lock status (broadcast to all clients).
    pub fn on_upstream_lock_status(&self, payload: &[u8]) {
        let Ok(cmds) = parse_commands(payload) else {
            return;
        };
        let mut g = self.inner.lock();
        for cmd in cmds {
            if !is_lock_status(cmd.name) {
                continue;
            }
            let Some(store) = lock_store_id(cmd.body) else {
                continue;
            };
            let locked = lock_request_enabled(cmd.body);
            let entry = g.entry(store).or_default();
            entry.upstream_locked = locked;
            if !locked {
                // Hardware released — clear owner so a new client can take it.
                if entry.owner.is_some() {
                    info!(store, "upstream unlocked; clearing proxy owner");
                }
                entry.owner = None;
            }
        }
    }

    /// Client disconnected: return unlock commands to forward upstream for stores it owned.
    pub fn client_disconnected(&self, addr: SocketAddr) -> Vec<Vec<u8>> {
        let mut unlocks = Vec::new();
        let mut g = self.inner.lock();
        for (store, entry) in g.iter_mut() {
            if entry.owner == Some(addr) {
                info!(%addr, store, "releasing lock after client disconnect");
                entry.owner = None;
                if entry.upstream_locked {
                    unlocks.push(synthesize_unlock(*store));
                }
            }
        }
        unlocks
    }

    /// True if addr currently owns any store (used by transfer lane).
    pub fn is_owner(&self, addr: SocketAddr) -> bool {
        self.inner.lock().values().any(|s| s.owner == Some(addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atem_protocol::serialize_command;

    fn lock_cmd(store: u16, enabled: bool) -> (CommandName, Vec<u8>) {
        let mut body = store.to_be_bytes().to_vec();
        body.push(u8::from(enabled));
        body.push(0);
        (CommandName(*b"LOCK"), body)
    }

    #[test]
    fn single_owner_exclusive() {
        let b = LockBroker::new();
        let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let c: SocketAddr = "127.0.0.1:2".parse().unwrap();
        let (name, body) = lock_cmd(0, true);
        assert_eq!(b.client_lock_request(a, name, &body), LockDecision::Forward);
        assert_eq!(b.client_lock_request(c, name, &body), LockDecision::Drop);
        let (uname, ubody) = lock_cmd(0, false);
        assert_eq!(
            b.client_lock_request(a, uname, &ubody),
            LockDecision::Forward
        );
        assert_eq!(b.client_lock_request(c, name, &body), LockDecision::Forward);
    }

    #[test]
    fn disconnect_emits_unlock_when_upstream_locked() {
        let b = LockBroker::new();
        let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (name, body) = lock_cmd(0, true);
        assert_eq!(b.client_lock_request(a, name, &body), LockDecision::Forward);
        // Simulate upstream LKST locked
        let lkst = serialize_command(CommandName(*b"LKST"), &[0, 0, 1, 0]);
        b.on_upstream_lock_status(&lkst);
        let unlocks = b.client_disconnected(a);
        assert_eq!(unlocks.len(), 1);
    }
}

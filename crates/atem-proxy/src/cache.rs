use atem_protocol::{
    command_identity, is_ephemeral_command, is_lock_command, is_transfer_command, parse_commands,
    parse_version, serialize_command, synthetic_init_complete, CommandName, INIT_COMPLETE,
};
use indexmap::IndexMap;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{debug, warn};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    name: CommandName,
    identity: Vec<u8>,
}

/// Coalescing ordered state cache for late-join init dumps.
#[derive(Debug, Default)]
pub struct StateCache {
    inner: RwLock<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Latest payload (full framed command) per key, insertion-order preserved for first see.
    map: IndexMap<CacheKey, Vec<u8>>,
    version: Option<(u16, u16)>,
    product: Option<String>,
    ready: bool,
}

impl StateCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn clear(&self) {
        let mut g = self.inner.write();
        g.map.clear();
        g.version = None;
        g.product = None;
        g.ready = false;
    }

    pub fn is_ready(&self) -> bool {
        self.inner.read().ready
    }

    pub fn set_ready(&self, ready: bool) {
        self.inner.write().ready = ready;
    }

    pub fn version(&self) -> Option<(u16, u16)> {
        self.inner.read().version
    }

    pub fn product(&self) -> Option<String> {
        self.inner.read().product.clone()
    }

    pub fn len(&self) -> usize {
        self.inner.read().map.len()
    }

    /// Ingest upstream packet payload commands into the cache.
    pub fn ingest_payload(&self, payload: &[u8]) {
        let cmds = match parse_commands(payload) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "failed to parse upstream commands");
                return;
            }
        };
        let mut g = self.inner.write();
        for cmd in cmds {
            if is_ephemeral_command(cmd.name)
                || is_lock_command(cmd.name)
                || is_transfer_command(cmd.name)
            {
                continue;
            }
            // Never store historical InCm — we synthesize at dump time.
            if cmd.name == INIT_COMPLETE {
                g.ready = true;
                continue;
            }
            if cmd.name.0 == *b"_ver" {
                g.version = parse_version(cmd.body);
            }
            if cmd.name.0 == *b"_pin" {
                if let Ok(s) = std::str::from_utf8(cmd.body) {
                    g.product = Some(s.trim_end_matches('\0').to_string());
                }
            }
            let key = CacheKey {
                name: cmd.name,
                identity: command_identity(cmd.name, cmd.body),
            };
            // IndexMap: entry API updates value in place without changing order.
            if let Some(slot) = g.map.get_mut(&key) {
                *slot = cmd.raw.to_vec();
            } else {
                g.map.insert(key, cmd.raw.to_vec());
            }
        }
    }

    /// Late-join dump: all current state blobs + trailing synthetic InCm.
    pub fn dump(&self) -> Vec<Vec<u8>> {
        let g = self.inner.read();
        let mut out: Vec<Vec<u8>> = g.map.values().cloned().collect();
        out.push(synthetic_init_complete());
        debug!(commands = out.len(), "built late-join state dump");
        out
    }

    /// Apply a parsed command as if from upstream (testing helper).
    pub fn upsert_raw(&self, raw: Vec<u8>) {
        self.ingest_payload(&raw);
        // If caller passed a bare command without using ingest path for InCm:
        if let Ok(cmds) = parse_commands(&raw) {
            for cmd in cmds {
                if cmd.name == INIT_COMPLETE {
                    self.set_ready(true);
                }
            }
        }
    }
}

/// Helper to build a framed command for tests.
pub fn framed(name: [u8; 4], body: &[u8]) -> Vec<u8> {
    serialize_command(CommandName(name), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_same_key_and_ends_with_incm() {
        let cache = StateCache::new();
        cache.ingest_payload(&framed(*b"PrgI", &[0, 0, 0, 1]));
        cache.ingest_payload(&framed(*b"PrgI", &[0, 0, 0, 2]));
        cache.ingest_payload(&framed(*b"InCm", &[]));
        assert_eq!(cache.len(), 1);
        let dump = cache.dump();
        assert_eq!(dump.len(), 2);
        let last = parse_commands(dump.last().unwrap()).unwrap();
        assert_eq!(last[0].name, INIT_COMPLETE);
        let first = parse_commands(&dump[0]).unwrap();
        assert_eq!(first[0].body, &[0, 0, 0, 2]);
    }

    #[test]
    fn unknown_commands_do_not_grow_unbounded() {
        let cache = StateCache::new();
        for i in 0..100u8 {
            // Same name + same body → one entry
            cache.ingest_payload(&framed(*b"ZzZz", &[1, 2, 3, i % 2]));
        }
        // Two distinct bodies → at most 2 entries
        assert!(cache.len() <= 2);
    }
}

use atem_protocol::{
    is_audio_levels_subscribe, is_lock_command, is_transfer_command, parse_commands, CommandName,
};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterAction {
    /// Forward framed command bytes upstream.
    Forward(Vec<u8>),
    /// Dropped (lock/transfer/etc).
    Dropped {
        name: CommandName,
        reason: &'static str,
    },
    /// Audio subscription changed; optionally forward enable/disable command.
    AudioSubscribe {
        enable: bool,
        forward: Option<Vec<u8>>,
    },
}

#[derive(Debug, Default)]
struct AudioSubs {
    clients: HashSet<SocketAddr>,
    /// Last enable command forwarded upstream (used to synthesize disable on last leave).
    last_enable: Option<Vec<u8>>,
}

/// Fan-in filter: drop lock/transfer, edge-trigger audio levels, forward the rest.
#[derive(Debug, Default)]
pub struct CommandFilter {
    audio: Mutex<AudioSubs>,
    warned: Mutex<HashSet<(SocketAddr, [u8; 4])>>,
}

impl CommandFilter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn audio_subscriber_count(&self) -> usize {
        self.audio.lock().clients.len()
    }

    /// On client disconnect: if that client was the last audio subscriber, returns a disable
    /// command to forward upstream.
    pub fn client_disconnected(&self, addr: SocketAddr) -> Option<Vec<u8>> {
        let mut g = self.audio.lock();
        if !g.clients.remove(&addr) {
            return None;
        }
        if g.clients.is_empty() {
            if let Some(enable_cmd) = g.last_enable.clone() {
                return Some(synthesize_audio_disable(&enable_cmd));
            }
        }
        None
    }

    pub fn process_payload(&self, from: SocketAddr, payload: &[u8]) -> Vec<FilterAction> {
        let cmds = match parse_commands(payload) {
            Ok(c) => c,
            Err(_) => {
                return vec![FilterAction::Forward(payload.to_vec())];
            }
        };
        let mut actions = Vec::new();
        for cmd in cmds {
            if is_lock_command(cmd.name) {
                self.warn_once(from, cmd.name, "lock");
                actions.push(FilterAction::Dropped {
                    name: cmd.name,
                    reason: "lock commands not supported through proxy; connect directly for media",
                });
                continue;
            }
            if is_transfer_command(cmd.name) {
                self.warn_once(from, cmd.name, "transfer");
                actions.push(FilterAction::Dropped {
                    name: cmd.name,
                    reason:
                        "data transfer not supported through proxy; upload media directly to ATEM",
                });
                continue;
            }
            if is_audio_levels_subscribe(cmd.name) {
                let enable = parse_enable_flag(cmd.body).unwrap_or(true);
                let edge = {
                    let mut g = self.audio.lock();
                    let before = g.clients.len();
                    if enable {
                        g.clients.insert(from);
                        g.last_enable = Some(cmd.raw.to_vec());
                    } else {
                        g.clients.remove(&from);
                    }
                    let after = g.clients.len();
                    if before == 0 && after > 0 {
                        Some(true)
                    } else if before > 0 && after == 0 {
                        Some(false)
                    } else {
                        None
                    }
                };
                let forward = edge.map(|en| {
                    if en {
                        cmd.raw.to_vec()
                    } else {
                        synthesize_audio_disable(cmd.raw)
                    }
                });
                actions.push(FilterAction::AudioSubscribe {
                    enable: edge.unwrap_or(enable),
                    forward,
                });
                continue;
            }
            actions.push(FilterAction::Forward(cmd.raw.to_vec()));
        }
        actions
    }

    fn warn_once(&self, from: SocketAddr, name: CommandName, kind: &str) {
        let mut g = self.warned.lock();
        if g.insert((from, name.0)) {
            info!(%from, command = %name, kind, "dropping unsupported command from client");
        } else {
            debug!(%from, command = %name, kind, "dropping unsupported command");
        }
    }
}

fn parse_enable_flag(body: &[u8]) -> Option<bool> {
    body.last().map(|b| *b != 0)
}

fn synthesize_audio_disable(enable_cmd: &[u8]) -> Vec<u8> {
    let mut out = enable_cmd.to_vec();
    if let Some(last) = out.last_mut() {
        *last = 0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use atem_protocol::serialize_command;

    #[test]
    fn drops_lock_and_forwards_cut() {
        let f = CommandFilter::new();
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut payload = serialize_command(CommandName(*b"LOCK"), &[0, 0]);
        payload.extend(serialize_command(CommandName(*b"DCut"), &[0, 0]));
        let actions = f.process_payload(addr, &payload);
        assert!(matches!(actions[0], FilterAction::Dropped { .. }));
        assert!(matches!(actions[1], FilterAction::Forward(_)));
    }

    #[test]
    fn last_audio_client_disconnect_returns_disable() {
        let f = CommandFilter::new();
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let sub = serialize_command(CommandName(*b"SALN"), &[1]);
        let actions = f.process_payload(addr, &sub);
        assert!(matches!(
            &actions[0],
            FilterAction::AudioSubscribe {
                enable: true,
                forward: Some(_)
            }
        ));
        let disable = f.client_disconnected(addr).expect("disable cmd");
        assert_eq!(*disable.last().unwrap(), 0);
        assert!(f.client_disconnected(addr).is_none());
    }
}

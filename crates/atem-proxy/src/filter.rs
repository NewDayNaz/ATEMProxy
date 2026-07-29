use crate::config::LockMode;
use crate::locks::{LockBroker, LockDecision};
use crate::transfer::{TransferClientDecision, TransferLane};
use atem_protocol::{
    is_audio_levels_subscribe, is_lock_request, is_transfer_command, parse_commands, CommandName,
};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterAction {
    Forward(Vec<u8>),
    Dropped {
        name: CommandName,
        reason: &'static str,
    },
    AudioSubscribe {
        enable: bool,
        forward: Option<Vec<u8>>,
    },
}

#[derive(Debug, Default)]
struct AudioSubs {
    clients: HashSet<SocketAddr>,
    last_enable: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct FilterPolicy {
    pub lock_mode: LockMode,
    pub locks: Arc<LockBroker>,
    pub transfer: Arc<TransferLane>,
}

/// Fan-in filter: policy-gated lock/transfer, audio subscribe edge, forward the rest.
pub struct CommandFilter {
    audio: Mutex<AudioSubs>,
    warned: Mutex<HashSet<(SocketAddr, [u8; 4])>>,
    policy: FilterPolicy,
}

impl CommandFilter {
    pub fn new(policy: FilterPolicy) -> Arc<Self> {
        Arc::new(Self {
            audio: Mutex::new(AudioSubs::default()),
            warned: Mutex::new(HashSet::new()),
            policy,
        })
    }

    pub fn policy(&self) -> &FilterPolicy {
        &self.policy
    }

    pub fn audio_subscriber_count(&self) -> usize {
        self.audio.lock().clients.len()
    }

    /// Commands to forward upstream on client disconnect (audio disable + lock unlocks).
    pub fn client_disconnected(&self, addr: SocketAddr) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut g = self.audio.lock();
            if g.clients.remove(&addr) && g.clients.is_empty() {
                if let Some(enable_cmd) = g.last_enable.clone() {
                    out.push(synthesize_audio_disable(&enable_cmd));
                }
            }
        }
        self.policy.transfer.client_disconnected(addr);
        out.extend(self.policy.locks.client_disconnected(addr));
        out
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
            if is_lock_request(cmd.name) {
                match self.policy.lock_mode {
                    LockMode::Deny => {
                        self.warn_once(from, cmd.name, "lock");
                        actions.push(FilterAction::Dropped {
                            name: cmd.name,
                            reason: "lock commands denied (enable compat.softatem or locks.mode=single_owner)",
                        });
                    }
                    LockMode::SingleOwner => {
                        match self
                            .policy
                            .locks
                            .client_lock_request(from, cmd.name, cmd.body)
                        {
                            LockDecision::Forward => {
                                actions.push(FilterAction::Forward(cmd.raw.to_vec()));
                            }
                            LockDecision::Drop => {
                                actions.push(FilterAction::Dropped {
                                    name: cmd.name,
                                    reason: "lock store busy or not owned by this client",
                                });
                            }
                        }
                    }
                }
                continue;
            }
            if is_transfer_command(cmd.name) {
                if !self.policy.transfer.enabled() {
                    self.warn_once(from, cmd.name, "transfer");
                    actions.push(FilterAction::Dropped {
                        name: cmd.name,
                        reason: "media transfer disabled (enable compat.softatem or media.enabled)",
                    });
                } else {
                    match self
                        .policy
                        .transfer
                        .client_transfer(from, cmd.name, cmd.body)
                    {
                        TransferClientDecision::Forward => {
                            actions.push(FilterAction::Forward(cmd.raw.to_vec()));
                        }
                        TransferClientDecision::Drop => {
                            actions.push(FilterAction::Dropped {
                                name: cmd.name,
                                reason:
                                    "transfer not allowed for this client (need lock ownership)",
                            });
                        }
                    }
                }
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
    use crate::config::MediaConfig;
    use crate::locks::LockBroker;
    use crate::transfer::TransferLane;
    use atem_protocol::serialize_command;

    fn deny_filter() -> Arc<CommandFilter> {
        let locks = LockBroker::new();
        let transfer = TransferLane::new(MediaConfig::default(), locks.clone());
        CommandFilter::new(FilterPolicy {
            lock_mode: LockMode::Deny,
            locks,
            transfer,
        })
    }

    #[test]
    fn drops_lock_and_forwards_cut_when_denied() {
        let f = deny_filter();
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mut payload = serialize_command(CommandName(*b"LOCK"), &[0, 0, 1, 0]);
        payload.extend(serialize_command(CommandName(*b"DCut"), &[0, 0]));
        let actions = f.process_payload(addr, &payload);
        assert!(matches!(actions[0], FilterAction::Dropped { .. }));
        assert!(matches!(actions[1], FilterAction::Forward(_)));
    }

    #[test]
    fn softatem_allows_lock_for_owner() {
        let locks = LockBroker::new();
        let transfer = TransferLane::new(
            MediaConfig {
                enabled: true,
                ..Default::default()
            },
            locks.clone(),
        );
        let f = CommandFilter::new(FilterPolicy {
            lock_mode: LockMode::SingleOwner,
            locks,
            transfer,
        });
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let payload = serialize_command(CommandName(*b"LOCK"), &[0, 0, 1, 0]);
        let actions = f.process_payload(addr, &payload);
        assert!(matches!(actions[0], FilterAction::Forward(_)));
    }

    #[test]
    fn last_audio_client_disconnect_returns_disable() {
        let f = deny_filter();
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
        let cmds = f.client_disconnected(addr);
        assert_eq!(*cmds[0].last().unwrap(), 0);
    }
}

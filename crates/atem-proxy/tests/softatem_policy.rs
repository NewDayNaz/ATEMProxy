//! SoftAtem policy: lock exclusivity + transfer lane + deny-by-default.

use atem_protocol::{serialize_command, CommandName};
use atem_proxy::config::{LockMode, MediaConfig};
use atem_proxy::filter::{CommandFilter, FilterAction, FilterPolicy};
use atem_proxy::locks::LockBroker;
use atem_proxy::transfer::TransferLane;
use std::net::SocketAddr;

fn softatem_filter() -> (std::sync::Arc<CommandFilter>, std::sync::Arc<LockBroker>) {
    let locks = LockBroker::new();
    let transfer = TransferLane::new(
        MediaConfig {
            enabled: true,
            ..Default::default()
        },
        locks.clone(),
    );
    let filter = CommandFilter::new(FilterPolicy {
        lock_mode: LockMode::SingleOwner,
        locks: locks.clone(),
        transfer,
    });
    (filter, locks)
}

#[test]
fn softatem_second_client_denied_media_lock() {
    let (f, _) = softatem_filter();
    let a: SocketAddr = "127.0.0.1:1001".parse().unwrap();
    let b: SocketAddr = "127.0.0.1:1002".parse().unwrap();
    let lock = serialize_command(CommandName(*b"LOCK"), &[0, 0, 1, 0]);
    assert!(matches!(
        f.process_payload(a, &lock)[0],
        FilterAction::Forward(_)
    ));
    assert!(matches!(
        f.process_payload(b, &lock)[0],
        FilterAction::Dropped { .. }
    ));
}

#[test]
fn softatem_owner_can_send_ftsd() {
    let (f, _) = softatem_filter();
    let a: SocketAddr = "127.0.0.1:1001".parse().unwrap();
    let lock = serialize_command(CommandName(*b"LOCK"), &[0, 0, 1, 0]);
    let ftsd = serialize_command(CommandName(*b"FTSD"), &[0, 0, 0, 1]);
    assert!(matches!(
        f.process_payload(a, &lock)[0],
        FilterAction::Forward(_)
    ));
    assert!(matches!(
        f.process_payload(a, &ftsd)[0],
        FilterAction::Forward(_)
    ));
}

#[test]
fn deny_mode_still_blocks_lock() {
    let locks = LockBroker::new();
    let transfer = TransferLane::new(MediaConfig::default(), locks.clone());
    let f = CommandFilter::new(FilterPolicy {
        lock_mode: LockMode::Deny,
        locks,
        transfer,
    });
    let a: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let lock = serialize_command(CommandName(*b"LOCK"), &[0, 0, 1, 0]);
    assert!(matches!(
        f.process_payload(a, &lock)[0],
        FilterAction::Dropped { .. }
    ));
}

#[test]
fn disconnect_releases_lock_with_unlock_cmd() {
    let (f, locks) = softatem_filter();
    let a: SocketAddr = "127.0.0.1:1001".parse().unwrap();
    let lock = serialize_command(CommandName(*b"LOCK"), &[0, 0, 1, 0]);
    let _ = f.process_payload(a, &lock);
    // Mark upstream locked so disconnect emits unlock
    let lkst = serialize_command(CommandName(*b"LKST"), &[0, 0, 1, 0]);
    locks.on_upstream_lock_status(&lkst);
    let cmds = f.client_disconnected(a);
    assert!(
        cmds.iter().any(|c| c.windows(4).any(|w| w == b"LOCK")),
        "expected unlock LOCK command"
    );
}

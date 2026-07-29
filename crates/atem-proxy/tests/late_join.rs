//! Integration-style tests for state cache late-join behavior (no real ATEM required).

use atem_protocol::{parse_commands, INIT_COMPLETE};
use atem_proxy::cache::{framed, StateCache};

#[test]
fn late_join_dump_bounded_after_many_updates() {
    let cache = StateCache::new();
    // Simulate hours of program changes on ME0 + unknown opcode updates
    for i in 0..10_000u32 {
        let src = (i % 20) as u8;
        cache.ingest_payload(&framed(*b"PrgI", &[0, 0, 0, src]));
        cache.ingest_payload(&framed(*b"ZzZz", &[1, 2, (i % 3) as u8]));
    }
    // Only a handful of coalesced entries, not 10k+
    assert!(cache.len() < 20, "cache grew unbounded: {}", cache.len());

    let dump = cache.dump();
    let last = parse_commands(dump.last().expect("dump")).unwrap();
    assert_eq!(last[0].name, INIT_COMPLETE);

    // Second joiner sees same bounded dump
    let dump2 = cache.dump();
    assert_eq!(dump.len(), dump2.len());
}

#[test]
fn multi_me_program_keys_are_distinct() {
    let cache = StateCache::new();
    cache.ingest_payload(&framed(*b"PrgI", &[0, 0, 0, 1]));
    cache.ingest_payload(&framed(*b"PrgI", &[1, 0, 0, 2]));
    assert_eq!(cache.len(), 2);
    let dump = cache.dump();
    // 2 state cmds + InCm
    assert_eq!(dump.len(), 3);
}

#[test]
fn stress_many_command_names_still_coalesce() {
    let cache = StateCache::new();
    for slot in 0..10u8 {
        let name = [b'A', b'B', b'C', b'0' + slot];
        for v in 0..100u8 {
            // Changing trailing byte but same identity prefix for unknowns ≤64 uses full body —
            // use identical body so each name is a single cache entry.
            let _ = v;
            cache.ingest_payload(&framed(name, &[1, 2, 3, slot]));
        }
    }
    assert_eq!(cache.len(), 10);
    let dump = cache.dump();
    let last = parse_commands(dump.last().unwrap()).unwrap();
    assert_eq!(last[0].name, INIT_COMPLETE);
}

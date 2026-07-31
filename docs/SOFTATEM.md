# SoftAtem (ATEM Software Control) through the proxy

## Enable

```toml
[compat]
softatem = true
```

Or:

```bash
atem-proxy --atem 192.168.1.50 --softatem
```

This turns on:

- `locks.mode = single_owner` — one client owns media/macro store locks
- `media.enabled = true` — forward FT* between SoftAtem and the ATEM for that owner
- `discovery.mdns = true` — Bonjour `_blackmagic._tcp` as `AtemSwitcher`

## How to connect SoftAtem

1. Start the proxy with SoftAtem mode and a reachable ATEM.
2. In ATEM Software Control, enter the **proxy PC’s LAN IP** (port 9910), **or** pick the device named like `… (Proxy)` from Bonjour discovery.
3. Use SoftAtem normally for switching and media pool uploads/downloads.

## Concurrent clients

| Client | Role |
|--------|------|
| SoftAtem (media) | Owns store locks; only one should upload at a time |
| Companion / tally | Control + state; do not compete for media locks |
| Second SoftAtem | Control-only is OK; media lock will be denied if busy |

### Companion stuck on “Connecting” / commands do nothing

- Empty 8-byte synthetic `InCm` is ignored by Companion’s `atem-connection` parser — use the LibAtem-shaped 12-byte `InCm` (`01 00 00 00`).
- If logs show a reconnect storm (`client handshake` every ~1s, growing dump sizes, `client timed out`) and buttons do nothing while SoftAtem→ATEM changes still appear in Companion, the proxy was expecting client reliable packet id `0`. Companion sends from `1`; a retransmit ask for `0` forces reconnect. Fixed by setting `expected_recv = 1` after HELLO.
- Point Companion at the **proxy LAN IP** (port `9910`), not the real ATEM.

## Soak checklist (hardware)

- [ ] SoftAtem connects by IP
- [ ] SoftAtem appears via mDNS / Bonjour
- [ ] Cut / Auto / preview work
- [ ] Audio meters subscribe
- [ ] Media pool slots/thumbnails populate
- [ ] Upload a still; assign to media player
- [ ] Download / capture path works
- [ ] Quit SoftAtem mid-upload — ATEM unlocks within a few seconds; reconnect works
- [ ] Companion still switches while SoftAtem is connected

## Safe mode

With `compat.softatem = false` (default), lock and media commands are denied — same as the original v1 proxy behavior.

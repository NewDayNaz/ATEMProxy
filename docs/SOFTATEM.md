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

### Companion stuck on “Connecting”

If proxy logs show `client handshake` + `queued init dump` but Companion never goes OK, rebuild with the LibAtem-shaped synthetic `InCm` (12-byte body `01 00 00 00`). Empty 8-byte `InCm` is ignored by Companion’s `atem-connection` parser.

Also ensure Companion’s host field is the **proxy LAN IP** (port `9910`), not the real ATEM — Companion does not use SoftAtem’s Bonjour picker.

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

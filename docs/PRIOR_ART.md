# Prior art lessons

## LibAtem/AtemProxy

Closest blueprint: one upstream LibAtem client, UDP `:9910` server, command cache for late joiners.

Issues we intentionally fix:

| Issue | Their behavior | Ours |
|-------|----------------|------|
| Late join after long uptime | Unknown cmds stored under unique incrementing keys → dump grows forever | Coalesce by name + identity; never autoincrement |
| `InCm` placement | Can sit mid-history in replay | Always append synthetic `InCm` at end of dump |
| Session ids | Hardcoded `0x8008` for every client | Per-client `0x8000 \| n` |
| Media / locks | Silent drop | Drop with structured logs |
| Threading | Per-client busy send loops | Tokio tasks + shared ticker |
| Reconnect | Dump flags not always reset | Fresh `ClientSession` per handshake |

Recommendation from upstream README still stands: keep critical SoftAtem / media-upload clients on a **direct** ATEM connection slot.

## OpenSwitcher proxy

Speaks custom TCP/HTTP/MQTT to clients — **not** transparent to SoftAtem/Companion. Useful for mixerstate cache ideas and media-transfer danger notes (RLE chunk splits can brick hardware until power cycle). We do not emulate their TCP frontend in v1.

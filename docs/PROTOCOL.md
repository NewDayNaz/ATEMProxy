# ATEM UDP protocol notes (proxy-relevant)

Port: **UDP 9910**.

## Header (12 bytes)

| Field | Notes |
|-------|--------|
| flags (5 bits) + length (11 bits) | Big-endian u16 |
| session id | Temporary during HELLO; ATEM assigns real session afterward (often `0x8000 \| n`) |
| ack id | Last received reliable packet id |
| retransmit request | Used for NACK-style resend asks |
| unknown | Misc / init quirk |
| packet id | 15-bit space |

Flags (LibAtem-style): `AckRequest=1`, `Handshake=2`, `IsRetransmit=4`, `RetransmitRequest=8`, `AckReply=16`.

## Handshake

1. Client sends HELLO (`Handshake` flag) with status `0x01`.
2. Switcher/proxy replies HELLO with status `0x02` (OK) or `0x04` (restart).
3. Client ACKs the HELLO `packet_id` (usually `0`); the init dump’s first reliable packet is the **next** id (usually `1`), not a second `0`.
4. Client→switcher reliable packets also start at id **`1`** (atem-connection / LibAtem). Expecting `0` makes the proxy NACK forever and Companion reconnect-loop.
5. Subsequent packets carry the assigned session id.

## Reliability

- Reliable packets set `AckRequest` and advance local packet ids (except pure ACK/HELLO).
- Peers must ACK or the sender retransmits (we retransmit the in-flight window after ~30ms).
- Idle connections die in ~1–5s without traffic; send empty ping `AckRequest`s while open.
- After init complete, clients often ACK with remote-seq field `0x61` (OpenSwitcher).

## Commands

Payload is a sequence of framed commands:

`u16 length | u16 reserved | 4-byte name | body`

Init dump ends with `InCm`. This proxy **synthesizes** `InCm` (LibAtem-shaped 12-byte command with body `01 00 00 00`) at the end of late-join dumps and never stores historical `InCm` blobs mid-cache. An empty 8-byte `InCm` is skipped by Companion/`atem-connection` (`while (buffer.length > 8)`).

## Proxy gotchas

1. **Do not byte-forward** upstream UDP packets to clients (wrong session/seq).
2. **Coalesce state** by `(command name, identity)` — never unbounded append of unknowns.
3. **Session ids** must be per-client and consistent from HELLO through data (`0x8000 | n` in the handshake reply — do not echo a temp client id then flip).
4. **Media lock/transfer** can hard-lock switchers if mishandled; v1 drops these — upload directly to the ATEM.
5. **Audio levels** are high-rate; subscribe upstream only while ≥1 client wants them.

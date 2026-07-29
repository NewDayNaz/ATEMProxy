# ATEM Proxy

Transparent multi-client **UDP proxy** for Blackmagic ATEM switchers.

ATEM hardware only allows a handful of simultaneous clients (often ~5). This proxy opens **one** connection to the switcher and speaks the native ATEM protocol on `:9910` so Companion, tally apps, and ATEM Software Control can share that slot.

```
[Companion] ──┐
[Tally]     ──┼──► atem-proxy :9910 ──► ATEM :9910
[SoftAtem]  ──┘         (one upstream session)
```

## Quick start

```bash
cargo build --release
./target/release/atem-proxy --atem 192.168.1.50 --bind 0.0.0.0:9910
```

### SoftAtem (media + locks + Bonjour)

```bash
./target/release/atem-proxy --atem 192.168.1.50 --softatem
```

Then point **ATEM Software Control** at this host’s IP (or pick the Bonjour entry). Details: [`docs/SOFTATEM.md`](docs/SOFTATEM.md).

### Config

TOML example: [`deploy/atem-proxy.toml.example`](deploy/atem-proxy.toml.example)

Env vars: `ATEM_PROXY_ATEM`, `ATEM_PROXY_BIND`, `ATEM_PROXY_MDNS`, `ATEM_PROXY_SOFTATEM`, `ATEM_PROXY_LOG`, `ATEM_PROXY_CONFIG`

Precedence: CLI > env > file > defaults.

## Deploy

| Platform | Path |
|----------|------|
| **Windows Service** (preferred on church PCs) | [`docs/DEPLOY.md`](docs/DEPLOY.md) · `deploy/windows/install-service.ps1` |
| Linux systemd | `deploy/atem-proxy.service` |
| Docker (Linux + host network) | `docker compose up -d` |

## Modes

| Mode | Locks / media | Use when |
|------|----------------|----------|
| Default (`compat.softatem = false`) | Denied | Companion/tally only; SoftAtem media stays direct |
| SoftAtem profile (`--softatem`) | Single-owner locks + FT* lane + mDNS | SoftAtem should “just work” through the proxy |

## Architecture

- `crates/atem-protocol` — header, handshake, command framing, reliable UDP helpers
- `crates/atem-proxy` — upstream client, state cache, lock broker, transfer lane, mDNS, CLI/service

Late joiners get a **coalesced** state dump ending in a synthetic `InCm`.

More: [`docs/PROTOCOL.md`](docs/PROTOCOL.md) · [`docs/DEPLOY.md`](docs/DEPLOY.md) · [`docs/PRIOR_ART.md`](docs/PRIOR_ART.md)

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p atem-proxy -- --atem 192.168.1.50 --softatem
```

## License

MIT

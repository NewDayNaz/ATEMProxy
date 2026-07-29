# ATEM Proxy

Transparent multi-client **UDP proxy** for Blackmagic ATEM switchers.

ATEM hardware only allows a handful of simultaneous clients (often ~5). This proxy opens **one** connection to the switcher and speaks the native ATEM protocol on `:9910` so Companion, tally apps, and secondary SoftAtem panels can all connect to the proxy instead.

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

Point clients at the proxy host IP. Keep media-upload SoftAtem on a **direct** ATEM IP (see below).

### Config

TOML example: [`deploy/atem-proxy.toml.example`](deploy/atem-proxy.toml.example)

Env vars: `ATEM_PROXY_ATEM`, `ATEM_PROXY_BIND`, `ATEM_PROXY_MDNS`, `ATEM_PROXY_LOG`, `ATEM_PROXY_CONFIG`

Precedence: CLI > env > file > defaults.

## Deploy

| Platform | Path |
|----------|------|
| **Windows Service** (preferred on church PCs) | [`docs/DEPLOY.md`](docs/DEPLOY.md) · `deploy/windows/install-service.ps1` |
| Linux systemd | `deploy/atem-proxy.service` |
| Docker (Linux + host network) | `docker compose up -d` |

## What is blocked (by design in v1)

- Media pool upload/download (`FT*` / data transfer)
- Lock arbitration (`LOCK` / `PLCK` / …)

These are dropped with logs. Upload graphics **directly** to the ATEM. See [`docs/PRIOR_ART.md`](docs/PRIOR_ART.md) for why (OpenSwitcher notes RLE mistakes can hard-lock a mixer).

## Architecture

- `crates/atem-protocol` — header, handshake, command framing, reliable UDP helpers
- `crates/atem-proxy` — upstream client, coalescing state cache, client server, filter, CLI/service

Late joiners get a **coalesced** state dump ending in a synthetic `InCm` (fixes LibAtem AtemProxy’s unbounded unknown-command growth).

More: [`docs/PROTOCOL.md`](docs/PROTOCOL.md) · [`docs/DEPLOY.md`](docs/DEPLOY.md)

## Development

```bash
cargo test --workspace
cargo run -p atem-proxy -- --atem 192.168.1.50
```

## License

MIT

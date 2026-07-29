# Deploying atem-proxy

Same binary for console, Windows Service, systemd, and Docker.

Config precedence: **CLI > env (`ATEM_PROXY_*`) > TOML file > defaults**.

## SoftAtem (official ATEM Software Control)

To point SoftAtem at the proxy and have media/locks work:

```toml
[compat]
softatem = true
```

Or `atem-proxy --atem <ATEM_IP> --softatem`.

Full notes and soak checklist: [`SOFTATEM.md`](SOFTATEM.md).

## Windows Service (recommended on a church Windows PC)

1. Build: `cargo build --release`
2. Edit/copy `deploy/atem-proxy.toml.example` or let the installer write `%ProgramData%\AtemProxy\atem-proxy.toml`
3. For SoftAtem-through-proxy, set `[compat] softatem = true` in that TOML.
4. Elevated PowerShell:

```powershell
.\deploy\windows\install-service.ps1 -Atem 192.168.1.50
```

Or manually:

```powershell
.\target\release\atem-proxy.exe service install --config $env:ProgramData\AtemProxy\atem-proxy.toml
.\target\release\atem-proxy.exe service start
```

5. Allow inbound **UDP 9910** (installer tries to add a firewall rule).
6. Point Companion / tally / SoftAtem at the PC’s LAN IP.

Service name: `AtemProxy`. Stop/uninstall:

```powershell
.\target\release\atem-proxy.exe service stop
.\target\release\atem-proxy.exe service uninstall
```

## Linux (systemd)

```bash
cargo build --release
sudo cp target/release/atem-proxy /usr/local/bin/
sudo useradd --system --no-create-home atem-proxy || true
sudo mkdir -p /etc/atem-proxy
sudo cp deploy/atem-proxy.toml.example /etc/atem-proxy/atem-proxy.toml
# edit atem=... and optionally compat.softatem = true
sudo cp deploy/atem-proxy.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now atem-proxy
```

`SIGTERM` / `systemctl stop` cancels the Tokio runtime and drops clients cleanly.

## Docker (Linux host)

**Use host networking.** Bridge NAT breaks UDP session affinity and mDNS.

```bash
export ATEM_PROXY_ATEM=192.168.1.50
export ATEM_PROXY_SOFTATEM=true
docker compose up -d --build
```

On **Windows Docker Desktop**, host networking is not equivalent — prefer the native Windows Service for production on that machine; run the container on a Linux NUC/Pi instead.

## Mission-critical panel

Even with SoftAtem mode, if you still have a free direct ATEM slot, you may keep one critical panel on the real ATEM IP for maximum resilience. Everything else can use the proxy.

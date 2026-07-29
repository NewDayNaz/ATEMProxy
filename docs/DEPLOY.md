# Deploying atem-proxy

Same binary for console, Windows Service, systemd, and Docker.

Config precedence: **CLI > env (`ATEM_PROXY_*`) > TOML file > defaults**.

## Windows Service (recommended on a church Windows PC)

1. Build: `cargo build --release`
2. Edit/copy `deploy/atem-proxy.toml.example` or let the installer write `%ProgramData%\AtemProxy\atem-proxy.toml`
3. Elevated PowerShell:

```powershell
.\deploy\windows\install-service.ps1 -Atem 192.168.1.50
```

Or manually:

```powershell
.\target\release\atem-proxy.exe service install --config $env:ProgramData\AtemProxy\atem-proxy.toml
.\target\release\atem-proxy.exe service start
```

4. Allow inbound **UDP 9910** (installer tries to add a firewall rule).
5. Point Companion / tally / secondary SoftAtem at the PC’s LAN IP. Keep media-upload SoftAtem pointed at the real ATEM.

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
# edit atem=...
sudo cp deploy/atem-proxy.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now atem-proxy
```

`SIGTERM` / `systemctl stop` cancels the Tokio runtime and drops clients cleanly.

## Docker (Linux host)

**Use host networking.** Bridge NAT breaks UDP session affinity and mDNS.

```bash
export ATEM_PROXY_ATEM=192.168.1.50
docker compose up -d --build
```

On **Windows Docker Desktop**, host networking is not equivalent — prefer the native Windows Service for production on that machine; run the container on a Linux NUC/Pi instead.

## What not to route through the proxy

- Large media pool uploads / downloads
- Anything that needs ATEM lock ownership
- Mission-critical single panel if you still have a free direct slot — put that one on the ATEM, everything else on the proxy

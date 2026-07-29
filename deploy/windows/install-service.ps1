#Requires -RunAsAdministrator
param(
    [string]$ExePath = "",
    [string]$ConfigPath = "$env:ProgramData\AtemProxy\atem-proxy.toml",
    [string]$Atem = "192.168.1.50"
)

$ErrorActionPreference = "Stop"

if (-not $ExePath) {
    $ExePath = Join-Path (Split-Path -Parent $PSScriptRoot) "..\..\target\release\atem-proxy.exe"
    $ExePath = [System.IO.Path]::GetFullPath($ExePath)
}

if (-not (Test-Path $ExePath)) {
    Write-Error "atem-proxy.exe not found at $ExePath. Build with: cargo build --release"
}

$configDir = Split-Path -Parent $ConfigPath
New-Item -ItemType Directory -Force -Path $configDir | Out-Null

if (-not (Test-Path $ConfigPath)) {
    @"
atem = "$Atem"
bind = "0.0.0.0:9910"
client_idle_ms = 5000
reconnect_ms = 1000
log = "info"

[compat]
softatem = true

[locks]
mode = "deny"

[media]
enabled = false
chunk_delay_ms = 0
max_upload_mb = 64

[discovery]
mdns = false
"@ | Set-Content -Path $ConfigPath -Encoding UTF8
    Write-Host "Wrote $ConfigPath (compat.softatem=true for SoftAtem-through-proxy)"
}

& $ExePath service install --config $ConfigPath
& $ExePath service start

# Allow inbound UDP 9910 for SoftAtem / Companion / tally clients
try {
    New-NetFirewallRule -DisplayName "ATEM Proxy UDP 9910" -Direction Inbound -Protocol UDP -LocalPort 9910 -Action Allow -ErrorAction SilentlyContinue | Out-Null
} catch {}

Write-Host "AtemProxy service installed and started. Point clients at this machine's IP."

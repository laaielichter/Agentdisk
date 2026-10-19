# Builds agentdisk from this folder and installs it for the current user.
# Usage (PowerShell, in the repo folder):  .\scripts\install.ps1
# Needs Rust (https://rustup.rs). Installs to %USERPROFILE%\.cargo\bin, which rustup puts on PATH.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
cargo install --locked --path crates/agentdisk-cli
if ($LASTEXITCODE -ne 0) { exit 1 }
agentdisk --version
Write-Host "Installed. Try: agentdisk scan" -ForegroundColor Green

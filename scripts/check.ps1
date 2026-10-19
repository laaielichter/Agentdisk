# Runs the same checks as CI on your own Windows machine.
# Usage (PowerShell, in the repo folder):  .\scripts\check.ps1
# Needs Rust (https://rustup.rs) and git.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

function Step($name, $cmd) {
    Write-Host "`n== $name ==" -ForegroundColor Cyan
    & $cmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $name" -ForegroundColor Red
        exit 1
    }
}

Step "Format"  { cargo fmt --all --check }
Step "Lint"    { cargo clippy --all-targets -- -D warnings }
Step "Tests"   { cargo test --all --locked }
Step "Build"   { cargo build --release --locked -p agentdisk }
Step "Smoke"   {
    .\target\release\agentdisk.exe --version
    if ($LASTEXITCODE -ne 0) { return }
    .\target\release\agentdisk.exe adapters
    if ($LASTEXITCODE -ne 0) { return }
    .\target\release\agentdisk.exe size .
}

Write-Host "`nAll checks passed." -ForegroundColor Green
Write-Host "Binary: target\release\agentdisk.exe"

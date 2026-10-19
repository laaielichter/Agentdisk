#!/usr/bin/env sh
# Runs the same checks as CI on macOS or Linux.  Usage: ./scripts/check.sh
set -eu
cd "$(dirname "$0")/.."
echo "== Format ==";  cargo fmt --all --check
echo "== Lint ==";    cargo clippy --all-targets -- -D warnings
echo "== Tests ==";   cargo test --all --locked
echo "== Build ==";   cargo build --release --locked -p agentdisk
echo "== Smoke ==";   ./target/release/agentdisk --version && ./target/release/agentdisk adapters >/dev/null && ./target/release/agentdisk size . >/dev/null
echo "All checks passed. Binary: target/release/agentdisk"

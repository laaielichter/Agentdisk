# Contributing

Thanks for helping. The most useful contributions:

1. **Adapters for more agents** — see [docs/adapters.md](docs/adapters.md). Include where you verified the paths (OS, agent version).
2. **Fixes to existing adapter paths** — agents move their data often.
3. **Bug reports with `agentdisk scan --json` output** (remove personal paths first).

## Development

```sh
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

The project uses no external crates on purpose: one small, auditable binary.

## Rules that must not change

- Scanning never writes anything.
- Cleanup is plan → quarantine → purge, and apply/purge require a person at a terminal.
- The MCP server never gets tools that move or delete files.
- When in doubt, classify data as `user-data`.

# agentdisk — notes for AI agents

agentdisk reports disk space used by AI coding agents and dev projects, and makes reversible cleanup plans.

## Use it

- `agentdisk check` — fast: free space and number of agent worktrees. Run before big work.
- `agentdisk scan -p <projects-folder> [--json]` — full read-only report.
- `agentdisk size <dir> [--json]` — one folder; "freed if deleted" counts hardlinks correctly.
- `agentdisk advise -p <dir>` — ways to need less space (pnpm/uv, duplicates, caches, agent settings).
- `agentdisk plan -p <dir>` — saves a cleanup plan. Moves nothing.
- MCP: `agentdisk mcp` (stdio) offers the same read-only tools plus `agentdisk_plan`.

## Rules

- Never run `agentdisk apply`, `purge` or `compact`. They are for the user, in their own terminal, and refuse non-interactive use.
- Show the user the `agentdisk apply <id>` command from the plan output instead.
- Never delete agent transcripts, checkpoints, memories or settings by hand.

## Working on this repo

- Rust workspace, no external crates. `cargo test --all`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check` must pass.
- Agent knowledge lives in `crates/agentdisk-core/adapters/*.toml`. To support a new agent, add a file there and to the `BUILTIN` list in `adapters.rs`.
- Scanning must never write. Cleanup must stay plan → quarantine → purge.

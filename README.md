# agentdisk

**The honest disk report for AI coding agents.**

AI coding agents (Claude Code, Cursor, Codex, Copilot, Gemini CLI, Cline and more)
leave worktrees, checkpoints, transcripts and duplicate `node_modules`, `.venv`,
`target/` and `.godot/` folders all over your disk. `agentdisk` shows what each
agent left behind, what is really reclaimable (shared bytes counted once), and
later gives you a safe, reversible way to get the space back.

> **Status: early (v0.0.1).** It does nothing yet except print its version.
> The first useful release, `agentdisk scan`, is read-only.

## Principles

- **Read-only first.** Scanning never changes your disk.
- **Correct numbers.** Hardlinked and cloned bytes are counted once.
- **Nothing is deleted without a plan you approve.** An AI agent can never approve its own deletion.
- **Never touched:** `.git`, uncommitted work, agent memories, MCP configs, credentials.
- **Works with every agent.** Agent knowledge lives in small YAML adapter files anyone can add.

## Roadmap

| Version | What |
|---|---|
| 0.0.1 | Project skeleton, CI on Windows, macOS and Linux |
| 0.1 | `agentdisk scan` (read-only), Claude Code + Cursor, Windows first |
| 0.2 | More agents, `--json`, ecosystem folders, duplicate report |
| 0.3 | MCP server + agent plugins (read-only) |
| 0.4 | Safe `plan` / `apply` with quarantine and undo |
| 0.5 | Dedupe advice (pnpm, uv, Dev Drive, reflinks) |
| 0.6 | Optional Windows compaction for read-mostly folders |

## Build

```sh
cargo build --release
./target/release/agentdisk --version
```

## License

Licensed under either of [MIT](LICENSE) or [Apache-2.0](LICENSE-APACHE), at your option.

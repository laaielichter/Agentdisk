# agentdisk

**The honest disk report for AI coding agents.**

Claude Code, Cursor, Codex, Copilot, Gemini CLI, Cline and friends leave worktrees, checkpoints,
transcripts and duplicate `node_modules`, `.venv`, `target/` and `.godot/` folders all over your disk.
`agentdisk` shows what each agent left behind, what is *really* reclaimable (shared bytes counted once),
and gives you a cleanup that you can undo.

- **Read-only by default.** `scan`, `size`, `advise` and `check` never change anything.
- **Correct numbers.** Hardlinked files (pnpm, uv) are counted once. "Freed if deleted" only counts bytes that actually come back.
- **Reversible cleanup.** `plan` → `apply` moves items to quarantine → `restore` or `purge`.
- **A person decides.** `apply`, `purge` and `compact` refuse to run without a human at a terminal. The MCP server has no tool to delete anything.
- **Every agent.** 15 agents built in; add more with a small TOML file. Windows-first, also macOS and Linux.
- **One small binary.** Rust, no external crates.

```text
$ agentdisk scan -p ~/Projects        # example output, shortened

WORKTREES (3)
  Agent        Repo  Branch   Idle   On disk  Reclaimable  Status
  claude-code  app   agent-b  12d   1.93 GiB     1.93 GiB  1 changed
  claude-code  app   agent-a  15d   1.93 GiB     1.93 GiB  clean
  cursor       game  feat-ui  21d   4.10 GiB     4.10 GiB  clean

DEPENDENCY & BUILD FOLDERS (4)
  Folder        Type    In        Idle  On disk  Reclaimable
  .godot        godot   project   30d  6.20 GiB     6.20 GiB
  node_modules  node    worktree  12d  1.91 GiB     1.91 GiB
  ...
```

## Install

**Windows, macOS, Linux:** download the archive for your system from
[Releases](https://github.com/laaielichter/agentdisk/releases), unpack it, and put `agentdisk`
(`agentdisk.exe` on Windows) in a folder on your `PATH`.

**With Rust installed:**

```sh
cargo install --git https://github.com/laaielichter/agentdisk agentdisk
```

Check it works: `agentdisk --version`

## Use

```sh
agentdisk scan                    # agent data and agent worktrees
agentdisk scan -p ~/Projects      # also your repositories and build folders
agentdisk advise -p ~/Projects    # ways to need less space
agentdisk plan -p ~/Projects      # a cleanup plan (moves nothing)
agentdisk apply <plan-id>         # you type a code; items move to quarantine
agentdisk restore <plan-id>       # undo
agentdisk purge                   # delete quarantined items older than 7 days
agentdisk size <folder>           # measure one folder
agentdisk check                   # quick warning when space is low (for hooks)
agentdisk adapters                # which agents are known and found here
```

Save your projects folder once so you can drop `-p`:

```sh
agentdisk config --init           # then edit `projects = [...]` in the file it prints
```

Every read command has `--json` for scripts and agents.

## What is safe to clean

| Kind | Planned for cleanup when… | Never when… |
|---|---|---|
| Agent worktrees | clean, idle 7+ days, unlocked, commits on a branch | uncommitted or untracked files, ignored files that are not build output (like `.env`), locked by a running agent, main worktree |
| Build and dependency folders (`node_modules`, `.venv`, `target`, `.godot`, Unity `Library`, Unreal `Intermediate`, …) | marker file present (`package.json`, `Cargo.toml`, `project.godot`, …), git-ignored, idle 14+ days | tracked by git, editor running (Godot, Unity, Unreal), all files shared via hardlinks |
| Agent logs and caches | marked `regenerable`, agent not running | — |
| Transcripts, checkpoints, memories, settings, models | — | **always reported, never planned** |

Branches are always kept. Items move to a quarantine folder on the same drive, so `apply` is instant
and `restore` puts everything back. Nothing is deleted until you run `purge`.

## Why not just compress?

Compression gives about 1.3–2x on code and build output and almost nothing on `.git`, images,
audio and video. The big waste is **duplication**: every agent worktree gets its own copy of the
same dependencies. `agentdisk advise` points at shared stores (pnpm, uv), copy-on-write clones and
the agents' own limits. For folders that are mostly read (SDKs, engine installs, old projects),
`agentdisk compact <folder>` applies Windows WOF compression.

## Use it from your agent

`agentdisk mcp` runs an MCP server on stdio with read-only tools plus `agentdisk_plan`.

**Claude Code plugin** (MCP server, skill, and a disk check at session start):

```
/plugin marketplace add laaielichter/agentdisk
/plugin install agentdisk@agentdisk
```

Cursor, Codex, Copilot, Gemini CLI, Cline, Windsurf and others: see [docs/clients.md](docs/clients.md).

## Supported agents

Claude Code · Cursor · OpenAI Codex · GitHub Copilot · Gemini CLI · Cline · Roo Code · Windsurf ·
Kiro · OpenCode · Amp · Aider · Zed · pi · Ollama models

Locations marked "undocumented" come from user reports and are reported only. Agents move their
data often; `agentdisk adapters` shows what was found on your machine. Missing an agent or a
path? [Add an adapter](docs/adapters.md) — it is a small TOML file.

## Known limits

- Copy-on-write clones (ReFS block cloning, APFS clones, Btrfs reflinks) are still counted as separate space.
- "In use" detection: Windows refuses to move open folders; Linux checks open files; macOS relies on idle time and editor detection.
- Several agent storage paths are undocumented and may change between versions.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Adapters for more agents are the most welcome help.

## License

Licensed under either of [MIT](LICENSE) or [Apache-2.0](LICENSE-APACHE), at your option.

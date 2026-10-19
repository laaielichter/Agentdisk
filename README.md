<h1 align="center">agentdisk</h1>
<p align="center"><b>The honest disk report for AI coding agents: what Claude Code, Cursor, Codex and friends left on your disk, and a cleanup you can undo.</b></p>
<p align="center">
  <a href="LICENSE"><img alt="License: MIT or Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%7C%20Apache--2.0-blue"></a>
  <img alt="Platforms: Windows, macOS, Linux" src="https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-informational">
  <img alt="MCP server" src="https://img.shields.io/badge/MCP-server-8A2BE2">
</p>
<p align="center">
  <img src="docs/demo.gif" alt="agentdisk scanning agent worktrees and build folders, making a cleanup plan, and explaining why a worktree with a .env file is kept" width="900">
  <br><sub>Real output on a test machine with sample projects.</sub>
</p>

agentdisk is a single-binary CLI and **MCP server** that finds the git worktrees, checkpoints,
transcripts and duplicate `node_modules`, `.venv`, `target/` and `.godot/` folders that AI coding
agents leave behind. It shows what is *really* reclaimable and cleans up only what is safe, with undo.

## Quickstart

```sh
agentdisk scan -p ~/Projects
```

You get three tables (agent and tool data, worktrees, dependency and build folders), a summary of
what is reclaimable, and what each agent already cleans up by itself. Nothing on disk changes.

Then, if you want space back:

```sh
agentdisk plan -p ~/Projects     # a plan; moves nothing
agentdisk apply <plan-id>        # you type a code; items move to quarantine
agentdisk restore <plan-id>      # changed your mind? everything goes back
```

## Why agentdisk?

- **See the real cost of parallel agents.** Every agent worktree gets its own `node_modules`, `.venv`, `target/` or `.godot/`. agentdisk puts them in one table, per agent.
- **Trust the numbers.** Hardlinked files (pnpm, uv) are counted once, and "reclaimable" only counts bytes that actually come back.
- **Clean up without losing work.** Worktrees with uncommitted files, a `.env`, a lock from a running agent, or commits on no branch are never touched. Transcripts and memories are never planned.
- **Undo anything.** Cleanup moves items to quarantine first; deletion is a separate, later step.
- **Keep the human in charge.** Only a person at a terminal can apply a plan. AI agents can scan, explain and plan, nothing more.

## Install

Prebuilt binaries will be on the Releases page. Until then, build from source with
[Rust](https://rustup.rs) 1.82 or newer:

```sh
git clone https://github.com/laaielichter/agentdisk
cd agentdisk
cargo install --locked --path crates/agentdisk-cli
```

On Windows you can run `.\scripts\install.ps1` instead. Check it works: `agentdisk --version`

## Commands

| Command | What it does | Changes disk? |
|---|---|---|
| `scan [-p dir]` | Agent and tool data, worktrees, build folders | No |
| `explain <dir>` | What a folder is, who made it, may it be cleaned, and why | No |
| `size <dir>` | Size, size on disk, freed if deleted | No |
| `advise [-p dir]` | Shared stores (pnpm, uv), duplicates, caches, Docker/WSL disks, agent settings | No |
| `check` | Fast warning when space is low or worktrees pile up (for hooks) | No |
| `adapters` | Which agents and tools are known, and found here | No |
| `plan [-p dir]` | Saves a cleanup plan | Writes a plan file |
| `apply <id>` | Moves plan items to quarantine (asks you for a code) | Moves files |
| `restore <id>` | Puts them back | Moves files |
| `purge` | Deletes quarantined items older than 7 days (asks you) | Deletes |
| `compact <dir>` | Windows WOF compression for read-mostly folders | Compresses |
| `mcp` | MCP server on stdio | No |

Every read command has `--json`. Save your projects folder once with `agentdisk config --init`.

## What is safe to clean

| Kind | Planned when… | Never when… |
|---|---|---|
| Agent worktrees | clean, idle 7+ days, unlocked, commits on a branch | uncommitted or untracked files, ignored files that are not build output (like `.env`), locked, main worktree |
| Build folders (`node_modules`, `.venv`, `target`, `.godot`, Unity `Library`, Unreal `Intermediate`, …) | marker file present, git-ignored, idle 14+ days | tracked by git, its editor is running (Godot, Unity, Unreal), all files shared via hardlinks |
| Agent logs and caches | marked regenerable, agent not running | — |
| Transcripts, checkpoints, memories, settings, models, SDK downloads | — | **always reported, never planned** |

Branches are always kept. Quarantine sits on the same drive, so `apply` is instant.

## Use it from your agent (MCP server)

`agentdisk mcp` offers read-only tools (`agentdisk_scan`, `agentdisk_explain`, `agentdisk_size`,
`agentdisk_advise`, `agentdisk_check`, `agentdisk_quarantine`) plus `agentdisk_plan`. There is no
tool to move or delete files.

**Claude Code plugin** (MCP server + skill + a disk check when a session starts):

```
/plugin marketplace add laaielichter/agentdisk
/plugin install agentdisk@agentdisk
```

**Cursor, Codex, GitHub Copilot, Gemini CLI, Cline, Windsurf, Kiro, Zed, OpenCode:** see
[docs/clients.md](docs/clients.md).

## Supported agents and tools

Claude Code · Cursor · OpenAI Codex · GitHub Copilot · Gemini CLI · Cline · Roo Code · Windsurf ·
Kiro · OpenCode · Amp · Aider · Zed · pi · Ollama · LM Studio · Hugging Face cache · Godot editor

Each one is a small TOML file. Missing one, or a path changed? [Add an adapter](docs/adapters.md).

## How it compares

| | agentdisk | Disk analyzers (WizTree, dua, WinDirStat) | Build cleaners (kondo, npkill) | Agents' own cleanup |
|---|---|---|---|---|
| Knows which agent made a folder | Yes, 18 agents and tools | No | No | Only its own |
| Knows what is safe to delete | Yes, with git checks | No | Build folders only | Its own data |
| Counts hardlinked bytes once | Yes | Some do | No | — |
| Undo | Quarantine + restore | No | No | Varies |
| Usable by AI agents | MCP server (read-only + plan) | No | No | — |
| What it does not do | No GUI; does not count copy-on-write clones as shared yet | | | |

## Why not just compress?

Compression gives about 1.3–2x on code and build output and almost nothing on `.git`, images,
audio and video. The big waste is duplication. `agentdisk advise` points at shared stores,
copy-on-write clones and the agents' own limits; `agentdisk compact` handles read-mostly folders on Windows.

## FAQ

**Will it delete my work?** Not by itself. Scans never write. A plan moves nothing. `apply` needs you
to type a code and only moves items to quarantine. Only `purge` deletes, and it asks too.

**Can my AI agent clean up for me?** It can make a plan and show you the command. Applying it is yours.

**Does it send data anywhere?** No. It runs locally and has no network code.

**Why is something "undocumented"?** Some agents do not document where they store data. Those paths
come from user reports and are reported only.

## Known limits

- Copy-on-write clones (ReFS block cloning, APFS clones, Btrfs reflinks) still count as separate space.
- "In use" detection: Windows refuses to move open folders; Linux checks open files; macOS relies on idle time and editor detection.
- Agent storage paths change between versions; `agentdisk adapters` shows what was found on your machine.

## Contributing

Adapters for more agents are the most welcome help. See [CONTRIBUTING.md](CONTRIBUTING.md) and
[docs/adapters.md](docs/adapters.md). Changes per version: [CHANGELOG.md](CHANGELOG.md).

## License

Licensed under either of [MIT](LICENSE) or [Apache-2.0](LICENSE-APACHE), at your option.

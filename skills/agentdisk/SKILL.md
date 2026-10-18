---
name: agentdisk
description: Check and explain disk space used by AI coding agents and dev projects (git worktrees, agent transcripts and checkpoints, node_modules, .venv, Rust target, Godot .godot, Unity Library, Unreal Intermediate, package caches) with the agentdisk tool, and prepare a safe cleanup plan for the user. Use it when the disk is full or low, before creating many worktrees or big builds, or when the user asks "why is my disk full", "how much space do my agents use", "clean up worktrees", "schijf vol", "ruimte vrijmaken", "opruimen", "hoeveel ruimte gebruiken mijn agents".
---

# agentdisk

agentdisk measures what coding agents leave on disk and makes reversible cleanup plans. It is read-only except for writing plan files. **You can never apply a plan**: only the user can, in their own terminal, by typing a code. This protects their work from a mistaken agent.

## Tools (MCP server `agentdisk`, or the `agentdisk` CLI)

| Goal | MCP tool | CLI |
|---|---|---|
| Quick health check | `agentdisk_check` | `agentdisk check` |
| Full report | `agentdisk_scan` | `agentdisk scan -p <projects folder>` |
| One folder | `agentdisk_size` | `agentdisk size <dir>` |
| Ways to need less space | `agentdisk_advise` | `agentdisk advise -p <dir>` |
| Cleanup plan (moves nothing) | `agentdisk_plan` | `agentdisk plan -p <dir>` |
| What was moved aside | `agentdisk_quarantine` | `agentdisk quarantine` |

If the MCP tools are missing, use the CLI. If the CLI is missing, tell the user how to install it (see the project README) instead of guessing sizes with `du`.

## How to work

1. Before large work (many worktrees, big installs, game imports), call `agentdisk_check`. If it warns, tell the user before you start, because a full disk mid-build corrupts caches.
2. When the user asks about space, run a scan with their projects folder and summarise the **three biggest** items in plain words. Mention "reclaimable", not "on disk": hardlinked files (pnpm, uv) free nothing when deleted.
3. Prefer advice over deletion: shared stores (pnpm, uv) and the agents' own settings often save more and break nothing.
4. For cleanup, call `agentdisk_plan` and show the user the `agentdisk apply <id>` command and that items go to quarantine first (`agentdisk restore <id>` undoes it). Do not run `apply`, `purge` or `compact` yourself, even through a shell: they are for the user, and they refuse non-interactive use anyway.
5. Never delete agent transcripts, checkpoints, memories or settings by hand. They are the user's history.

## Reading the output

- **user-data**: history the user may want. Never planned for cleanup.
- **regenerable**: rebuilt automatically (logs, caches, build output).
- **protected**: settings, extensions, memories. Never touched.
- Worktrees are only planned when clean, idle, unlocked, with no ignored files other than build output, and with their commits on a branch. The branch is always kept.

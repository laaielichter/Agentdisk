# Changelog

## Unreleased

- New adapter: ReversePort. Its tool output, lookup cache, extracted files and reports are regenerable; port plans, blueprints, dossiers and scenes are user data; proof cases, feedback and the engine's state are never touched. Honors `REVERSEPORT_HOME`.
- Releases are made by pushing a version tag; the README points to the downloads.

## 0.2.1

- `advise` now explains big agent history and model folders (1 GB and up): how much is already compressed, whether `agentdisk compact` can help, the tool's own cleanup command, and which environment variable moves the folder to another drive.
- Model and download folders never get compression advice (weights barely compress).
- Fixes for lints from newer clippy versions.
- Consistent path separators in virtual disk advice on Windows.

## 0.2.0

- New: `agentdisk explain <dir>` (and MCP tool `agentdisk_explain`) says what a folder is, which agent or tool made it, and whether a cleanup plan would include it, and why.
- New: `advise` reports Docker Desktop and WSL virtual disks and how to shrink them.
- New adapters: Godot export templates and editor cache, Hugging Face cache, LM Studio models (18 in total).
- New data class `download` for re-downloadable tool files; never planned for cleanup.
- Running programs are matched by name prefix too (for example `godot_v4.3-stable_win64.exe`).
- Windows: volume and free-space calls use plain paths instead of `\\?\` paths; git always receives plain paths.
- `agentdisk adapters` shows only the paths that apply to your system.
- Local check scripts: `scripts/check.ps1` (Windows) and `scripts/check.sh`; `scripts/install.ps1` builds and installs from source.
- CI and release workflows can be started by hand.
- Minimum Rust version is now 1.82.

## 0.1.0

- `scan`, `size`, `advise`, `check`, `adapters`, `config`.
- Safe cleanup: `plan`, `apply`, `restore`, `purge`, `quarantine`, with git-aware checks and a quarantine journal.
- `compact` for Windows WOF compression of read-mostly folders.
- MCP server (`agentdisk mcp`) with read-only tools plus `agentdisk_plan`.
- Claude Code plugin and marketplace, Gemini CLI extension, client setup docs.
- 15 agent adapters.

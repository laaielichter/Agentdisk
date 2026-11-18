# Changelog

## Unreleased

- Config and adapter TOML files now ignore one leading UTF-8 byte order mark so their first key loads correctly. Thanks to @dannyward630! (#25, #39)
- Adapter files are rejected when a path has a `platforms` value other than `windows`, `macos` or `linux`, or has `kind = "worktree"` without a `marker`. Before, such a path was silently never found, or its worktrees were shown as `git`.
- New: `agentdisk plans [--json]` lists the saved cleanup plans, newest first, with what each would free and whether it is empty, not applied, applied, restored or purged. A plan file that cannot be read is listed as unreadable. Read-only. Thanks to @ariktadas144! (#31, #37)
- `size` and `explain` with several folders: `--json` now prints one JSON array in the order given instead of one object per line, and a folder that cannot be read is reported (`{"path", "error"}` in JSON, stderr in text) without stopping the others. The exit code is 1 if any folder failed. One folder behaves as before. Thanks to @ariktadas144! (#30, #41)
- `purge --older-than` and `check --min-free-gb` no longer overflow on huge numbers: a release build used to wrap `--older-than 144115188075855872` to 0 days and list items moved today for permanent deletion, and a debug build panicked. Both now saturate, so a huge age lists nothing and a huge free-space threshold always warns. Thanks to @ViktorBystrov72! (#45, #54)
- MCP: `projects` that is not a list of strings, and `worktree_days` / `build_days` that are not whole numbers of 0 or more, are tool errors (`isError: true`) that name the argument, instead of being dropped for the defaults. Missing arguments still use the defaults. Thanks to @ViktorBystrov72! (#46, #55)
- `purge --plan <id>` names the plan and exits 1 when nothing is in its quarantine, like `restore`. When the plan's items are too young, it says how many wait and how to delete them now (`--older-than 0`). Unscoped `purge` prints what it did before. Thanks to @ViktorBystrov72! (#44, #60)
- A closed output pipe, such as `agentdisk adapters | head -1`, now stops quietly with exit status 0 instead of panicking. Other write errors are still reported as `error: ...` with exit 2. Thanks to @ViktorBystrov72! (#48, #61)
- `scan`, `plan` and the other scanning commands now warn `project path is not a folder: <path>` when a `-p` / `projects` entry is a file instead of a folder; before, it gave no warning at all. Missing folders still give `project folder not found`. Thanks to @ViktorBystrov72! (#42, #62)
- `scan`, `advise`, `plan`, `purge`, `check`, `plans`, `adapters` and `config` refuse a loose argument they do not take with exit 2, before anything is scanned or written: `scan demo` used to scan no projects, `plan demo` saved an empty plan and `purge <id>` listed every plan. `scan`, `advise` and `plan` suggest `-p <folder>`, `purge` suggests `--plan <id>`. Thanks to @ViktorBystrov72! (#43, #63)
- `plan` keeps the scan's warnings: a missing or unreadable project folder no longer turns into an apparently clean empty plan. The text says "Nothing to clean, but the scan had warnings." and lists them, and JSON plans have a `warnings` array (`schema_version` stays 1; older saved plans still load). Thanks to @ViktorBystrov72! (#27, #64)
- `config.toml` warns instead of silently using the default when a known setting has a wrong value (`worktree_days = -3`, `build_days = "soon"`, `projects = 5`), when a folder list has entries that are not text, and when settings sit under a `[section]` header. A single string for `projects` or `adapters` is still read as one folder. Thanks to @ViktorBystrov72! (#29, #65)
- Dart and Flutter `.dart_tool` folders next to `pubspec.yaml` are recognized as build folders of ecosystem `dart` (rebuild: `dart pub get / flutter pub get`) and planned under the usual rules. Thanks to @ViktorBystrov72! (#28, #69)
- `check` uses the adapter folders, `min_free_gb` and `max_worktrees` from `config.toml`, so a configured setting is no longer ignored by the fast CLI and MCP checks. An explicit flag still wins, and both limits still default to 20. Thanks to @ViktorBystrov72! (#12, #70)
- The scan report's summary totals, its JSON summary fields and the 40-row build-folder limit are now covered by tests. Thanks to @dannyward630! (#8, #72)
- Every build-folder rule and marker is documented in one reference, and a test keeps that page in step with the code. Thanks to @dannyward630! (#6, #74)
- Test coverage for `advise`, `compact`, `config --init` and `quarantine` through the binary: the text and JSON output, refusals, dry runs and empty or populated quarantines are checked. Test-only. Thanks to @dannyward630! (#47, #76)
- The README has a Spanish translation next to the English one, with a language switcher at the top of the English README; commands, flags and code blocks are unchanged. Thanks to @dannyward630! (#34, #77)
- `explain` agrees with `plan` on agent caches: a regenerable cache nested inside another adapter folder is reported as `keep` and named after the outer folder, and so is a cache with no reclaimable bytes. Thanks to @ViktorBystrov72! (#13, #78)
- `adapters --help`, `config --help` and `mcp --help` now print their usage and exit 0 instead of running the command (`mcp --help` used to start the stdio server); `mcp` rejects an unknown option with exit 2, and `advise --help` lists `--depth`, `--threads`, `--adapters` and `--no-agents`. Thanks to @Golden007-prog! (#9, #59)

## 0.3.1

### Security

- `apply` checks agent caches again right before moving them, as it already did for worktrees and build folders. A cache is skipped when its agent is running, when a program uses the folder, or when it is no longer a plain folder that its adapter lists as regenerable. Before, these checks ran only when the plan was made.
- `restore` and `purge` accept only real plan ids, and only touch entries inside that plan's quarantine folder. Any other path in a quarantine journal is left alone and reported. The `purge` confirmation now lists the quarantine paths it will delete.

## 0.3.0

- New adapter: ReversePort. Its tool output, lookup cache, extracted files and reports are regenerable; port plans, blueprints, dossiers and scenes are user data; proof cases, feedback and the engine's state are never touched. Honors `REVERSEPORT_HOME`.
- Releases with binaries for Windows, macOS and Linux are made automatically when a new version in `Cargo.toml` reaches `main` (or when a version tag is pushed); the README points to the downloads.

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

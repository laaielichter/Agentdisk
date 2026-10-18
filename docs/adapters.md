# Writing an adapter

An adapter is a small TOML file that tells agentdisk where one agent keeps data and what
deleting it would cost. Adapters contain data only, never code.

Put your file in `<config dir>/adapters/` (see `agentdisk config`) to try it, then open a
pull request adding it to `crates/agentdisk-core/adapters/` and to `BUILTIN` in `adapters.rs`.
A file with the same `id` as a built-in one replaces it.

```toml
id = "my-agent"                 # lowercase, digits, '-'
name = "My Agent"
vendor = "Example Inc."
docs = ["https://example.com/docs/storage"]
policy = "What the agent cleans up by itself."
cleanup = "my-agent sessions prune"   # the vendor's own cleanup command, if any
processes = ["my-agent", "my-agent.exe"]
verified = "2026-10 on Windows 11"

[[path]]
id = "sessions"
path = "{home}/.my-agent/sessions"
class = "user-data"             # regenerable | user-data | protected | model
documented = true               # false = found by users, not in official docs
note = "Optional text shown in reports."

[[path]]
id = "worktrees"
path = "{home}/.my-agent/worktrees/*"
kind = "worktree"               # each match is a git worktree
class = "user-data"
marker = "/.my-agent/worktrees/" # identifies this agent's worktrees by path
documented = true
```

## Placeholders

| Placeholder | Windows | macOS | Linux |
|---|---|---|---|
| `{home}` | `%USERPROFILE%` | `~` | `~` |
| `{appdata}` | `%APPDATA%` | `~/Library/Application Support` | `$XDG_CONFIG_HOME` |
| `{localappdata}` | `%LOCALAPPDATA%` | `~/Library/Application Support` | `$XDG_DATA_HOME` |
| `{xdg_data}` | `%LOCALAPPDATA%` | `~/.local/share` | `$XDG_DATA_HOME` |
| `{xdg_config}` | `%APPDATA%` | `~/.config` | `$XDG_CONFIG_HOME` |
| `{xdg_cache}` | `%LOCALAPPDATA%` | `~/.cache` | `$XDG_CACHE_HOME` |
| `{vscode:App}` | `%APPDATA%\App\User` | `~/Library/Application Support/App/User` | `~/.config/App/User` |
| `{env:VAR\|home/.x}` | value of `VAR`, else the fallback | | |
| `{repo}` | a repository found while scanning projects (worktrees only) | | |

A path segment of exactly `*` matches every sub-folder. `platforms = ["windows"]` limits a path to some systems.

## Classes

- **regenerable**: rebuilt automatically. May be planned for cleanup when idle.
- **user-data**: history or work. Reported, never planned.
- **protected**: settings, memories, credentials, extensions. Reported, never planned.
- **model**: large downloads (local models). Reported, never planned.

When unsure, choose `user-data`. Being wrong in that direction costs disk space; being wrong the other way costs someone's work.

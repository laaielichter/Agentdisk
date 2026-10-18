# Connect agentdisk to your agent

Install the `agentdisk` binary first (see the README) so `agentdisk` runs from a terminal.
Every client below starts the same stdio server: `agentdisk mcp`.

The MCP server offers read-only tools plus `agentdisk_plan`. It has **no** tool to apply,
restore, purge or compact: only you can do that, in your own terminal.

## Claude Code

As a plugin (MCP server + skill + a SessionStart disk check):

```
/plugin marketplace add laaielichter/agentdisk
/plugin install agentdisk@agentdisk
```

Or only the MCP server:

```
claude mcp add agentdisk -- agentdisk mcp
```

## Cursor — `~/.cursor/mcp.json` or `.cursor/mcp.json`

```json
{ "mcpServers": { "agentdisk": { "command": "agentdisk", "args": ["mcp"] } } }
```

Optional rule (`.cursor/rules/agentdisk.mdc`): "Before creating worktrees or large builds, call agentdisk_check."

## OpenAI Codex — `~/.codex/config.toml`

```toml
[mcp_servers.agentdisk]
command = "agentdisk"
args = ["mcp"]
```

## GitHub Copilot in VS Code — `.vscode/mcp.json`

```json
{ "servers": { "agentdisk": { "type": "stdio", "command": "agentdisk", "args": ["mcp"] } } }
```

## Gemini CLI

```
gemini extensions install https://github.com/laaielichter/agentdisk
```

## Cline, Roo Code, Windsurf, Kiro, Zed, Continue, OpenCode, Amp

Add an MCP server with command `agentdisk` and arguments `mcp` in the client's MCP settings.
The JSON shape is the same as the Cursor example.

## Any agent without MCP

Point it at `AGENTS.md` in this repo, or let it call the CLI with `--json`.

# Security

agentdisk moves and deletes files, so safety bugs matter.

## Report a problem

Please use GitHub's private reporting: **Security → Report a vulnerability** on this repository.
Do not open a public issue for something that could make agentdisk remove data it should keep,
or run commands it should not.

You will get a reply within a week. Fixes for data-loss bugs take priority over everything else.

## What counts

- agentdisk planning or moving something its rules say it must keep (uncommitted work, `.env`, transcripts, settings).
- `apply`, `purge` or `compact` running without a person confirming at a terminal.
- The MCP server exposing anything that moves or deletes files.
- Following a link (symlink or junction) out of the folder being measured or cleaned.

## Supported versions

Only the latest release gets fixes.

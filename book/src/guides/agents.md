# AI agents

There are two ways to let an AI agent use OpenReadout. You can use either or both.

- **The skill** is a Markdown file that tells the agent when and how to run the `openreadout` command through its shell tool. It suits coding agents such as Claude Code, Codex, Cursor, Copilot and Gemini CLI.
- **The MCP server** (`openreadout mcp`) offers OpenReadout as typed tools over the Model Context Protocol. It suits clients without a shell, such as Claude Desktop, and clients where you want to approve each tool separately.

Both return the same JSON as the command line. Install the program first (see [Install](../getting-started/install.md)). The commands below assume `openreadout` is on your `PATH`.

## Claude Code plugin

In Claude Code, the plugin installs the skill and the MCP server together:

```text
/plugin marketplace add openreadout/openreadout
/plugin install openreadout@openreadout
```

The plugin's MCP server runs `openreadout mcp`, so the program must be on your `PATH`.

## The skill

```bash
openreadout self skill                     # print the skill
openreadout self skill --install claude    # into ~/.claude/skills/openreadout
openreadout self skill --install agents    # into ~/.agents/skills/openreadout, read by Codex, Cursor, Copilot and Gemini CLI
openreadout self skill --install all       # both
```

The skill is `SKILL.md` plus a few reference files, in the [Agent Skills](https://agentskills.io) format. The source is [`skills/openreadout`](https://github.com/openreadout/openreadout/tree/main/skills/openreadout). If your client reads skills from another directory, copy the folder there.

## The MCP server

`openreadout mcp --install <client>` adds OpenReadout to a client's configuration file. It keeps the other servers and settings, backs up the old file next to it, and changes nothing if OpenReadout is already configured. `openreadout mcp --config <client>` prints the entry instead, with the full path of the program filled in, so you can add it by hand.

| client | name | user-level file | project file (`--project`) |
| --- | --- | --- | --- |
| Claude Code | `claude` | `~/.claude.json` | `.mcp.json` |
| Claude Desktop | `claude-desktop` | `claude_desktop_config.json` | none |
| Cursor | `cursor` | `~/.cursor/mcp.json` | `.cursor/mcp.json` |
| OpenAI Codex | `codex` | `~/.codex/config.toml` | `.codex/config.toml` |
| VS Code (Copilot) | `vscode` | the user `mcp.json` | `.vscode/mcp.json` |
| Gemini CLI | `gemini` | `~/.gemini/settings.json` | `.gemini/settings.json` |
| Windsurf | `windsurf` | `mcp_config.json` | none |
| Zed | `zed` | `~/.config/zed/settings.json` | `.zed/settings.json` |
| Continue | `continue` | none | `.continue/mcpServers/openreadout.yaml` |
| Cline | `cline` | `cline_mcp_settings.json` | none |

`--config-path PATH` edits another file. A configuration file with comments or trailing commas (common in Zed and VS Code) is not rewritten: the command stops and prints the entry to paste.

The repository contains `.mcp.json`, `.cursor/mcp.json` and `.vscode/mcp.json`, so opening a clone of it in those clients is enough when `openreadout` is on your `PATH`.

### Examples

Most clients use the same JSON entry:

```json
{ "mcpServers": { "openreadout": { "command": "openreadout", "args": ["mcp"] } } }
```

A few differ:

- **VS Code** uses the top-level key `servers` and needs `"type": "stdio"`:

  ```json
  { "servers": { "openreadout": { "type": "stdio", "command": "openreadout", "args": ["mcp"] } } }
  ```

- **Codex** uses TOML:

  ```toml
  [mcp_servers.openreadout]
  command = "openreadout"
  args = ["mcp"]
  ```

- **Zed** puts the entry under `context_servers`.

Several clients also have their own command for this:

```bash
claude mcp add --transport stdio --scope user openreadout -- openreadout mcp
codex mcp add openreadout -- openreadout mcp
gemini mcp add -s user openreadout openreadout mcp
```

**Claude Desktop:** each release includes an MCP bundle, `openreadout-mcp-<target>.mcpb`. Open it to install. Otherwise run `openreadout mcp --install claude-desktop` and restart Claude Desktop.

### Without installing the program

A client can start the server through npm or Docker instead. With Docker, mount the directory that holds your data and keep `-i`:

```json
{ "mcpServers": { "openreadout": { "command": "npx", "args": ["-y", "openreadout", "mcp"] } } }
```

```json
{ "mcpServers": { "openreadout": { "command": "docker",
  "args": ["run", "--rm", "-i", "-v", "/path/to/data:/data", "ghcr.io/openreadout/openreadout", "mcp"] } } }
```

## What to ask

Once the agent is connected, ask in plain words. The skill and the tool descriptions tell the agent which command answers which question.

- "What is in `~/data/plate7.nd2`? Channels, pixel size, how many positions?"
- "Check every file in `/mnt/scope/2026-09-21/` and tell me which ones are truncated."
- "Export position 3, channel 1 and the first ten time points of `big.nd2` to OME-TIFF."
- "Did converting `run.czi` to OME-TIFF change any pixels?"
- "Show me what `stack.lif` looks like, all channels, as a maximum projection."

The MCP server also offers resources and prompts, which some clients show as menu entries or slash commands. See [MCP tools](../reference/mcp.md#resources-and-prompts).

## What the agent can change

- Only `openreadout_export` writes next to your data. It always writes a new file, verifies it by reading it back, and never modifies the source. It replaces an existing output only when asked to overwrite.
- `openreadout_batch`, `openreadout_index` and `openreadout_check` write only to output paths given to them.
- No tool connects to the network.
- Errors come back with a stable `code`, an `exit_code` and a `hint` the agent can act on.

To see exactly what a client receives, run the example client in [`examples/mcp/mcp_client.py`](../../../examples/mcp/mcp_client.py). All tools and their arguments are listed in [MCP tools](../reference/mcp.md).

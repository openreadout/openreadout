# mcp

`mcp` runs OpenReadout as an MCP (Model Context Protocol) server, or configures an MCP client to start it.

```text
openreadout mcp [OPTIONS]
```

Without flags it serves the MCP tools over stdio. The [MCP reference](../mcp.md) describes the tools, resources and prompts it offers.

## Flags

### Configure a client

`CLIENT` is one of `claude`, `claude-desktop`, `cursor`, `codex`, `vscode`, `gemini`, `windsurf`, `zed`, `continue` or `cline`.

- `--config CLIENT`: print the configuration snippet for this client, with this binary's absolute path, instead of serving.
- `--install CLIENT`: write the configuration into the client's config file. Other servers and settings are kept, the previous file is backed up next to it, and the file is left alone if the server is already configured.
- `--project`: with `--install`, write the project-level file in the current directory (such as `.mcp.json` or `.vscode/mcp.json`) instead of the user-level one.
- `--config-path PATH`: with `--install`, edit this file instead of the client's default.

### Serve over HTTP

- `--http ADDR`: serve Streamable HTTP at `ADDR` (for example `127.0.0.1:8765`, endpoint `/mcp`) instead of stdio. Needs a build with the `mcp-http` cargo feature; release binaries are built without it.
- `--allow-remote`: allow `--http` to bind a non-loopback address. Anyone who can reach the port can then read every file this user can read. Set `OPENREADOUT_MCP_TOKEN` to require a bearer token, and use a firewall.

## Examples

```bash
openreadout mcp --install claude                 # Claude Code, user level
openreadout mcp --install cursor --project       # .cursor/mcp.json in this directory
openreadout mcp --config vscode                  # print the snippet only
```

Per-client setup and what to ask: [AI agents](../../guides/agents.md).

Run `openreadout mcp --help` for the full help of your installed version.

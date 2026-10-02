# Connect an assistant

OpenReadout is designed for AI agents. An assistant can use it in two ways: through the **skill**, a Markdown file that tells an agent with a shell how to run the `openreadout` command, or through the **MCP server** (`openreadout mcp`), which offers the same operations as typed tools to clients without a shell, such as Claude Desktop. Both return the same JSON as `--json` on the command line.

## Connect it

**In one line.** Paste this into your agent's chat. The agent reads the skill, which tells it how to install the program and use every command:

```text
curl -fsSL https://raw.githubusercontent.com/openreadout/openreadout/main/skills/openreadout/SKILL.md
```

There is no packaged release yet, and the install script the skill names starts working with the first release. Until then, [install with cargo](install.md) yourself, then connect the program with one of the commands below.

**The skill**, for agents with a shell (Claude Code, Codex, Cursor, Copilot, Gemini CLI):

```bash
openreadout self skill --install claude    # ~/.claude/skills/openreadout
openreadout self skill --install agents    # ~/.agents/skills/openreadout (Codex, Cursor, Copilot, Gemini CLI)
openreadout self skill --install all       # both
```

**The MCP server:**

```bash
openreadout mcp --install claude-desktop   # then restart Claude Desktop
```

The client names are `claude` (or `claude-code`), `claude-desktop`, `cursor`, `codex`, `vscode`, `gemini`, `windsurf`, `zed`, `continue` and `cline`. The command adds an entry with the program's full path to the client's configuration file, keeps the other servers, backs up the old file next to it, and changes nothing if OpenReadout is already there:

```text
$ openreadout mcp --install cursor
configured openreadout in /home/you/.cursor/mcp.json (previous file saved as /home/you/.cursor/mcp.json.bak.1790920590)
```

`--project` writes the project-level file in the current directory instead (such as `.mcp.json`), and `--config <client>` prints the entry without writing it.

**The Claude Code plugin** installs the skill and the MCP server together (the program must be on your `PATH`):

```text
/plugin marketplace add openreadout/openreadout
/plugin install openreadout@openreadout
```

## What to ask first

Ask in plain words and give the path:

- "What is in `~/data/plate7.nd2`? Channels, pixel size, how many positions?"
- "Check every file in `/mnt/scope/2026-09-21/` and tell me which ones are truncated."
- "Show me what `stack.lif` looks like, all channels, as a maximum projection."

## Why it suits an agent

- **Stable JSON.** Every command takes `--json`, with [published schemas](../reference/json/envelope.md). Keys are not renamed or removed without a `schema_version` bump.
- **Fixed exit codes:** 0 ok, 1 error, 2 usage, 3 unknown format, 4 corrupt, 5 I/O, 6 unsupported feature.
- **Errors with a hint** that says what to do next (see below).
- **Assurance on every answer:** whether files like this one were validated against an independent reader.
- **Pictures:** `preview` draws images, traces, spectra and plates, so the agent can look at the data.
- **Cheap metadata:** `info` reads headers only, and `--only` returns just the fields asked for.
- **Read-only inputs** and no network access.

When an agent asks for an image that does not exist, the error tells it how to recover:

```text
$ openreadout preview mini.nd2 --image 3 --json
{
  "ok": false,
  "schema_version": "1",
  "tool": { "name": "openreadout", "version": "0.1.0" },
  "error": {
    "code": "usage",
    "message": "usage error: image 3 not found (file has 1 images)",
    "hint": "Indices are zero-based; `openreadout info FILE --json` lists the images, traces (sweep_count, sample_count), tables (row_count) and spectra the file holds.",
    "exit_code": 2
  }
}
```

## How the assistant sees images

`openreadout_preview` returns a picture as image content, followed by JSON that says what was drawn. Image previews have rulers labelled in full-resolution pixels and a µm scale bar when the pixel size is known, so the assistant can read a feature's coordinates off the rulers and ask again for just that `region`; only the tiles the region touches are read. It measures intensities with `openreadout_stats`, not from the picture.

## Privacy

- No tool connects to the network. Release binaries contain no networking code.
- Source files are never modified. `openreadout_export` writes a new file, reads it back to verify it, then renames it into place, and replaces an existing output only when asked to overwrite.
- The server runs as you, so it can open any file your user account can read.

## More

- [AI agents](../guides/agents.md): every client's configuration file, npm and Docker setups.
- [MCP tools](../reference/mcp.md): every tool and its arguments, resources, prompts and errors.

# OpenReadout agent plugins

The [OpenReadout](https://github.com/openreadout/openreadout) plugin for Claude Code and Codex, and its extension for Gemini CLI. Each one gives the agent the OpenReadout skill and the OpenReadout MCP server, so it can read raw lab-instrument files without the vendor software: metadata, integrity checks, analyses and export to open formats.

## Install

Claude Code:

```text
/plugin marketplace add openreadout/agent-plugins
/plugin install openreadout@openreadout
```

Codex:

```bash
codex plugin marketplace add openreadout/agent-plugins
codex plugin add openreadout@openreadout
```

Gemini CLI:

```bash
gemini extensions install https://github.com/openreadout/agent-plugins
```

## Install the program first

The MCP server runs `openreadout mcp`, so the `openreadout` program must be on your `PATH`. See [Install](https://openreadout.github.io/openreadout/getting-started/install.html). The [AI agents guide](https://openreadout.github.io/openreadout/guides/agents.html) covers the other ways to connect an agent.

## Where this comes from

A release workflow in [openreadout/openreadout](https://github.com/openreadout/openreadout) generates this repository from `packaging/agent-plugins/` and `skills/openreadout/` there, and replaces its contents with each release. Open issues and pull requests in [openreadout/openreadout](https://github.com/openreadout/openreadout/issues), not here.

OpenReadout runs on your computer and makes no network connections. See [PRIVACY.md](PRIVACY.md). Licensed under either the [Apache License 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT), at your option.

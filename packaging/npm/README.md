# openreadout (npm)

Read raw lab-instrument files without vendor software. OpenReadout prints what is in a file as JSON, checks its integrity, exports it to open formats and runs as an MCP server for AI agents. The formats it reads are listed at <https://openreadout.github.io/openreadout/formats.html>.

This npm package runs the native [OpenReadout](https://github.com/openreadout/openreadout) binary. The binary comes in a second, platform-specific package that your package manager picks for your machine, so the install downloads nothing else and runs no install scripts. It works with npm, pnpm, Yarn and Bun, behind proxies and from registry mirrors.

```bash
npx openreadout info run42.czi            # one-off, no global install
npm install -g openreadout && openreadout self formats --json
```

MCP client configuration (Claude Desktop, Cursor, ...):

```json
{ "mcpServers": { "openreadout": { "command": "npx", "args": ["-y", "openreadout", "mcp"] } } }
```

Node.js 18 or newer.

| platform | package |
| --- | --- |
| macOS on Apple silicon | `@openreadout/cli-darwin-arm64` |
| macOS on Intel | `@openreadout/cli-darwin-x64` |
| Linux on x64 (static build, any distribution) | `@openreadout/cli-linux-x64` |
| Linux on arm64 (static build, any distribution) | `@openreadout/cli-linux-arm64` |
| Windows on x64, and on Arm under emulation | `@openreadout/cli-win32-x64` |

## If the binary is missing

`openreadout` exits with a message naming the platform package when that package is not installed. This happens when optional dependencies are turned off (`npm install --omit=optional`) or when a lockfile made on another platform is reused. Reinstall with optional dependencies, or add the package yourself, for example `npm install @openreadout/cli-linux-x64`.

To use a binary you already have, set `OPENREADOUT_BINARY` to its path.

Other ways to install (Homebrew, Scoop, the install script, cargo, Docker, Python): see the [install guide](https://openreadout.github.io/openreadout/getting-started/install.html).

Dual-licensed MIT OR Apache-2.0.

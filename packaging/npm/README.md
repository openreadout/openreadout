# openreadout (npm)

Read raw lab-instrument files without vendor software. OpenReadout prints what is in a file as JSON, checks its integrity, exports it to open formats and runs as an MCP server for AI agents. The formats it reads are listed at <https://openreadout.github.io/openreadout/formats/index.html>.

This npm package is a thin installer for the native [OpenReadout](https://github.com/openreadout/openreadout) binary. On install it downloads the prebuilt, statically linked binary for your platform from the GitHub release with the same version, **verifies its SHA-256 against the release's `SHA256SUMS`**, and links it as `openreadout`. There are no runtime dependencies.

```bash
npx openreadout info run42.czi            # one-off, no global install
npm install -g openreadout && openreadout self formats --json
```

MCP client configuration (Claude Desktop, Cursor, ...):

```json
{ "mcpServers": { "openreadout": { "command": "npx", "args": ["-y", "openreadout", "mcp"] } } }
```

Platforms: macOS (arm64, x64), Linux (x64, arm64; static musl builds, any distribution), Windows (x64; arm64 via emulation). Node.js 18 or newer.

## Environment variables

| variable | effect |
| --- | --- |
| `OPENREADOUT_BINARY` | use this binary instead of downloading one |
| `OPENREADOUT_DOWNLOAD_BASE` | base URL (or `file://` directory) with the release assets and `SHA256SUMS`, e.g. an internal mirror |
| `OPENREADOUT_SKIP_DOWNLOAD` | skip the postinstall download; the first run downloads instead |

Behind a proxy on Node 24+, set `NODE_USE_ENV_PROXY=1` together with `HTTPS_PROXY`. If the postinstall download fails because the network is unavailable, the install still succeeds and the binary is fetched on first run; a checksum mismatch always fails.

Other ways to install (Homebrew, Scoop, cargo, Docker, Python): see the [install guide](https://openreadout.github.io/openreadout/getting-started/install.html).

Dual-licensed MIT OR Apache-2.0.

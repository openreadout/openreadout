# Privacy policy

OpenReadout is software that runs on your own computer. It doesn't collect, store or send any data about you or your files.

## What OpenReadout does with your data

- It reads the files you point it at and returns the results to you, or to the program that called it.
- It makes no network connections. The release binaries contain no networking code, and `cargo deny` checks this in CI.
- It has no telemetry, no analytics, no crash reporting and no update check.
- It writes only the files you ask it to write, such as exports or an AI client's MCP configuration. It doesn't modify the files it reads.
- `openreadout report` writes a diagnostic bundle to a local file. It doesn't send the bundle anywhere. You decide whether to attach it to an issue, and you can read it first with `--dry-run`.

## The MCP server

`openreadout mcp` talks to your AI client over standard input and output. It runs as your user, so it can read any file your account can read.

The optional `mcp-http` build feature, which release binaries don't include, adds a Streamable HTTP server. It listens on a loopback address unless you pass `--allow-remote`, and it never makes outbound connections.

OpenReadout sends its results only to the client that started it. Your AI client may send those results to its model provider. That provider's privacy policy covers what happens next.

## Other ways to install it

The Python, R, npm, WebAssembly and Docker distributions run the same code and behave the same way.

Installing OpenReadout uses the network, separately from OpenReadout itself:

- The npm package downloads the release binary for your platform from GitHub Releases when you install it, or on first run if the install step was skipped. It checks the binary's SHA-256 against the release's `SHA256SUMS` before using it.
- The install scripts (`scripts/install.sh` and `scripts/install.ps1`) download the release archive from GitHub.
- Package managers such as Homebrew, Scoop, PyPI and crates.io download from their own servers.

GitHub, which also hosts the documentation site, and these package registries have their own privacy policies.

The documentation site and the browser demo load no resources from other origins. The demo page forbids network requests, so a file you drop on it stays in your browser.

## Contact

Questions about this policy: openreadout@gmail.com. To report a network connection you didn't expect, follow [SECURITY.md](SECURITY.md).

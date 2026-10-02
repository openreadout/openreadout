# openreadout

The `openreadout` command reads raw lab-instrument files without the vendor software. It prints what is in them as text or JSON, exports them to open formats such as OME-TIFF, OME-Zarr and mzML, checks them for damage, and runs as an MCP server for AI agents. It is one binary and makes no network connections.

```bash
cargo binstall openreadout     # prebuilt binary
cargo install openreadout      # build from source (Rust 1.91 or later)
```

```bash
openreadout info run.czi                         # what the file holds
openreadout check plate.nd2                      # is it intact? exit code 4 if not
openreadout export slide.lif -o slide.ome.tiff   # convert, verified on write
openreadout mcp                                  # MCP server on stdio
```

Supported formats: [format list](https://openreadout.github.io/openreadout/formats/index.html), or `openreadout self formats`. Documentation: <https://openreadout.github.io/openreadout/>. Other install channels: [install page](https://openreadout.github.io/openreadout/getting-started/install.html).

Cargo features: `mcp` (on by default) adds the MCP server. `mcp-http` (off by default) adds a Streamable HTTP transport that listens on loopback only.

Licensed under MIT or Apache-2.0, at your option.

mcp-name: io.github.openreadout/openreadout

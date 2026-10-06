# Maintaining openreadout-mcp

The MCP server: one module per tool family in `src/tools/`, resources and prompts in `src/resources.rs`, the HTTP transport in `src/http.rs`, and the viewer app in `src/app/`. `book/src/reference/mcp.md` is the user documentation; keep it in step with what the server offers.

## The viewer app

The viewer is an MCP App (the `io.modelcontextprotocol/ui` extension, spec at github.com/modelcontextprotocol/ext-apps, `specification/2026-01-26/apps.mdx`). It shows the data of a file in the chat.

| file | what it holds |
| --- | --- |
| `src/app/mod.rs` | capability check, tool `_meta`, the `ui://` resource, the table of tools with a viewer (`TOOL_VIEWS`), the result hint, the file extensions offered to hosts with file entrypoints |
| `src/app/view.rs` | `openreadout_view`: what each view returns, and the size caps |
| `src/app/web/viewer.html`, `viewer.css`, `viewer.js` | the page, assembled into one HTML document at compile time |
| `src/app/tests.rs` | unit tests and a scripted MCP session over a byte stream |
| `tests/viewer.test.mjs` | Node tests of the page's helpers |

### How a call reaches the viewer

1. The client declares `capabilities.extensions["io.modelcontextprotocol/ui"]` in `initialize`. Only then does `tools/list` add `_meta.ui.resourceUri` to the tools in `TOOL_VIEWS`, list `openreadout_view` (with `visibility: ["app"]`, so the client hides it from the model) and list the `ui://` resource. `OPENREADOUT_MCP_APPS=on|off` overrides the check.
2. The model calls, say, `openreadout_info`. The result gains `_meta["openreadout/view"]`: the arguments the viewer starts from (`view_hint`). The client renders `ui://openreadout/viewer.html` and passes the tool's input and result to it.
3. The page calls `openreadout_view` with those arguments through the client (`tools/call` over `postMessage`) and draws what comes back. Every control in the page is another `openreadout_view` call.
4. Hosts that open files with an app (OpenAI's `_meta["openai/ui"].entrypoints`, type `file`) call `openreadout_view` with `{file: {name, resourceUri}}`. They add the file's path to calls from the page as `_meta["openai/resource"].path`. The first call can arrive without it; the tool then answers `view: "pending"` and the page calls again.

The hooks into the rest of the server are in `src/lib.rs`, in the `ServerHandler` methods `list_tools`, `call_tool`, `get_tool`, `list_resources` and `read_resource`. They call into `app` and touch no tool. When a tool is renamed or merged, update `TOOL_VIEWS` and `view_hint` in `src/app/mod.rs`; the test `listed_tools_point_at_the_viewer` fails while a listed tool does not exist.

### No build step

The page is plain JavaScript and CSS with no dependencies. `html()` in `src/app/mod.rs` puts `viewer.css` and `viewer.js` into `viewer.html` with `include_str!`, so the binary builds without Node and there is no generated bundle to keep in sync. The page is about 70 kB before compression. Keep it dependency-free: a charting or UI library would need a bundler, a committed build and a license review, and the canvas drawing here covers what the views need.

Rules for the page:

- Load nothing from outside the page. MCP Apps hosts apply a CSP with no external origins unless the resource declares them, and rule 5 (no network) applies to the page too. `the_page_is_self_contained` fails on `http://`, `https://`, `<link`, `fetch(` or `import(` in the assembled HTML.
- Use the host's CSS variables (`--color-*`, `--font-*`) with the fallbacks in `viewer.css`. Hosts send colours as `light-dark(…)`, which a canvas cannot parse; `colors()` in `viewer.js` resolves each one through a hidden element.
- Keep the pure helpers (ticks, number formatting, axis arithmetic, decoding) at the top of `viewer.js` and exported for `tests/viewer.test.mjs`.

### Size caps

`openreadout_view` keeps each result small enough for a `postMessage` round trip: pictures at most `MAX_IMAGE_SIZE` px and the preview byte budget (`resources::PREVIEW_BUDGET`), plots at most `MAX_POINTS` points per series (signals longer than that become a min/max envelope, so single-sample spikes survive), at most `MAX_CHANNELS` channels, `MAX_SAMPLES_READ` samples read per channel, `MAX_EVENTS` flow-cytometry events (float32, base64) and `MAX_PEAKS` peak markers. Each result reports what it reduced in `limits` and `notes`. The smoke test (`oracle/mcp_smoke.py`) checks the picture and point caps over every file it reads.

### Testing

```bash
cargo nextest run -p openreadout-mcp            # unit tests and the scripted session
node --test crates/openreadout-mcp/tests/viewer.test.mjs
python3 oracle/mcp_smoke.py target/debug/openreadout crates/openreadout-lif/tests/fixtures/synthetic-dims.lif --synthetic
```

Look at the page in a real host after changing it. The basic host in the ext-apps repository works locally:

1. Build with the HTTP transport (`cargo build -p openreadout --features mcp-http`) and start `openreadout mcp --http 127.0.0.1:3100`.
2. The server refuses browser `Origin` headers, so put a small proxy in front of it that drops `Origin` and answers CORS preflights.
3. In a clone of github.com/modelcontextprotocol/ext-apps, `examples/basic-host`: make its `Client` declare the extension (`new Client(info, {capabilities: {extensions: {"io.modelcontextprotocol/ui": {mimeTypes: ["text/html;profile=mcp-app"]}}}})`), run `npm install`, build `index.html` and `sandbox.html` with Vite, and start `serve.ts` with `SERVERS='["http://localhost:<proxy port>/mcp"]'`.
4. Open http://localhost:8080, call `openreadout_info` on files of each family (image, ABF, FCS, mzML, a ChemStation `.D`, an NMR directory, a plate-reader export), and check light and dark themes and a narrow window.

The basic host hides `openreadout_view` because only the app may call it. To test the file-entrypoint path, use a page that plays the host: read the `ui://` resource over stdio, call `openreadout_view` with `{file: {name, resourceUri}}`, put the page in a sandboxed iframe and add `_meta["openai/resource"].path` to the page's `tools/call` requests.

### Fragile spots

- The extension is young. Hosts differ in what they put in `hostContext` (theme, `containerDimensions`, `displayMode`), and the page has to work when any of it is missing.
- Clients that implement MCP Apps but do not declare the extension see no viewer. That is what the spec asks servers to do; `OPENREADOUT_MCP_APPS=on` is the workaround.
- `FILE_EXTENSIONS` decides which files the ChatGPT and Codex apps open in the viewer. Add only extensions that belong to one vendor format. A generic one (`.tif`, `.csv`, `.txt`, `.raw`) would take over files that other apps open better.

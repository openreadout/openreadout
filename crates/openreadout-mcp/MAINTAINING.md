# Maintaining openreadout-mcp

The MCP server: one module per tool family in `src/tools/`, resources and prompts in `src/resources.rs`, the HTTP transport in `src/http.rs`, and the viewer app in `src/app/`. `book/src/reference/mcp.md` is the user documentation; keep it in step with what the server offers.

## Reply size and writes

- `src/budget.rs` cuts the longest lists of any reply over 38,000 characters and says what it cut. It is the last resort.
- `src/lean.rs` comes first for per-well rows (`openreadout_assay`, `openreadout_qpcr`): each list's main fields, the values left out when they say nothing, rounding, the `fields` argument, and paging by whole plate rows with the `wells` for the rest. When an analysis gains a row field worth reading by default, add it to the list there; the output schema of those rows requires no field, because the default view leaves fields out.
- Only `openreadout_export` and `openreadout_extract` write next to the data (`openreadout_batch`, `openreadout_summarize`, `openreadout_index` and `openreadout_report` write only where told). `openreadout_plot` and `openreadout_preview` only draw, and their results carry a `save` line with the export call; `openreadout_export` with `plot` or `preview` draws the same figure again and saves it through `openreadout_plot::write`. Keep the annotations honest: a tool that can write is not read-only (`every_tool_is_annotated`).

## The viewer app

The viewer is an MCP App (the `io.modelcontextprotocol/ui` extension, spec at github.com/modelcontextprotocol/ext-apps, `specification/2026-01-26/apps.mdx`). It shows the data of a file in the chat.

| file | what it holds |
| --- | --- |
| `src/app/mod.rs` | capability check, tool `_meta`, the `ui://` resource, the table of tools with a viewer (`TOOL_VIEWS`), the result hint, the file extensions offered to hosts with file entrypoints |
| `src/app/view.rs` | `openreadout_view`: what each view returns, and the size caps |
| `src/app/web/viewer.html`, `viewer.css`, `viewer.js` | the page, assembled into one HTML document at compile time |
| `src/app/web/fonts/` | subsets of IBM Plex Sans and Mono for the data labels (SIL OFL, renamed OpenReadout Sans and Mono as the licence asks of modified fonts) |
| `src/app/tests.rs` | unit tests and a scripted MCP session over a byte stream |
| `tests/viewer.test.mjs` | Node tests of the page's helpers |

### How a call reaches the viewer

1. The client declares `capabilities.extensions["io.modelcontextprotocol/ui"]` in `initialize`. Only then does `tools/list` add `_meta.ui.resourceUri` to the tools in `TOOL_VIEWS`, list `openreadout_view` (with `visibility: ["app"]`, so the client hides it from the model) and list the `ui://` resource. `OPENREADOUT_MCP_APPS=on|off` overrides the check.
2. The model calls, say, `openreadout_info`. The result gains `_meta["openreadout/view"]`: the arguments the viewer starts from (`view_hint`). The client renders `ui://openreadout/viewer.html` and passes the tool's input and result to it.
3. The page calls `openreadout_view` with those arguments through the client (`tools/call` over `postMessage`) and draws what comes back. Every control in the page is another `openreadout_view` call.
4. Hosts that open files with an app (OpenAI's `_meta["openai/ui"].entrypoints`, type `file`) call `openreadout_view` with `{file: {name, resourceUri}}`. They add the file's path to calls from the page as `_meta["openai/resource"].path`. The first call can arrive without it; the tool then answers `view: "pending"` and the page calls again.
5. The hint carries the agent's `annotations` as the tool got them (`PASSED_KEYS`). The page normalizes them (`normalizeAnnotations` in `viewer.js` accepts several spellings of points, boxes, polygons, lines, arrows, text, bands and lines across a plot, and `wells`) and draws them itself: in full-resolution pixels on images, in axis units on plots. `openreadout_view` never sees them.
6. A tool listed as `ToolView::Figure` (meant for `openreadout_plot`) gets the hint `{view: "figure"}`. The page then shows the tool's own result and calls nothing: the first image content, zoomable, with hover read from `structuredContent.figure` (or `_meta["openreadout/figure"]`) when the result has it:

   ```text
   figure: { title?, width, height,            // size of the picture the data refers to
             panels: [{ area: {x, y, width, height},   // the plot area in picture pixels (or plot_area)
                        x: {domain: [a, b], scale?: "linear" | "log", unit?, reversed?},
                        y: {domain: [a, b], scale?, unit?},
                        series: [{name, x: [...], y: [...]}] }] }
   ```

   A one-panel figure may put `area`, `x`, `y` and `series` at the top level.

### What the user sends back

The page tells the host what the user looks at and what they selected, with `ui/update-model-context` (text for the model plus `structuredContent.viewer`), and sends a question as a user message with `ui/message` when the user presses Ask. Both are optional host capabilities (`hostCapabilities.updateModelContext`, `.message`); without them the page offers only its local actions and a copy button. The text names the selection in the units the tools take (full-resolution pixels, well specs such as `A1:B3`, minutes, m/z, parameter filters) and writes out the tool calls that would look closer, so the agent can use them as they are. `selectionText` in `viewer.js` builds all of it.

### Images

The page keeps a camera (a rectangle in full-resolution pixels) and the pictures it has loaded (`S.img.textures`), and draws them coarse to fine. Zooming and panning move the camera at once; when it settles, the page asks `openreadout_view` for the visible region at the canvas's size, unless a loaded picture is already sharp enough there. A whole-image picture at about 640 px is the overview: it fills in while zooming and is the small map in the corner. When an agent's call has a `region`, the page shows the overview first and flies to the region; when a host keeps one viewer for several calls, each new region is a flight from the current view.

The hooks into the rest of the server are in `src/lib.rs`, in the `ServerHandler` methods `list_tools`, `call_tool`, `get_tool`, `list_resources` and `read_resource`. They call into `app` and touch no tool. When a tool is renamed or merged, update `TOOL_VIEWS` and `view_hint` in `src/app/mod.rs`; the test `listed_tools_point_at_the_viewer` fails while a listed tool does not exist.

### No build step

The page is plain JavaScript and CSS with no dependencies. `html()` in `src/app/mod.rs` puts `viewer.css` and `viewer.js` into `viewer.html` with `include_str!`, and the fonts as base64 in place of `/*FONTS*/[]`, so the binary builds without Node and there is no generated bundle to keep in sync. The page is about 250 kB before compression (80 kB of it fonts) and is read once per conversation. The page loads the fonts with the `FontFace` API from those bytes: the default MCP Apps CSP has no `font-src`, so a `data:` URL in CSS would be blocked. Keep it dependency-free: a charting or UI library would need a bundler, a committed build and a license review, and the canvas drawing here covers what the views need.

Rules for the page:

- Load nothing from outside the page. MCP Apps hosts apply a CSP with no external origins unless the resource declares them, and rule 5 (no network) applies to the page too. `the_page_is_self_contained` fails on `http://`, `https://`, `<link`, `fetch(` or `import(` in the assembled HTML.
- Use the host's CSS variables (`--color-*`, `--font-*`) with the fallbacks in `viewer.css`. Hosts send colours as `light-dark(…)`, which a canvas cannot parse; `colors()` in `viewer.js` resolves each one through a hidden element.
- Keep the pure helpers (ticks, number formatting, axis arithmetic, decoding, annotation shapes, label placement, camera paths, well specs) at the top of `viewer.js` and exported for `tests/viewer.test.mjs`.
- Draw data in the OpenReadout look (`THEMES` in `viewer.js`, the same colours as `openreadout-figure`): ink and a warm red accent, quiet axes with the unit after the last tick, direct labels instead of legends. Chrome colours come from the host.
- The page draws everything but the pointer's overlays (hover, selection, popovers) into an offscreen canvas once; moving the pointer copies it back and draws the overlays, so hover stays cheap on 50,000 events or a 384-well plate of curves.
- No `<form>`: sandboxed frames without `allow-forms` block form submission.

### Size caps

`openreadout_view` keeps each result small enough for a `postMessage` round trip: pictures at most `MAX_IMAGE_SIZE` px and the preview byte budget (`resources::PREVIEW_BUDGET`), plots at most `MAX_POINTS` points per series (signals longer than that become a min/max envelope, so single-sample spikes survive), at most `MAX_CHANNELS` stacked channels or `MAX_OVERLAY_CHANNELS` overlaid ones (qPCR wells) sharing `MAX_PLOT_VALUES` points, `MAX_SAMPLES_READ` samples read per channel, `MAX_EVENTS` flow-cytometry events (float32, base64; with gating, the first `MAX_GATED_READ` events are gated and sampled), `MAX_PEAKS` peak markers, and `MAX_TIME_POINTS` time points per well of a kinetic plate (float32, base64). Each result reports what it reduced in `limits` and `notes`. The smoke test (`oracle/mcp_smoke.py`) checks the picture and point caps over every file it reads.

### Testing

```bash
cargo nextest run -p openreadout-mcp            # unit tests and the scripted session
node --test crates/openreadout-mcp/tests/viewer.test.mjs
python3 oracle/mcp_smoke.py target/debug/openreadout crates/openreadout-lif/tests/fixtures/synthetic-dims.lif --synthetic
```

Look at the page in a host after changing it, at the size people see it, in light and dark themes and in a narrow frame. A small test host is enough for most changes: a Node script that starts `openreadout mcp` over stdio, declares the extension in `initialize`, serves a page that puts the viewer in an `<iframe sandbox="allow-scripts">`, answers `ui/initialize` (with and without the `message` and `updateModelContext` capabilities), forwards `tools/call`, logs `ui/message` and `ui/update-model-context`, and sends the agent's tool input and result. Drive it with Playwright for screenshots. Serve the page from the source files (with the fonts filled in) so that changing the page needs no rebuild.

The basic host in the ext-apps repository works too:

1. Build with the HTTP transport (`cargo build -p openreadout --features mcp-http`) and start `openreadout mcp --http 127.0.0.1:3100`.
2. The server refuses browser `Origin` headers, so put a small proxy in front of it that drops `Origin` and answers CORS preflights.
3. In a clone of github.com/modelcontextprotocol/ext-apps, `examples/basic-host`: make its `Client` declare the extension (`new Client(info, {capabilities: {extensions: {"io.modelcontextprotocol/ui": {mimeTypes: ["text/html;profile=mcp-app"]}}}})`), run `npm install`, build `index.html` and `sandbox.html` with Vite, and start `serve.ts` with `SERVERS='["http://localhost:<proxy port>/mcp"]'`.
4. Open http://localhost:8080, call `openreadout_info` on files of each family (image, ABF, FCS, mzML, a ChemStation `.D`, an NMR directory, a plate-reader export), and check light and dark themes and a narrow window.

The basic host hides `openreadout_view` because only the app may call it. To test the file-entrypoint path, use a page that plays the host: read the `ui://` resource over stdio, call `openreadout_view` with `{file: {name, resourceUri}}`, put the page in a sandboxed iframe and add `_meta["openai/resource"].path` to the page's `tools/call` requests.

### Fragile spots

- The extension is young. Hosts differ in what they put in `hostContext` (theme, `containerDimensions`, `displayMode`), and the page has to work when any of it is missing.
- Clients that implement MCP Apps but do not declare the extension see no viewer. That is what the spec asks servers to do; `OPENREADOUT_MCP_APPS=on` is the workaround.
- `FILE_EXTENSIONS` decides which files the ChatGPT and Codex apps open in the viewer. Add only extensions that belong to one vendor format. A generic one (`.tif`, `.csv`, `.txt`, `.raw`) would take over files that other apps open better.

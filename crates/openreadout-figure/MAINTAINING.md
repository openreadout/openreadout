# Maintaining openreadout-figure

The drawing layer under every picture OpenReadout makes: `openreadout-preview` (previews, the viewer's pictures, chromatogram and assay plots) and `openreadout-plot` (`openreadout plot`, `openreadout_plot`) build a `Figure` here and render it. The crate knows nothing about files or instruments.

## Layout

| file | what it holds |
| --- | --- |
| `src/scene.rs` | `Figure`: an ordered list of paths, text and raster images in pixel coordinates (y down), and its PNG, JPEG and SVG output. |
| `src/path.rs`, `src/geom.rs` | Vector paths and rectangles, and the grid of small multiples. |
| `src/render.rs` | Rasterizing with tiny-skia: anti-aliased paths, text as glyph outlines, images copied pixel for pixel when they sit on whole pixels. |
| `src/svg.rs` | SVG output. Text stays text (IBM Plex with fallbacks), images are embedded as PNG. |
| `src/font.rs` | The embedded fonts: WOFF tables inflated on first use, read with skrifa for metrics, kerning and outlines. |
| `src/theme.rs` | The look: colours of the light and dark themes and type sizes that follow the figure size (`Theme::for_size`). |
| `src/scale.rs` | Linear and log scales, nice ticks, short tick and value labels. |
| `src/chart.rs` | `Panel` (a plot rectangle with marks in data units and quiet axes), `Envelope` (min/max per pixel column), markers, arrows, colour bars, legends and end labels. |
| `src/label.rs` | `LabelPlacer` (labels that keep apart) and `Pill` (a label on a background that reads on any image). |
| `src/color.rs`, `src/raster.rs`, `src/jpeg.rs` | Colours and colour maps, 8-bit RGB rasters with PNG encoding, and a baseline JPEG encoder written from ITU-T T.81. |
| `fonts/` | IBM Plex Sans Regular and Medium and IBM Plex Mono Regular, the unmodified WOFF files IBM publishes, and `OFL.txt`. |
| `examples/gallery.rs` | Sample figures: `cargo run -p openreadout-figure --example gallery -- OUT_DIR`. |

## Rules

- Same input, same bytes. No threads, no hash-map iteration order in output, no platform maths that can differ between targets (the JPEG DCT uses literal constants). Tests compare bytes and hashes.
- No I/O: the crate returns bytes and the callers write them (through `write_verified` in `openreadout-preview` and `openreadout-plot`).
- The look lives in `theme.rs`. Callers take colours and sizes from the `Theme`, not from literals, so light and dark stay in step.
- The fonts are data under the SIL Open Font License 1.1, an exception in `deny.toml` and `about.toml`. Keep the files unmodified: a subset or a changed font would have to be renamed under the licence. `cargo xtask r-vendor` copies `fonts/` into the R source package, and `NOTICE` names them.
- The WebAssembly module carries the fonts too (about 240 kB of WOFF). Check `scripts/wasm.sh sizes` after adding a face.

## Checking a change

`cargo nextest run -p openreadout-figure` runs the unit tests and `cargo test --doc -p openreadout-figure` the examples in the docs. Then look at real pictures, at the size they are shown: the example gallery, `cargo nextest run -p openreadout-preview` (the golden hashes in its `tests/synthetic.rs` change with any visible change; update them only after looking at the new pictures), and the website gallery (`node book/scripts/gallery.mjs` with a release build), in light and dark.

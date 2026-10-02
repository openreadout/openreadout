# Try it in your browser

**[Open the demo page](../demo/index.html)** and drop a microscopy, mass spectrometry, flow cytometry, electrophysiology, NMR or plate-reader file on it. The page shows what `info --view explain`, `info --json`, `check` and `preview` would print. It runs OpenReadout compiled to WebAssembly, inside the browser tab.

Nothing is uploaded. The page's Content-Security-Policy forbids any network request after the page has loaded its own files, so a dropped file cannot leave your computer. Files up to 256 MiB are read whole; larger files are read in blocks, only where the readers look.

The page reads one dropped file. It does not read data sets that span several files or a directory, such as Bruker `.d`, ChemStation `.D`, Waters `.raw`, NMR experiment directories or multi-file OME-TIFF. The npm package can read those from a dropped folder, and the command line reads all of them. OME-Zarr is not in the WebAssembly build.

To use the same WebAssembly module in your own web page or in Node, see [WebAssembly](../guides/wasm.md).

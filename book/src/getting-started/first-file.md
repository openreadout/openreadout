# Your first file

This page walks through the four commands you will use most: `info`, `check`, `export` and `preview`. The examples use `mini.nd2`, a small Nikon ND2 file committed to the repository at [`crates/openreadout-cli/tests/fixtures/mini.nd2`](https://github.com/openreadout/openreadout/blob/main/crates/openreadout-cli/tests/fixtures/mini.nd2). It holds one 8 × 8 pixel image, one channel and two time points. Every output below is real; long lines are trimmed. Use your own files the same way.

## What is in the file?

```text
$ openreadout info mini.nd2
mini.nd2
  format: Nikon ND2 (nd2) v3.0  size: 1.8 KiB  images: 1  planes: 2
  assurance: validated - every variant feature of this file was read correctly in ...
  [0] -  8x8 z=1 c=1 t=2  uint16 XYCZT  px=0.2500 µm
      ch0 DAPI
      objective: Plan Fluor 10x 10x NA 0
```

`info` reads headers and metadata only, never pixel data, so it is as fast on a 100 GB file as on this one. Each `[n]` line is one image: a CZI scene, an ND2 XY position, a LIF series. Other commands select it with `--image n`.

The `assurance` line says whether files like this one were checked against an independent reader during development. See [Assurance and strict mode](../reference/assurance.md).

`info --view full` adds the vendor's complete metadata tree, with the vendor's names. `info --view structure` lists the parts of the container (chunks, blocks, frames) with their offsets and sizes.

## Is it intact?

```text
$ openreadout check mini.nd2
mini.nd2 (nd2): OK
  info     frames_checked     2 frame chunks verified
  assurance: validated - every variant feature of this file was read correctly in ...
  checks performed:
    - file signature chunk and version string
    - chunk map located from the trailing offset and parsed (or rebuilt by scanning)
    - every chunk in the map has a valid header with the same name at that offset
    - every frame chunk declared by uiSequenceCount exists and holds a full frame
    - loop-tree frame count equals uiSequenceCount
```

Here is the same file cut off after 1200 bytes, as happens when a copy from the microscope computer is interrupted:

```text
$ openreadout check cut.nd2 --live-window 0; echo "exit=$?"
cut.nd2 (nd2): PROBLEMS FOUND
  error    truncated          the chunk map that ends every ND2 is missing or unusable; ...
  warning  structure          trailing chunk-map offset is missing or out of range @1192
  warning  structure          chunk map unusable; recovered 5 chunks by scanning (interrupted acquisition?)
  error    truncated          chunk 'ImageDataSeq|0!' extends past end of file @1155
  error    missing_planes     0 of 2 frames present
  ...
exit=4
```

Exit code 4 means the file is corrupt or truncated, so scripts can test for it. All exit codes are listed in [Commands](../reference/commands/index.md#exit-codes).

A file that changed in the last five minutes may still be in the middle of an acquisition, so `check` reports `acquisition_in_progress` and the other problems as warnings instead of errors, and exits 0. `cut.nd2` was written a moment before the check, so the example passes `--live-window 0` to turn this off.

## Convert it

```text
$ openreadout export mini.nd2 -o mini.ome.tiff
wrote mini.ome.tiff (1 images, 2 planes, 2612 bytes, verified=true)

$ openreadout export mini.nd2 --to ome-zarr -o mini.ome.zarr
wrote mini.ome.zarr (1 images, 2 planes, 2876 bytes, verified=true)
```

`export` never opens the source file for writing. It writes to a temporary file, reads every plane back and compares it with the source, and only then renames the file into place. To export part of a file, use `--image` and `--select`, for example `--select c=1 --select z=2-4`. See [export](../reference/commands/export.md) for all target formats.

To compare the source with its export yourself:

```text
$ openreadout check mini.nd2 --against mini.ome.tiff
mini.nd2 (nd2)
mini.ome.tiff (tiff)
=> identical
metadata: same (0 differences)
image 0: geometry same, channel names same, physical size same
planes: 2 compared, 2 identical, 0 within tolerance, 0 mismatched
```

## Look at it

```text
$ openreadout preview mini.nd2
wrote mini.preview.png (35x49 png, 231 bytes, verified=true): image 0 level 0 c=[0] z=[0] t=[0] ...
view it: open (or Read) mini.preview.png; zoom with --region X,Y,W,H in full-res px read off the rulers (now showing 0,0,8,8)
```

`preview` writes a PNG of one plane next to the input: by default channel 0, the middle z and the first time point. Open `mini.preview.png` and you see the 8 × 8 plane in gray, dark at the top-left corner and brightening toward the bottom-right, inside a frame with rulers along the top and left edges. The rulers are labelled in full-resolution pixels. On larger images a µm scale bar is drawn as well when the pixel size is known; this tiny picture has none. Contrast stretches the 0.1 to 99.9 percentiles of the plane (`--contrast`).

On a large image, read the coordinates you want off the rulers and zoom in with `--region X,Y,WIDTH,HEIGHT`; only the tiles the region touches are read. `--select c=1`, `--mip z` and `--composite` pick other planes, a maximum projection or all channels blended. For traces, spectra and plates, `preview` draws a plot or a heat map instead. See [preview](../reference/commands/preview.md).

## Next steps

- [Connect an assistant](assistant.md) so an AI assistant can run these commands for you.
- Add `--json` to any command for machine-readable output: [Reading the JSON output](reading-json.md).
- `openreadout <command> --help` lists every option. The [command reference](../reference/commands/index.md) has the same information with examples.
- `openreadout self formats` lists every supported format and what it does not handle yet.

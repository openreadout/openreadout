# Fiji and ImageJ

Fiji reads CZI, ND2 and LIF files with its bundled Bio-Formats importer, so you do not need OpenReadout just to look at a file. OpenReadout is still useful alongside Fiji in three ways:

- **Before opening:** `openreadout check` tells you whether a large file is complete, and `openreadout info` what it holds, without starting Java.
- **For sharing:** an exported OME-TIFF opens in Fiji, QuPath, napari, Python and OMERO, with no vendor reader needed on the other side.
- **As a second opinion:** OpenReadout and Bio-Formats were written independently. If they disagree about a file, one of them has a bug. Please [open an issue](https://github.com/openreadout/openreadout/issues/new/choose).

## Open an export

```bash
openreadout export run.czi -o run.ome.tiff
```

In Fiji, choose **File › Import › Bio-Formats**, pick `run.ome.tiff`, and choose *Hyperstack* view and *Composite* colour mode. The dimensions, channel names and pixel size (**Image › Properties**) come from the OME-XML that OpenReadout wrote. Dragging the file onto the Fiji window also works.

If you only need part of a large file, export only that part:

```bash
openreadout export big.nd2 -o pos3_gfp.ome.tiff --image 3 --select c=1 --select t=0-9
```

Use OME-TIFF for Fiji. Whether your Fiji opens OME-Zarr (`--format ome-zarr`, which writes OME-NGFF 0.5 on Zarr v3) depends on the version of its Zarr plugins.

## From a macro

An ImageJ macro can run the program with `exec` and open the result with Bio-Formats. Save this as `openreadout_open.ijm` and run it from **Plugins › Macros › Run…**:

```javascript
// Check a raw file, export it to OME-TIFF with OpenReadout, open the export in Fiji.
input = File.openDialog("Instrument file (CZI, ND2, LIF)");
output = File.getDirectory(input) + File.getNameWithoutExtension(input) + ".ome.tiff";

report = exec("openreadout", "check", input);
if (indexOf(report, "PROBLEMS FOUND") >= 0) {
    if (!getBoolean("openreadout check found problems:\n\n" + report + "\nExport anyway?")) exit();
}

result = exec("openreadout", "export", input, "-o", output, "--overwrite");
print(result);
if (indexOf(result, "verified=true") < 0) exit("Export failed:\n" + result);

run("Bio-Formats Importer", "open=[" + output + "] color_mode=Composite view=Hyperstack stack_order=XYCZT");
```

On macOS, an application started from the Dock does not get your shell's `PATH`. If `exec` cannot find `openreadout`, use its full path, for example `/Users/you/.local/bin/openreadout`.

## Many files

To convert a whole folder, run the export outside Fiji, then point Fiji's batch tools (**Process › Batch › Macro…**) at the exported OME-TIFFs. `openreadout export` takes a directory and writes an OME-TIFF next to each file it recognizes:

```bash
openreadout export raw/ --recursive --skip-unknown --jsonl > export-log.jsonl
```

See also [Many files: batch tables and sample sheets](batch.md).

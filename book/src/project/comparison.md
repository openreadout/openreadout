# Comparison with other tools

How OpenReadout relates to tools you may already use. Facts about other projects come from their public documentation. If something here is out of date, please [open an issue](https://github.com/openreadout/openreadout/issues/new/choose). Several of these projects are the reference readers that OpenReadout is [validated](validation.md) against.

## At a glance

| | OpenReadout | Bio-Formats | bioio | czifile / nd2 / liffile | msconvert (ProteoWizard) |
| --- | --- | --- | --- | --- | --- |
| **What it is** | command-line tool, MCP server, Rust library, Python and R packages | Java library + command-line tools (`showinf`, `bfconvert`) | Python image-reading API with per-format plugins | one Python library per format | mass-spectrometry converter (CLI and GUI) |
| **Domain** | lab-instrument files across many techniques | microscopy and related imaging | microscopy and related imaging | Zeiss CZI / Nikon ND2 / Leica LIF-family | mass spectrometry |
| **Formats** | over 90 ([Formats](../formats/index.md)) | 150+ | depends on installed plugins (CZI, ND2, LIF, OME-TIFF, OME-Zarr, Bio-Formats bridge, ...) | 1 each | most vendor MS formats (Thermo, Bruker, Sciex, Agilent, Waters, Shimadzu) plus open ones |
| **Runtime** | none: one static binary | JVM | Python; some plugins need a JVM or native libraries | Python + NumPy (+ imagecodecs for compressed data) | vendor formats need the vendor DLLs, so Windows (or the Wine-based Docker image) |
| **License** | MIT OR Apache-2.0 | GPL-2.0+ for the format readers (some components BSD) | BSD-3 core; plugins vary (bioio-czi and bioio-lif are GPL-3.0) | BSD-3 | Apache-2.0 for ProteoWizard's code; vendor reader libraries under vendor licenses |
| **How formats were derived** | from public files and permissively licensed documentation, logged per format ([clean-room policy](clean-room.md)) | long-standing community reverse engineering and vendor contributions | delegates to the plugin's library | author's analysis and public information | vendor SDKs (the vendor DLLs) for vendor formats |
| **Machine interface** | JSON with published schemas, fixed exit codes, error hints, MCP tools | Java API; CLI output is text for people | Python objects (xarray, dask, NumPy) | Python objects | files; exit status |
| **Metadata** | normalized OME-style model, the vendor tree, and the provenance of each field | OME-XML model + original metadata | OME model via plugins (`ome_types`) + standard dims/channels/pixel sizes | format-specific structures (nd2 is especially rich) | mzML (PSI-MS controlled vocabulary) |
| **Integrity check** | `check`: structure, truncation, missing planes (exit code 4) | not a dedicated feature | no | no | no |
| **Writes** | open formats such as OME-TIFF, OME-Zarr, mzML, Parquet and NWB ([list](../reference/commands/export.md)); never modifies the source | OME-TIFF and other open formats; OME-Zarr via `bioformats2raw` | OME-TIFF, OME-Zarr, via writer plugins | no (read-only) | mzML, mzXML, MGF and others |
| **Large files** | header-only metadata; one plane at a time | plane/tile access through the API | lazy dask arrays | memory-mapped / lazy access varies by library | streaming conversion |

## When to use which

- **You use Fiji, QuPath or OMERO, or need a format OpenReadout does not read:** Bio-Formats. It reads more microscopy formats than any other reader.
- **You live in Python notebooks and want xarray/dask:** bioio. Its `bioio-openreadout` plugin uses OpenReadout underneath without a GPL dependency, and its other plugins add more formats.
- **You need every detail of one format's vendor metadata in Python:** the dedicated library (`nd2` in particular exposes a great deal). OpenReadout also shows the full vendor tree with `info --view full`, but the dedicated libraries offer more format-specific conveniences.
- **You need a scriptable reader with no dependencies, a permissive license, stable JSON, an integrity check or an MCP server:** OpenReadout.
- **Flow cytometry:** FCS is an open standard with good readers in every language (flowCore, FlowIO, fcsparser, FlowKit). OpenReadout offers the same JSON, CSV and MCP interface and integrity checks as for its other formats, and is compared with FlowIO and fcsparser.
- **Mass spectrometry:** msconvert is the reference converter. OpenReadout reads Thermo `.raw`, Bruker timsTOF `.d`, Agilent MassHunter `.d`, Waters `.raw` and Sciex `.wiff` without vendor DLLs and exports indexed mzML. The [format pages](../formats/index.md) list the versions it was checked on.

## How OpenReadout uses them

czifile, nd2 and liffile are the main reference readers for microscopy in the [validation](validation.md) tests. Bio-Formats (`bfconvert`) decides when they disagree with OpenReadout. bioio and its plugins give second opinions and check exports. OpenReadout's parsers were written from files and permissively licensed documentation, never from the source code of the copyleft projects above ([clean-room policy](clean-room.md)).

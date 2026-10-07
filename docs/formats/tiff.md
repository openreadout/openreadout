# TIFF family

Many microscopes, slide scanners and cameras write TIFF files with their own metadata conventions: OME-TIFF, ImageJ, Zeiss LSM, MetaMorph, NIS-Elements, Aperio, Hamamatsu, Philips, PerkinElmer, Micro-Manager and others. OpenReadout reads all of them with one reader and returns images with their pixel size, channels and planes. The table below lists each convention.

Derived from the open TIFF 6.0, BigTIFF and OME-TIFF specifications, public files, and the public documentation of `tifffile` (BSD-3-Clause) for the vendor conventions. See `docs/provenance/tiff.md`.

One reader (`tiff`, crate `openreadout-tiff`) handles the TIFF container and every microscopy convention layered on it. The container walk and the pixel decoder are shared; a *sub-format* ("flavor") decides how pages become images. `info` names it in `notes` (`TIFF sub-format: …`).

| sub-format (`Flavor`) | detected by | images |
| --- | --- | --- |
| `ome-tiff` (`OmeTiff`) | page 0 `ImageDescription` parses as OME-XML with `Image` elements, or holds `BinaryOnly` | one per OME `Image` with `TiffData` |
| `ome-companion` (`OmeCompanion`) | the opened file is a `*.companion.ome` XML document | as `ome-tiff`, pixels in the referenced TIFFs |
| `zeiss-lsm` (`Lsm`) | tag 34412 with a valid LSM info record | one per position/tile |
| `metamorph-stk` (`MetamorphStk`) | page 0 carries tags 33628 and 33629 (UIC1, UIC2) | one; the UIC2 planes as Z (or T) |
| `nis-elements` (`NisElements`) | tag 65331 holds a block named `MetadataTiffV1_0` (Nikon NIS-Elements TIFF export) | the export's files grouped by name: one image per `xy` position, `c`/`z`/`t` from the name tokens |
| `aperio-svs` (`Svs`) | `ImageDescription` starts with `Aperio` | the baseline (focal planes as Z) |
| `philips-tiff` (`PhilipsTiff`) | `Software` starts with `Philips` and page 0's description is an XML `DataObject` of type `DPUfsImport` | one pyramidal image (§ Philips TIFF) |
| `hamamatsu-ndpi` (`Ndpi`) | tag 65420 present | the largest pyramid page (focal planes as Z) |
| `perkinelmer-qptiff` (`Qptiff`) | `Software` starts with `PerkinElmer-QPI` or the description is a `PerkinElmer-QPI-ImageDescription` | full-resolution pages as channels |
| `metaseries` (`MetaSeries`) | `Software` = `MetaSeries` and a `<MetaData>` description | as `plain`, with the MetaSeries calibration, name, time and objective |
| `imagej` (`ImageJ`) | description starts with `ImageJ=` key/value lines | one hyperstack |
| `micro-manager` (`MicroManager`) | tag 51123 on a file with no OME-XML/ImageJ description | as `plain` |
| `plain` (`Plain`) | anything else | pages grouped by identical geometry, each group one image with its pages as Z |
| `metamorph-nd` (`MetamorphNd`) | the opened file is a `.nd` text file starting with `"NDInfoFile"` | one per stage position; wavelengths as C, time points as T, each file's planes as Z |

Micro-Manager JSON (tag 51123) is also read on top of any other flavor (pixel size, channel, exposure, camera, time). When a flavor's metadata is inconsistent (e.g. OME-XML that does not parse), the reader falls back to `plain` and says so in `notes`.

## Container (TIFF 6.0 and BigTIFF — open specifications)

### Header

| offset | size | field | meaning |
| --- | --- | --- | --- |
| 0 | 2 | `byte_order` | `II` little-endian, `MM` big-endian |
| 2 | 2 | version | 42 = classic TIFF (`TIFF_MAGIC`), 43 = BigTIFF (`BIGTIFF_MAGIC`) |
| 4 | 4 (classic) | `first_ifd_offset` | u32 |
| 4 | 2+2 (BigTIFF) | offset size, reserved | must be 8 and 0 |
| 8 | 8 (BigTIFF) | `first_ifd_offset` | u64 |

### IFD (image file directory, a "page")

Classic: u16 entry count, 12-byte entries, u32 next-IFD offset. BigTIFF: u64 count, 20-byte entries, u64 next offset. An entry is `tag` (u16), `field_type` (u16), `count` (u32/u64), and either the value itself (when it fits in 4/8 bytes) or `value_offset`. Field types 1–13 and 16–18 are decoded (rationals are divided out); unknown types are skipped as the specification requires. The main chain is walked from the header; cycles, offsets inside the header, directories past the end of file and out-of-range values are recorded as `StructureProblem`s (`problems`) and reported by `check` rather than aborting the open. SubIFDs (tag 330) are read on demand for pyramid levels.

### Tags the reader uses

| tag | spec name | our use |
| --- | --- | --- |
| 254 | NewSubfileType | bit 0 = reduced resolution (thumbnail, pyramid level); 1/9 = SVS label/macro |
| 256/257 | ImageWidth/ImageLength | plane size |
| 258 | BitsPerSample | with 339 → pixel type |
| 259 | Compression | 1 none, 5 LZW, 6 old-style JPEG (tables form), 7 JPEG, 8/32946 deflate, 32773 PackBits, 50000 zstd, 33003/33004/33005/34712 (JPEG 2000), 50001 WebP, 50002/52546 JPEG XL decoded; 34887 LERC recognized, decoded only in builds with the `lerc` feature |
| 262 | PhotometricInterpretation | 2 RGB / 6 YCbCr → interleaved RGB samples; YCbCr is converted to RGB only inside JPEG and JPEG 2000 (Aperio 33003 codes YCbCr under photometric 2) |
| 270 | ImageDescription | OME-XML, ImageJ, Aperio, QPI conventions |
| 271/272/305/306 | Make/Model/Software/DateTime | instrument and acquisition time for plain/NDPI files |
| 273/279, 324/325 | Strip/Tile offsets and byte counts | chunk table |
| 277 | SamplesPerPixel | samples per pixel |
| 278 | RowsPerStrip | strip height (default: whole image) |
| 282/283/296 | X/YResolution, ResolutionUnit | physical pixel size: µm = 10 000 / res (cm) or 25 400 / res (inch); inch resolutions of 1, 72, 96, 150, 300, 600 are treated as screen/print defaults and ignored; unit 1 (none) gives no size |
| 284 | PlanarConfiguration | 1 chunky (samples interleaved), 2 planar (one sample per chunk) |
| 317 | Predictor | 2 horizontal differencing, 3 floating point (Adobe technical note 3) |
| 322/323 | TileWidth/TileLength | tiled pages |
| 330 | SubIFDs | OME-TIFF sub-resolutions |
| 266 | FillOrder | 1 most significant bit first (default), 2 least significant first (packed samples only) |
| 339 | SampleFormat | 1 unsigned, 2 signed, 3 float, 5 complex integer, 6 complex float |
| 347 | JPEGTables | abbreviated table stream spliced before each JPEG chunk |
| 34412 | (LSM info) | Zeiss LSM record, below |
| 33628–33631 | (MetaMorph UIC1–UIC4) | STK settings, per-plane Z distance and times, wavelengths, per-plane arrays (below) |
| 65325–65333 | (NIS-Elements) | time, pixel size, stage position, metadata blocks of a NIS-Elements TIFF export (below) |
| 50838/50839 | (ImageJ metadata) | ImageJ binary metadata: info, slice labels, display ranges |
| 51123 | (Micro-Manager metadata) | JSON per plane |
| 65420–65449 | (NDPI) | NDPI flag, magnification, slide-centre offsets, focal-plane Z offset, slide label, scanner serial |

### Pixel data

A page's chunks (strips or tiles) are decoded one by one: read `byte_counts[i]` bytes at `offsets[i]`, decompress, undo the byte order (big-endian files; not for predictor 3), undo the predictor, then copy the rows into the plane (edge tiles are cropped). Chunks with offset or byte count 0 were never written and read as zeros. The plane is the stored values, little-endian, rows unpadded — no photometric inversion, no palette expansion. Chunky multi-sample pages give interleaved samples (`samples_per_pixel` > 1, e.g. RGB); planar pages expose each sample as a channel. Pixel types: see "Sample formats" below. JPEG uses `openreadout-codecs::jpeg_decode_tiff` (the pure-Rust `jpeg-decoder` shared with the CZI reader, `JPEGTables` spliced in front of each chunk; 12-bit sequential streams go to the codec crate's own decoder, which handles grey and colour without chroma subsampling and refuses progressive or subsampled 12-bit streams with exit 6); the colour transform is chosen per chunk as described in "JPEG colour" below. Pyramid levels, thumbnails and macro pages of the slide corpus files decode to within 0.05 of tifffile's mean (`tests/corpus_pages.rs`). JPEG decoders may differ from libjpeg-turbo by one count in a small fraction of samples (IDCT rounding), so JPEG corpus files are marked `lossy` and compared by plane mean when the hash differs. Planes larger than `MAX_PLANE_BYTES` (4 GiB) are refused whole with exit 6 (whole-slide level 0); `info`, `info --view structure` and `check` still work.

A tile in the last row of tiles may decode to fewer rows than the tile height when it holds only the rows inside the image (GDAL writes such PackBits tiles); it is padded with zero rows, as libtiff and tifffile do. A chunk short of rows inside the image is corrupt (exit 4).

### Sample formats

`PageLayout::coding` maps (BitsPerSample, SampleFormat) to the returned pixel type and a `SampleCoding`:

| stored | returned | `SampleCoding` |
| --- | --- | --- |
| 8/16/32-bit integers, 32/64-bit floats | as stored | `Native` |
| 64-bit integers (format 1, 2) | `uint64` / `int64` | `Native` |
| 64/128-bit complex floats (format 6: real, imaginary) | `complex` / `double-complex` | `Native` (each part byte-swapped separately) |
| 1–7, 9–15, 17–31-bit integers (format 1, 2) | `uint8`/`int8`, `uint16`/`int16`, `uint32`/`int32` | `Packed` (`bits`, `signed`): packed most significant bit first, every row starting on a byte (FillOrder 2 reverses the bits of each byte first); signed values are sign-extended |
| 16-bit floats (format 3) | `float` | `Float16` (IEEE half, widened exactly) |
| 24-bit floats (format 3) | `float` | `Float24` (1 sign, 7 exponent bits biased by 63, 16 mantissa bits, in the file's byte order; widened exactly) |
| 32/64-bit complex integers (format 5: two int16 / int32) | `complex` / `double-complex` | `ComplexInt` (widened exactly) |

A 24-bit integer is a packed sample like a 12-bit one (most significant bit first, whatever the file's byte order), while a 24-bit float is stored in the file's byte order: GDAL's `int24.tif` decoded this way equals its `int12.tif` (the same test image), and `float24.tif` equals tifffile. Packed, half-float, 24-bit float and complex-integer samples are decoded with no compression, LZW, deflate, PackBits or zstd; the horizontal predictor is undone on complex integers, the floating-point predictor on half and 24-bit floats; other combinations exit 6. `check` and `info` report other bit depths (33 bits and more, 64-bit floats in format 1) as unsupported samples. 12-bit JPEG pages (compression 7, BitsPerSample 12, unsigned) are read as uint16: the JPEG decoder returns whole 16-bit samples holding 0–4095, so nothing is unpacked.

### Regions and pyramid levels

`read_region` decodes only the chunks a rectangle overlaps (in parallel), and crops each into a region-sized buffer; uncompressed chunks are read row segment by row segment, so a window of a huge uncompressed page reads only its own bytes. Decoded chunks are kept in a 64 MiB `ChunkCache` per open file for the next region. Levels: OME-TIFF — the plane's own page lists its reduced levels in `SubIFDs` (330), largest first; whole-slide flavours — the reduced pages of the main chain (SVS, NDPI, QPTIFF, plain reduced pages), the k-th page with the level's size (counted from the level's first page) belonging to the k-th distinct full-resolution page of the image (QPTIFF: one per channel; NDPI: one per focal plane). `info` → `resolution_levels` lists each level with its tile (or `width × RowsPerStrip`) size. Regions of every level of the corpus pyramids equal tifffile's (`corpus/oracle/regions/`).

Chunks are read in file order and, for costly codecs (LZW, JPEG, JPEG 2000, deflate, zstd), decoded on the thread pool a batch (4 per thread) at a time, then pasted in order; the result does not depend on the thread count.

### JPEG 2000 (compressions 33003, 33004, 33005, 34712)

Each chunk is a raw JPEG 2000 codestream (`FF4F FF51`; a JP2 file is unwrapped to its `jp2c` box), decoded by `openreadout-codecs::jpeg2000_decode_limited` (`rust-j2k`: pure Rust, OpenJPEG-style mid-point reconstruction). The codestream's component count must equal SamplesPerPixel and its precision BitsPerSample (8 or 16, unsigned); the decoded tile is cropped or zero-padded to the chunk. Sub-sampled components (4:2:2 chroma) are replicated to the full grid. Aperio writes two flavours (tifffile's names `APERIO_JP2000_YCBC` and `APERIO_JP2000_RGB`):

| compression | writer (from the codestream's COM marker) | components | colour |
| --- | --- | --- | --- |
| 33005 | Kakadu (`Kakadu-v6.4.1`) | R, G, B with the codestream's irreversible colour transform (COD MCT = 1) | undone by the decoder |
| 33003 | Matrox (`MIL`) | Y, Cb, Cr, no multiple-component transform, Cb/Cr at half horizontal resolution (SIZ XRsiz = 2), photometric still 2 | converted here as OpenJPEG's `sycc_to_rgb` does: `R = Y + (int)(1.402·Cr')`, `G = Y − (int)(0.344·Cb' + 0.714·Cr')`, `B = Y + (int)(1.772·Cb')` with `Cb' = Cb − 128`, `Cr' = Cr − 128`, truncation toward zero, clamped |

Irreversible (9/7) tiles are not bit-exact across decoders: on 156 Aperio tiles (both files, levels 0, 1 and 2) our samples are within 1 grey level of OpenJPEG (tifffile + imagecodecs) for 33005 and within 2 for 33003 (the colour conversion adds one), with 0.03–0.07 % of samples differing; OpenSlide differs from OpenJPEG by up to 1 at level 0 in 42 % of 33003 samples (its own YCbCr conversion). The corpus files are marked `lossy`.

### WebP, JPEG XL, LERC and old-style JPEG (compressions 50001, 50002/52546, 34887, 6)

Validated on GDAL's autotest samples (`crates/openreadout-tiff/tests/fixtures/codecs/`, MIT; strips, tiles, planar and chunky pages): every sample equals tifffile + imagecodecs' (libwebp, libjxl, Esri lerc).

- **WebP (50001):** each chunk is a WebP file (lossy VP8 or lossless VP8L), decoded by `image-webp`. A lossless chunk without alpha in a 4-sample page gets opaque alpha (255); an alpha channel in a 3-sample page is dropped.
- **JPEG XL (50002; 52546 is the DNG 1.7 code):** each chunk is a JPEG XL codestream or container, decoded by `jxl-oxide`: first frame, colour channels then extra channels; integers rounded as libjxl does (`v × max + 0.5`).
- **LERC (34887), off by default:** `lerc-rs` 0.5 panics on malformed blobs (an index out of bounds in its mask handling, an arithmetic overflow; found by fuzzing, SECURITY.md), and with `panic = "abort"` that would end the process, so release builds leave LERC out (exit 6, with a hint to convert the file with GDAL). With the `lerc` cargo feature of `openreadout-tiff` (for trusted files): each chunk is a LERC2 blob, first unwrapped as `LercParameters` (tag 50674) says: value 2 is 0 none, 1 deflate, 2 zstd (value 1 is the LERC version). Every blob header is validated first (rows, columns, depth, valid-pixel count, micro-block size, blob size). Pixels the blob's validity mask marks invalid are 0 (tifffile's convention; GDAL substitutes its `GDAL_NODATA` value, tag 42113, which we do not apply).
- **Old-style JPEG (6), "tables" form (TIFF 6.0 section 22):** chunks hold entropy-coded data only; the tables are separate, one per component, at the offsets in JPEGQTables (519, 64 bytes in zig-zag order, as in a DQT segment), JPEGDCTables (520) and JPEGACTables (521, each 16 code-length counts then the values, as in a DHT segment); JPEGRestartInterval (515) gives a DRI. We rebuild `SOI`, DQT, DHT, DRI, an extended-sequential frame header (SOF1, so four table ids are allowed) sized to the chunk, with Y sampled at YCbCrSubSampling (530, default 2 × 2) and the others 1 × 1, and a scan header, and decode that with `jpeg-decoder`. A chunk that already starts with `SOI` is decoded as is. YCbCr (photometric 6) is converted to RGB the TIFF 6.0 section 21 way — ReferenceBlackWhite (532) and YCbCrCoefficients (529) apply, as in libtiff — not the full-range JFIF way used for compression 7. On the one public sample (corpus `gdal-ojpeg-zackthecat`, ReferenceBlackWhite 16–235) we are within 1.02 grey levels on average of Pillow + libtiff (9.5 with JFIF conversion; worst sample 20, at colour edges: libtiff converts from sub-sampled chroma, jpeg-decoder interpolates it first). Not decoded (exit 6): JPEGProc 14 (lossless) and pages that keep one whole JPEG stream at JPEGInterchangeFormat (513) instead of the table tags.

### JPEG colour

A three-component JPEG stream holds either R, G, B or Y, Cb, Cr; the TIFF page and the stream each say which, and writers disagree. Decided per chunk from `PhotometricInterpretation` and the stream's own markers (`codecs::jpeg_markers`: JFIF `APP0`, `Adobe` `APP14` transform byte, frame component ids and sampling), following libjpeg 6b as tifffile drives it:

| photometric | planar | stream | decoded as | seen in |
| --- | --- | --- | --- | --- |
| 1 / 0 (grey) | any | 1 component | grey | — |
| 2 (RGB) | 1 | JFIF | Y, Cb, Cr → R, G, B | Bio-Formats 6.x (`synthetic-tiff-jpeg-bf670-*`): 4:2:0 JFIF streams in photometric-RGB pages |
| 2 (RGB) | 1 | no JFIF | components as coded (R, G, B) | Aperio SVS (ids 0, 1, 2), QPTIFF and tifffile `outcolorspace="RGB"` (ids `R`,`G`,`B`, Adobe 0) |
| 2 (RGB) | 2 | 1 component per plane | grey per sample | Bio-Formats 6.x from planar input |
| 6 (YCbCr) | 1 | any | the stream decides: JFIF → YCbCr; else Adobe 0 → RGB, 1 → YCbCr; else ids `R`,`G`,`B` → RGB; else YCbCr | NDPI, Bio-Formats 8.x, tifffile's default |
| 6 (YCbCr) | 2 | — | **refused** (exit 6): separate Y, Cb, Cr planes are not converted | Bio-Formats 8.x from planar input (tifffile refuses too) |
| 3, 4, 5, ≥ 7 | any | — | **refused** (exit 6) | — |
| any | any | 2 or ≥ 4 components | **refused** (exit 6) | — |

Photometric RGB without JFIF but with an `Adobe` segment declaring YCbCr is decoded as coded (as tifffile does); `check` flags it `jpeg_colour_ambiguous`. `check` reads the markers of every JPEG page's first written chunk and reports the refused combinations as `unsupported_samples` warnings, so a page that would decode to wrong colours is never silent. (Before 2026-09-24 photometric RGB always meant "as coded", and Bio-Formats 6.x pages came back as Y, Cb, Cr.)

## OME-TIFF (open standard: OME-TIFF specification, OME-XML schemas 2008-02 … 2016-06)

- The OME-XML block is page 0's `ImageDescription`. Element and attribute names in `OmeDocument` follow the OME schema.
- Each `Image` → one image. `Pixels/@SizeX/Y/Z/C/T`, `DimensionOrder`, `Type`; physical sizes and `TimeIncrement` are converted to µm and seconds from their `*Unit` attributes (default µm / s). When `TimeIncrement` is absent but planes carry `DeltaT`, the mean step over T is used.
- `Channel`: `Name`, `Fluor`, `ExcitationWavelength`/`EmissionWavelength` (→ nm), `Color` (signed RGBA → `#RRGGBB`), `AcquisitionMode` + `ContrastMethod` → one readable `acquisition_mode` label (`book/src/guides/metadata.md` § General rules); the detection band (`emission_band_*_nm`, `emission_range_nm`) from the channel's emission filters: `LightPath/EmissionFilterRef` (else the `EmissionFilterRef`s of its `FilterSetRef`) → `Instrument/Filter/TransmittanceRange` `CutIn`/`CutOut` (→ nm, default nm); with several filters in the path the band all of them pass (largest cut-in, smallest cut-out); none when a filter lacks an edge or the band is empty; `Plane/@ExposureTime` of the first plane of each channel → `exposure_ms`. `SamplesPerPixel` > 1 (RGB) divides `SizeC`: chunky pages give interleaved samples, planar pages give one channel per sample.
- `TiffData` maps IFDs to planes: planes are rasterised in `DimensionOrder` (fastest first); `IFD` (default 0), `FirstC/Z/T` (default 0) and `PlaneCount` (default: every IFD of the file, or 1 when `IFD` is given; legacy `NumPlanes` accepted) place `PlaneCount` consecutive IFDs starting at the linear index of `(FirstC, FirstZ, FirstT)`.
- Multi-file sets: `TiffData/UUID` text names the file holding the IFDs; matched first against the UUIDs of files already in the set (`OME/@UUID` of the opened file), then by `UUID/@FileName`, resolved in the opened file's directory (directory components are stripped). Siblings are opened lazily. A missing sibling makes the affected planes fail with exit 4 and is reported by `check` (`missing_file`). A renamed single-file dataset (no `OME/@UUID`, every `TiffData` naming one file that does not exist) is read from the opened file, with a note.
- `BinaryOnly/@MetadataFile`: the opened file holds pixels only; the OME-XML is read from the named file — either a `*.companion.ome` XML document or another OME-TIFF. A `*.companion.ome` can also be opened directly.
- `Instrument` (`Microscope`, `Objective`, `Detector` — the first one is the instrument's detector — and `Filter`/`FilterSet`) resolved through `InstrumentRef`/`ObjectiveSettings` (else the first). `AcquisitionDate` → `acquired_at`. `StructuredAnnotations` linked by `AnnotationRef` appear under `images[].extra.annotations` (kind, namespace, value; `MapAnnotation` as an object); all of them are in `info --view full`.
- `MapAnnotation`s of namespace `openreadout.dev/normalized` (`NORMALIZED_NS`, written by our own export) hold model fields OME cannot carry exactly; linked from the `Image` they give `acquired_at` (full precision, used only when it agrees with `AcquisitionDate` to the second), `instrument.software`, `instrument.software_version` and `objective.immersion`; linked from a `Channel`, `acquisition_mode`. They are read into the model and not repeated in `extra.annotations`.
- An `XMLAnnotation` of namespace `openmicroscopy.org/omero/dimension/modulo` (`MODULO_NS`; OME model documentation, "6D, 7D and 8D storage") folds an extra dimension into Z, C or T: `Value/Modulo/ModuloAlongZ` (or `C`, `T`) with `Type` (angle, phase, tile, lifetime, lambda, other), optional `TypeDescription` and `Unit`, and either `Label` children or `Start`/`Step`/`End`. Its size is the number of labels, else `(End − Start) / Step + 1`; it varies fastest inside its parent axis (stored index = parent index × size + sub index). Each one linked from the image whose size divides the parent axis is listed in `images[].extra.modulo[]` (`along`, `type`, `size`, `parent_size`, and `type_description`, `unit`, `start`, `step`, `end`, `labels` when present); planes keep their stored C/Z/T indices. `Label` text comes from the element, or from a `Text` attribute as in the OME sample files. Checked against tifffile's series axes on the four OME `modulo/` sample files (sub-dimension sizes).
- SubIFDs of the first plane's page → `pyramid_levels` and `info --view structure` entries; only full resolution is read.
- Our own exports (`openreadout export … -o x.ome.tiff`) round-trip: geometry, pixel type, every plane hash and the normalized metadata (channels with bands and modes, objective, instrument with software, exact acquisition time, image names) are reproduced; `compare` finds them identical (`ome_tiff_round_trip_keeps_metadata_corpus` for LIF, CZI and ND2 files, `crates/openreadout-ometiff/tests/metadata_round_trip.rs`). What does not round-trip is listed in `book/src/guides/metadata.md` § OME-XML export.

## ImageJ hyperstacks

`ImageDescription` lines `key=value`, first key `ImageJ`. `channels`, `slices`, `frames` (default 1) give C, Z, T; `images` without them is a plain stack exposed as Z. Page order follows `order` (default `czt`: channel fastest, then slice, then frame; the other five permutations are honoured). Pixel size = 1 / XResolution in `unit` (`micron`, `um`, `µm`, `nm`, `mm`, `cm`, `m`, `inch`; `pixel` gives none), Z step = `spacing` × unit, frame interval = `finterval` (× `tunit`, default seconds) or 1 / `fps`. RGB pages with `channels` equal to the sample count are one channel of interleaved RGB; planar multi-sample pages expose samples as channels. When the file holds fewer pages than planes and page 0's data is one contiguous uncompressed block, the remaining planes follow it back to back (ImageJ "virtual" stacks and > 4 GiB files). Slice labels (binary metadata `labl`) become channel names only when there is one label per channel.

## Zeiss LSM (tag 34412; layout from tifffile's public documentation)

The LSM info record (little-endian) — offsets in bytes:

| offset | type | our name | meaning |
| --- | --- | --- | --- |
| 0 | u32 | `magic` | 0x0300494C or 0x0400494C (`LSM_INFO_MAGICS`) |
| 4 | i32 | `record_size` | record length; later fields exist only if it covers them |
| 8, 12, 16 | i32 | `dim_x`, `dim_y`, `dim_z` | voxel counts |
| 20, 24 | i32 | `dim_channels`, `dim_time` | channels, time points |
| 28 | i32 | `data_type` | 1 = 8-bit, 2 = 12-bit, 5 = float, 0 = varies |
| 40, 48, 56 | f64 | `voxel_size_x_m`, `voxel_size_y_m`, `voxel_size_z_m` | metres |
| 88 | u16 | `scan_type` | 0 xyz stack, 3 xy time series, 6/7 xyz time series (others: line/point/spline scans, not images) |
| 90 | u16 | `spectral_scan` | non-zero for lambda stacks |
| 108 | u32 | (offset) | channel colours and names sub-record |
| 112 | f64 | `time_interval_s` | seconds |
| 132 | u32 | (offset) | time stamps sub-record: i32 size (= 8 + 8·n), i32 n, n × f64 seconds |
| 204 | u32 | (offset) | channel wavelength ranges: i32 n, n × (f64, f64) metres |
| 264, 268 | i32 | `dim_positions`, `dim_tiles` | positions and mosaic tiles (LSM 4.2+) |

Channel colours/names sub-record: u32 size, u32 colour count, u32 name count, u32 colour offset, u32 name offset, u32 mono; colours are RGBA bytes; names are u32-length-prefixed NUL-terminated strings. Pages alternate data and thumbnail (thumbnails have NewSubfileType 1 and are listed as attachments). Channels are the samples of each data page. Data pages run T outer, Z inner, per position. In compressed LSM files `StripByteCounts` holds uncompressed sizes; the reader uses the distance to the next strip instead. Time increment = (last − first time stamp) / (T − 1) when stamps exist, else `time_interval_s`.

## MetaMorph STK, MetaSeries and `.nd` series (layout from corpus files and tifffile's public documentation)

**STK** (`StkInfo`, `parse_stk`, `is_stk`). Page 0 carries four private tags whose declared counts do not describe their sizes, so they are read raw from their `value_offset` (little-endian files only):

| tag | our const | declared | stored | our fields |
| --- | --- | --- | --- | --- |
| 33628 | `STK_TAG_SETTINGS` | LONG × n | n (id u32, value u32) pairs; ids 4/5/6/7/16/17 hold an offset | 3 → `calibrated` (non-zero = on), 4/5 → `x_calibration`/`y_calibration` (u32 / u32 at the offset), 6 → `calibration_units`, 7 → `name` (u32 length + text at the offset) |
| 33629 | `STK_TAG_PLANES` | RATIONAL × planes | 6 u32 per plane | `plane_count` = count; `z_distance` (rational); `created` = (MetaMorph day number, milliseconds since midnight); modification times are not used |
| 33630 | `STK_TAG_WAVELENGTHS` | RATIONAL × planes | as declared | `wavelengths_nm` |
| 33631 | `STK_TAG_PLANE_ARRAYS` | LONG × planes | u16 id + data, until id 0 | 28 → `stage_x`/`stage_y` (per plane x num, x den, y num, y den; **signed** numerators), 37 → `stage_labels` (per plane u32 length + text), 40 → `absolute_z` (per plane signed rational), 29/41 skipped; an id of unknown size ends the walk |

`ImageDescription` holds one NUL-separated text per plane (`plane_texts`); `Exposure: 40 ms` in the first gives `exposure_ms`. Planes: page 0's strips cover plane 0; plane i starts at the first strip offset + i × plane bytes (uncompressed and contiguous only; anything else exits 6). `plane_axis`: Z when every Z distance is non-zero, else T when the creation times differ, else Z. `pixel_size_um` = the calibration × `unit_um(calibration_units)` when calibration is on; otherwise the TIFF resolution tags (as Bio-Formats does: 312 pixels/cm → 32.05 µm). `z_step_um` = the common Z distance. `metamorph_time`: day number + 1 = Julian day number (2440588 = 1970-01-01), local time, no zone; `metamorph_seconds` for differences. `acquired_at` = the first plane's time; `time_increment_s` for T stacks. `extra.metamorph` (`to_json`), `extra.stage_position_um` {x, y, z} of plane 0, `extra.stage_label`. Frames (`info --view full`): per plane `z` (or `t`), `delta_t_s`, `time_local`, `stage_x_um`, `stage_y_um`, `stage_z_um`, `wavelength_nm`.

**MetaSeries** (`MetaSeriesPlane`, `parse_metaseries`). `Software` = `MetaSeries`; the description is `<MetaData>` with `prop`/`custom-prop` elements (`id`, `type`, `value`) at the root, in `PlaneInfo` and in `SetInfo` (kept as `props`: `root`, `plane`, `set`). Used: `spatial-calibration-state`/`-x`/`-y`/`-units` → `pixel_size_um`; `image-name` → `image_name` (image name); `acquisition-time-local` (`20240816 09:49:42.978`, `metaseries_time`) → `acquired_local` → `acquired_at` (local time); `_IllumSetting_` → `illumination` (channel name); `_MagSetting_` → `objective` (with `magnification` from a leading `20x`), `_MagNA_` → `objective_na`; `Description` `Exposure: …` → `exposure_ms`; `stage-position-x/y`, `z-position` → `stage_um`; `wavelength` → `wavelength_nm`; `ApplicationName`/`ApplicationVersion` → `application`/`application_version` (software); `number-of-planes` → `plane_count`. `extra.metaseries` keeps `stage-label`, `camera-binning-x/y`, `wavelength`, `acquisition-time-local`.

**`.nd` series** (`NdFile`, `parse_nd`). A text file of `"Key", value` lines (values quoted or not; a `Description` value continues on the following lines until the next quoted key; `"EndFile"` ends it) — all kept in `keys`. `NDInfoFile` → `version`; `StartTime1` → `start_time` → every image's `acquired_at` (local time, a note says so); `DoTimelapse`/`NTimePoints` → `timelapse`/`time_points`; `DoStage`/`NStagePositions`/`Stage<j>` → `stages` (image names); `DoWave`/`NWavelengths`/`WaveName<i>`/`WaveDoZ<i>` → `waves` (channel names)/`wave_do_z`; `WaveInFileName` → `wave_in_file_name`; `DoZSeries`/`NZSteps` → `z_steps`; `ZStepSize` (decimal point or comma) → `z_step_um`. `image_count` = stages (≥ 1), `channel_count` = wavelengths (≥ 1).

File names (`file_stem`): the `.nd` stem, then `_w<i>` + the wavelength name when `WaveInFileName` (only with wavelengths), `_s<j>` (only with stages), `_t<k>` (only with a time lapse), all one-based; extension `.tif`, `.stk` or `.tiff` in any case, matched against the directory listing case-insensitively. Each file is a member of the data set (`info --view structure` lists them, `check` opens them); a missing file makes its planes fail with exit 4 and `check` report `missing_file`. The geometry, pixel type and calibration come from the first member that opens (STK calibration, else MetaSeries, else resolution tags); Z = `NZSteps` (or the first file's plane count when the `.nd` declares no Z series, with a note), Z step = `ZStepSize` (else the STK Z distance). Plane (c, z, t) of image s is plane z of the file of (s, c, t): an STK stack plane read contiguously, or page z of another TIFF. A wavelength with `WaveDoZ` FALSE in a Z series repeats its single plane at every Z (inferred; no corpus file, a note says so). Files named like the series past `NTimePoints` are counted in a note and not read (Figshare 7583960 holds a 25th time point). `extra`: `description`, `stage_index`, `files` (the first 64 member names). `vendor`: `nd` (every key), `first_file_stk`, `first_file_metaseries`.

## Nikon NIS-Elements TIFF exports (layout from corpus files; see `docs/provenance/tiff.md`)

NIS-Elements' "export ND document to TIFF" writes one single-page TIFF per position, channel, Z plane and time point. Detection (`parse_nis`, `NisPage`): tag 65331 (`NIS_TAG_METADATA`) is a block whose directory names `MetadataTiffV1_0` (UTF-16 at bytes 8–40). Values read (all DOUBLE, meanings inferred — see provenance): 65325 (`NIS_TAG_TIME`) → `time_ms`, time since the acquisition started; 65326 (`NIS_TAG_PIXEL_SIZE`) → `pixel_size_um` → physical size X = Y (0.1625 µm in the corpus: 6.5 µm camera pixels behind a 40× objective, and neighbouring positions 329.5 µm apart ≈ one 2048-pixel field); 65329 (`NIS_TAG_STAGE`) → `stage_um` (X, Y, Z µm; a fourth value repeats the time); 65327/65328 → `unknown` (1.0; kept in `vendor.nis_elements`). The LV-encoded blocks of 65330/65331 (objective, per-channel names, text information — the structures the ND2 reader decodes) and the XML of 65332 are not decoded.

**File sequence.** `parse_sequence_name` (`SequenceName`: `prefix`, `indices`, `extension`) reads index tokens from the end of the file stem: `xy<n>` (`SequenceAxis::Position`), `c<n>` (`Channel`), `z<n>` (`Z`), `t<n>` (`Time`), e.g. `single_plane_1-2000_2xy01c1.tif` → prefix `single_plane_1-2000_2`, xy 1, c 1 (the depositors describe `xy` as the stage positions and `c1`/`c2` as the two channels; `z` and `t` are inferred from the pattern). Opening any file of an export reads every file in the same directory with the same prefix, extension and token letters (`axes`, `index`) as one data set: one image per distinct `xy` (named `<prefix>xy<nn>`), channels, Z planes and time points by sorted distinct numbers (channels named `c<n>`), plane (c, z, t) of an image = page 0 of its file. A missing combination is an unmapped plane (`check`: `missing_planes`). Files are members (`info --view structure`, `index`). The opened file's own image carries `extra.nis_elements` {file, time_ms} and `extra.stage_position_um`. A NIS export whose name has no tokens is one image. Bio-Formats (black box) reads each file on its own; its planes equal ours file by file.

## Aperio SVS

`ImageDescription` = header line(s) + `|`-separated `key = value` pairs (`AperioDescription`: `header`, `keys`). `MPP` → physical size X = Y (µm); `AppMag` → objective nominal magnification; `Date` (MM/DD/YY) + `Time` → `acquired_at`; `ScanScope ID`, `Filename`, all keys under `images[0].extra.aperio`. Page roles: page 0 tiled baseline; page 1 stripped thumbnail; further tiled pages of the baseline size are focal planes (Z), smaller ones pyramid levels; remaining stripped pages are label (NewSubfileType 1) and macro (9) — listed by `info --view structure` as attachments, not images.

## Leica SCN

An SCN file (Leica SCN400/SCN400F; extension `.scn`, shared with Bio-Rad gel images, which are not TIFF) is a BigTIFF whose first `ImageDescription` is an XML document in the namespace `http://www.leica-microsystems.com/scn/2010/10/01` (`parse_scn` → `ScnDocument`: `collection_name`, `uuid`, `collection_size_nm`, `barcode`, `images`). Layout read from the corpus files and tifffile's SCN series (BSD-3, documentation and oracle); OpenSlide's public format page was also consulted.

- `collection` (`sizeX`, `sizeY`: the slide in nanometres; `barcode` as stored, base64 text in the corpus) holds one `image` per scan: the slide overview(s) and the scanned regions, each an image of ours in document order (`ScnImage`: `name`, `uuid`, `creation_date`, `device_model`, `device_version`, `size_x`, `size_y`, `dimensions`, `view_size_nm`, `view_offset_nm`, `spacing_z`, `objective`, `numerical_aperture`, `illumination`, `channels`).
- `pixels/dimension` (`ScnDimension`: `size_x`, `size_y`, `r`, `c`, `z`, `ifd`) names the page of each resolution `r` (0 = full), channel `c` and focal plane `z` (both 0 when absent). Level 0 pages are the image's planes (C = distinct `c`, Z = distinct `z`; an RGB page with one `c` is three interleaved samples); every `r > 0` that lists a page for every plane is a pyramid level whose pages are taken from the dimensions (`Level.pages`), not guessed by size. Pages no image lists are `associated` attachments. An image whose level-0 pages are missing or of another size is skipped with a note.
- `view` (`sizeX`, `sizeY`, `offsetX`, `offsetY`, nanometres): the image's extent and position on the slide. Pixel size = view size / pixel count (0.5 µm for a 20x region, 16.44 µm for the overview; equal to the pages' resolution tags in the corpus), `extra.scn.view_size_nm`, `view_offset_nm`; `extra.scn.covers_slide` marks an image whose view is the whole collection from its origin (an overview), also named in a note.
- `creationDate` → `acquired_at`; `device@model`/`@version` (`;`-separated: scanner, then software) → instrument model and software version (first part), manufacturer Leica, both whole in `extra.scn`; `objectiveSettings/objective` → nominal magnification (0.60833 for the overview camera), `illuminationSettings/numericalAperture` → lens NA, `illuminationSource` (`brightfield`, `fluorescence`) → the channels' `acquisition_mode`.
- Fluorescence `channelSettings/channel` (`ScnChannel`: `index`, `name`, `rgb`, `excitation_filter`, `suppression_filter`, `dichroic`, `exposure_raw`, `ccd_gain`): name, colour (`#RRGGBB`), excitation = the centre of the excitation filter (`BP 405/60` → 405 nm, `filter_centre_nm`), emission band = the suppression filter's centre ± half its width (`470/50` → 445–495 nm, `filter_band_nm`). `exposureTime` (105000, 146000) is copied as stored (`extra.scn.channels[].exposure_time_raw`): its unit is not recorded.

## Ventana BIF

A Roche Ventana BIF (`.bif`; iScan HT/Coreo, DP 200/600) is a BigTIFF marked by an XMP packet (tag 700) holding an `iScan` element (`bif_iscan`: the first of the first 16 pages that has one; OpenSlide's public format page gives the same rule). Pages are named by their `ImageDescription` (`page_role` → `BifPage`): `level=N mag=M quality=Q` (`Level` with `level`, `magnification`) are the pyramid of one image (level 0 the full resolution; `Level.pages` names each level's page), `Label_Image` / `Label Image` (`Label`, the slide overview), `Probability_Image` (`Probability`, the tissue map), `Thumbnail` (`Thumbnail`) and anything else (`Other`) are attachments (`info --view structure`), as for SVS.

- `iScan` attributes (`iscan_attributes`, all under `extra.bif.iscan` as stored): `ScanRes` → pixel size at level 0 (µm, as OpenSlide and tifffile read it; 0.25 in the corpus, equal to the resolution tags), `Magnification` → objective nominal magnification (else the level-0 page's `mag=`), `ScannerModel` → instrument model, `BuildVersion` → software version (`Software` tag → software).
- The full-resolution page's XMP holds `EncodeInfo` (`stitching` → `BifStitching`: `rows`, `cols`, `tile_width`, `tile_height` from `SlideStitchInfo/ImageInfo`; `joints`, `overlapping_joints`, `max_overlap`, `directions` from its `TileJointInfo` entries), summarised under `extra.bif.stitching`. The scanner records where neighbouring camera tiles overlap (`OverlapX`, `OverlapY`, a `Direction`); tifffile does not stitch and neither do we: tiles are returned on their stored grid. When any joint records an overlap (105 of 922 joints, 24 px, in the corpus file) a note says so and the assurance marks the pixels as partially decoded (`--strict` refuses them). No public description establishes how the joints place tiles, so no stitching is attempted.
- Volumetric BIF (ImageDepth, tag 32997, > 1 on the level-0 page) exits 6.

## Philips TIFF

A Philips TIFF (the export format of Philips' scanners and of converters that write it; `.tiff`,
BigTIFF) is detected by `Software` starting with `Philips` and page 0's `ImageDescription` being an
XML `DataObject` of `ObjectType` `DPUfsImport` (`parse_philips` → `PhilipsObject` { `object_type`,
`attributes`, `arrays` }, `attr`, `array`, `to_json`). Rules from OpenSlide's public format page;
layout read from the corpus files (`docs/provenance/tiff.md`).

- The XML is a tree: `Attribute` elements (`Name`, `Group`, `Element`, `PMSVR` type: `IString`,
  `IUInt16`, `IStringArray`, `IDoubleArray` — quoted lists, `string_array`) and
  `IDataObjectArray` attributes holding `Array/DataObject` children. `PIM_DP_SCANNED_IMAGES` holds
  one `DPScannedImage` per `PIM_DP_IMAGE_TYPE` (`WSI`, `LABELIMAGE`, `MACROIMAGE`;
  `scanned_image`); the WSI's `PIIM_PIXEL_DATA_REPRESENTATION_SEQUENCE` one
  `PixelDataRepresentation` per pyramid level (`…_NUMBER`, `…_COLUMNS`, `…_ROWS`, its own
  `DICOM_PIXEL_SPACING`). The whole tree (image data left out) is `images[0].extra.philips.metadata`.
- Page 0 is level 0; the reduced tiled pages smaller than it are the pyramid, in page order; pages
  whose description starts with `Label` or `Macro` are attachments (`info --view structure`), as are other pages.
- Level k is representation k: its downsample is its pixel spacing over level 0's (snapped to an
  integer within 0.1 %, the spacings having 6 significant digits), and the level's size is level
  0's divided by it, rounded down — the TIFF pages are padded to whole tiles (level 4 of
  `Philips-1` is stored 3072 × 2560 for a 2816 × 2240 image). The level is the page's top-left.
  These sizes equal OpenSlide's and tifffile's.
- Pixel size: the WSI's `DICOM_PIXEL_SPACING` (`pixel_spacing_um`: millimetres, row then column →
  µm, x from the column spacing), as OpenSlide reports it. In `Philips-1` (converted from an NDPI)
  it is 0.226907 µm while level 0's representation spacing and the resolution tags hold a rounded
  0.227273 (tifffile's value): the corpus oracle takes OpenSlide's.
- Tiles outside the scanned regions have offset and byte count 0: they are returned white
  (`PageLayout.sparse_fill` = 255), as the scanner renders them in its own downsampled levels (and
  as OpenSlide's transparent tiles are over a white background); other TIFFs keep zeros.
- `DICOM_MANUFACTURER` → manufacturer, `DICOM_MANUFACTURERS_MODEL_NAME` → model,
  `DICOM_SOFTWARE_VERSIONS` (first) → software version (all in `extra.philips.software_versions`),
  `Software` tag → software, `DICOM_ACQUISITION_DATETIME` (`YYYYMMDDhhmmss.ffffff`,
  `dicom_datetime`) → `acquired_at`; `extra.philips`: `barcode` (`PIM_DP_UFS_BARCODE`, base64,
  `base64_decode`), `device_serial_number`, `interface_version`, `derivation`, `representations`.
- The XML's `LABELIMAGE`/`MACROIMAGE` `PIM_DP_IMAGE_DATA` (base64 JPEG) are the attachments
  `label` and `macro` (`attachments`, `extract`).

## Hamamatsu NDPI

Tag 65420 marks the file. 65421 = source-lens magnification (> 0 pyramid page, −1 macro, −2 map); 65422/65423 = X/Y offset of the image centre from the slide centre (nm); 65424 = focal-plane Z offset (nm); 65427 = slide label; 65442 = scanner serial. The largest pyramid page is the image, same-size pyramid pages are focal planes (Z), smaller ones pyramid levels. Physical size from X/YResolution (centimetres). Levels are single JPEG strips; files over 4 GiB (offset high bits in tag 65324) are not supported. The full-resolution page (too large to decode whole) is read by its JPEG restart intervals: tag 65426 lists the byte offset of every interval relative to the strip (65432: their high 32 bits), and the page is a grid of `restart interval × MCU width` by `MCU height` tiles (2048 × 8 in the corpus slide). A tile is decoded as a small JPEG: the page's header up to the start of scan with the frame size set to one tile and the restart interval (DRI) removed (`jpeg_frame`), the interval's bytes without their trailing RST marker, and EOI. Only baseline/sequential JPEG; a progressive full-resolution page is refused (exit 6).

## PerkinElmer QPTIFF (Vectra / inForm)

Each page's `ImageDescription` is an XML `PerkinElmer-QPI-ImageDescription` (`QpiPage`): `ImageType` (`FullResolution`, `Thumbnail`, `ReducedResolution`, `Overview`, `Label`), `Name` (channel name), `Color` (`R,G,B`), `Objective`, `ExposureTime` (unit not documented publicly; raw values under `images[].extra.qpi_exposure_time_raw`), `SlideID`, `AcquisitionSoftware`, `InstrumentType`, and (deep) `Magnification`, `PixelSizeMicrons`. Consecutive full-resolution pages of equal size are the channels; `ReducedResolution` pages are pyramid levels; the rest are attachments. Pixel size from `PixelSizeMicrons`, else X/YResolution.

## Thermo Fisher EER (Falcon electron-event movies)

Falcon 4, 4i and C cameras write movies as EER files (`.eer`): a BigTIFF (little-endian) whose every page is one detector frame of the sensor size (4096 × 4096 in the corpus), stored as one strip with compression 65000, 65001 or 65002 and no BitsPerSample. A page whose compression is one of these marks the file (`Flavor::Eer`, dialect `thermo-eer`); pages of another geometry or compression are skipped with a note.

**Event coding** (`EerCoding`: `skip_bits`, `horz_bits`, `vert_bits`; read as imagecodecs' `imcd_eer_decode` does, BSD-3). The strip is a bit stream read least significant bit first. Each code starts with a run length of `skip_bits` bits: the run's pixels (in raster order) hold no event. A run of all ones (127 for 7 bits) continues into the next code without an event. Any other run is followed by `horz_bits` + `vert_bits` of sub-pixel position, and the event lies on the pixel the run reached; the next run starts on the pixel after it. The stream ends when a run reaches the last pixel or the bits run out; a run past the last pixel is corrupt. 65000 is 8 + 2 + 2 bits, 65001 is 7 + 2 + 2, 65002 reads the three sizes from tags 65007, 65008 and 65009 (7, 2, 2 when absent); other sizes (run lengths outside 2–16 bits, codes over 24 bits) are refused (exit 6).

**Returned samples.** Each frame is an `uint8` plane of event counts on the sensor grid; a pixel holds at most one event per frame (the run after an event starts on the next pixel), so frames are 0/1 and a dose image is the sum over T. The sub-pixel bits (super-resolution positions) are not used, the gain reference is not applied (it is a separate file, below) and the TIFF Orientation (2 or 5 in the corpus) is reported (`extra.eer.orientation`, note), not applied — as tifffile does.

**Metadata.** Tag 65001 of page 0 is an XML block `<metadata><item name=".." unit="..">value</item>…</metadata>` (`parse_items`: numbers as numbers, `Yes`/`No` as booleans; `extra.eer.acquisition`, units in `extra.eer.units`). Used: `sensorPixelSize.width`/`.height` (unit `m`) → physical size X/Y (the nominal pixel size at the specimen, e.g. 7.06e-11 m; the 2022 writer omits it); `exposureTime` (s) / `numberOfFrames` → `time_increment_s` and the channel's `exposure_ms`; `timestamp` → `acquired_at` (ISO 8601 with its offset, as stored); `commercialName` (`cameraName`) → detector. `extra.eer` also has `compression`, `run_length_bits`, `subpixel_bits`. Tag 65002 of each page is the frame's block (`frameID`, `dose` in e/pixel, `timestamp`, `rleCodeLength`, `nrOfSubPixelPerDirection`, `orientation`, `pixelFormat`, `decompressionAlgorithmVersion`): `info --view full` lists it per frame (`extra.frames`); the 2022 writer has none.

**Dose check.** The frame `dose` is the frame's event count divided by the pixel count, to 6 decimals, in every frame of the corpus files that record it: `check` decodes every frame (`eer_event_counts`) and reports `eer_dose_mismatch` when they differ by more than 1e-6, `eer_bad_frame` for a stream that does not decode, and the event total (`eer_events`, info).

**Gain references** (`.gain`) are ordinary TIFFs (4096 × 4096 float32, LZW) whose tag 65001 holds an EER metadata block after an XML declaration; they are read as plain TIFF and the block is copied to `extra.eer.acquisition`.

## Molecular Dynamics GEL (Typhoon, Storm and FLA scanners)

`.gel` files of Molecular Dynamics, Amersham/GE/Cytiva Typhoon and FLA scanners are TIFF pages
with private tags (names as tifffile documents them): 33445 `MDFileTag`, 33446 `MDScalePixel`
(rational), 33447 `MDColorTable`, 33448 `MDLabName`, 33449 `MDSampleInfo` (`key=value` lines:
serial number, date and time, laser, filter, PMT voltage, pixel size, scan mode, software), 33450
`MDPrepDate` (`YYYY:MM:DD`), 33451 `MDPrepTime`, 33452 `MDFileUnits` (`Counts`). `MDFileTag` 2
means the samples are square roots of the counts: counts = value² × `MDScalePixel`; 128 means
linear samples: counts = value × `MDScalePixel`. When a plain TIFF (no other sub-format) carries
`MDFileTag` 2 or 128 on its first page and every image is single-sample 8- or 16-bit, every plane
is returned as the counts, float32 (the arithmetic in float32, as tifffile's `mdgel` series does
it, so the planes agree bit for bit), with a note, `images[].extra.md_gel` (encoding, scale,
units, lab name, preparation date, the sample-info pairs, the stored pixel type) and the scan date
as `acquired_at`. Other `MDFileTag` values are reported and left as stored. A `.gel` re-saved by
ImageJ has lost these tags and is an ordinary TIFF. Image Lab `.scn` files imported from `.gel`
scans keep their square-root samples as stored (`openreadout-gel`).

## Micro-Manager

Tag 51123 holds per-plane JSON. `PixelSizeUm` fills the pixel size when nothing else did; `Channel` and `Exposure-ms` fill a single channel's name and exposure; `Camera` → detector; `Time` → `acquired_at` when absent. Selected keys are under `images[0].extra.micromanager`; the whole object is in `info --view full`. Micro-Manager stacks carry OME-XML, which determines the geometry.

## Integrity checks (`check`)

Codes (EER: `eer_bad_frame`, `eer_dose_mismatch`, `eer_events`, § EER): `truncated` (IFD, field value, strip/tile or contiguous stack past end of file), `bad_ifd_offset`, `ifd_cycle`, `too_many_ifds`, `bad_field`, `bad_page`, `missing_chunks` (fewer offsets/counts than the geometry needs), `sparse_chunks` (info), `unsupported_samples` / `unsupported_compression` (warnings; the structure is valid; `unsupported_samples` also covers JPEG colour codings that are refused, see "JPEG colour"), `jpeg_colour_ambiguous` (warning), `bad_chunk` (a JPEG page whose first chunk has no readable frame header), `missing_planes` (planes not mapped to any IFD), `bad_tiffdata` (IFD index beyond the referenced file), `missing_file` / `unreadable_file` (OME sibling or metadata file), `uuid_mismatch` (warning), `short_acquisition` (LSM with fewer pages than declared). NIS-Elements export files are opened like OME-TIFF members; a (position, c, z, t) without a file is `missing_planes`. Contiguous stacks (ImageJ virtual stacks, STK files, STK members of a `.nd` series) must end inside their file (`truncated`); `.nd` members are opened like OME-TIFF members (`missing_file`, `unreadable_file`). Any error → exit 4.

## Oracle mapping (corpus harness)

`oracle/gen.py` uses `tifffile` series as ground truth (STK files included: tifffile's `stk` series, planes on the Z axis). Series that are our images: all series for OME, Micro-Manager, generic/uniform/shaped files; series 0 only for ImageJ, LSM, SVS, NDPI and QPI (the rest are thumbnails/labels/macros/levels). Axes: Y, X the plane; `S` last or `C` after `X` → interleaved samples; `C` → channel; `S` before Y/X (planar samples) → channel, `c = c_C · S + s`; `Z`, `I`, `Q` → z; `T` → t; `P`, `M` → separate images; any other axis is an oracle error. Physical sizes come from the scales tifffile's format code sets (`coord_scales` with µm-convertible units); axes without one are not compared. BinaryOnly files follow their metadata file; companion files are resolved per the OME-TIFF specification with tifffile reading each page.

## Vocabulary (every public identifier in `openreadout-tiff` must appear here)

| identifier | meaning |
| --- | --- |
| `TiffReader`, `TiffDataset`, `FORMAT_ID`, `EXTENSIONS`, `open`, `path` | reader entry points |
| `Flavor` { `OmeTiff`, `OmeCompanion`, `Lsm`, `Svs`, `Ndpi`, `Qptiff`, `ImageJ`, `MicroManager`, `LeicaScn`, `VentanaBif`, `PhilipsTiff`, `Eer`, `Plain` }, `id` | detected sub-format |
| `PhilipsObject` { `object_type`, `attributes`, `arrays` }, `attr`, `array`, `to_json`, `parse_philips`, `scanned_image`, `string_array`, `pixel_spacing_um`, `dicom_datetime`, `base64_decode`, `sparse_fill` | Philips TIFF (§ Philips TIFF) |
| `BifPage` { `Level`, `Label`, `Probability`, `Thumbnail`, `Other` }, `level`, `magnification`, `page_role`, `iscan_attributes`, `BifStitching`, `stitching`, `rows`, `cols`, `tile_width`, `tile_height`, `joints`, `overlapping_joints`, `max_overlap`, `directions`, `XMP` | Ventana BIF (§ Ventana BIF) |
| `ScnDocument`, `ScnImage`, `ScnDimension`, `ScnChannel`, `SCN_NAMESPACE`, `parse_scn`, `pixel_size_um`, `covers_collection`, `filter_centre_nm`, `filter_band_nm`, `collection_name`, `collection_size_nm`, `barcode`, `creation_date`, `device_model`, `device_version`, `dimensions`, `view_size_nm`, `view_offset_nm`, `spacing_z`, `numerical_aperture`, `illumination`, `r`, `c`, `z`, `rgb`, `excitation_filter`, `suppression_filter`, `dichroic`, `exposure_raw`, `ccd_gain` | Leica SCN description (§ Leica SCN; `uuid`, `images`, `name`, `size_x`, `size_y`, `ifd`, `index`, `objective`, `channels` as elsewhere) |
| `EerCoding`, `skip_bits`, `horz_bits`, `vert_bits`, `event_bits`, `check`, `of`, `is_eer_compression`, `walk`, `decode_counts`, `parse_items`, `has_metadata`, `eer`, `ACQUISITION_METADATA`, `FRAME_METADATA`, `SKIP_BITS`, `HORZ_SUB_BITS`, `VERT_SUB_BITS` | Thermo Fisher EER (§ EER); `eer` is the page's coding in `PageLayout` |
| `TiffFile`, `TiffHeader`, `ByteOrder` { `Little`, `Big` }, `byte_order`, `big_tiff`, `first_ifd_offset`, `header`, `ifds`, `file_len`, `problems`, `TIFF_MAGIC`, `BIGTIFF_MAGIC`, `looks_like_tiff` | container |
| `Ifd`, `offset`, `fields`, `next_offset`, `field`, `uint`, `uints`, `float`, `text`, `bytes`, `read_ifd` | image file directory and typed accessors |
| `Field`, `tag`, `field_type`, `count`, `value_offset`, `value`, `FieldValue` { `Unsigned`, `Signed`, `Float`, `Ascii`, `Bytes` }, `type_size` | IFD entries |
| `StructureProblem`, `code`, `detail` | problems recorded during the walk |
| `ByteSource`, `len`, `order`, `big`, `read_at`, `u16_of`, `u32_of`, `u64_of`, `from_bytes` | bounds-checked random access with the file's byte order; `from_bytes` (on `ByteSource` and `TiffFile`) reads a TIFF held in memory, e.g. a stream of an Olympus OIB compound file; `open_in` (on `ByteSource` and `TiffFile`) opens a path in a byte-source namespace (`openreadout_core::source::Fs`), e.g. a member of an OIF folder dropped into a browser |
| `FileSetMember`, `name`, `uuid`, `metadata_only` | members of a multi-file OME-TIFF set |
| `SampleCoding` (`Native`, `Packed` with `bits`, `signed`, `Float16`, `Float24`, `ComplexInt`), `coding`, `fill_order` | how stored samples become returned samples (Sample formats) |
| `PageLayout`, `from_ifd`, `width`, `height`, `bits_per_sample`, `sample_format`, `samples_per_pixel`, `planar`, `compression`, `photometric`, `predictor`, `tiled`, `chunk_width`, `chunk_height`, `offsets`, `byte_counts`, `jpeg_tables`, `pixel_type`, `chunks_across`, `chunks_down`, `chunks_per_plane`, `expected_chunks`, `is_rgb`, `compression_name` | page geometry and sample layout |
| `SampleSelect` { `All`, `One` }, `read_page`, `MAX_PLANE_BYTES`, `is_decoded` | plane decoding; `is_decoded` tells whether chunks of a compression code are decoded |
| `read_region`, `ChunkCache`, `jpeg_frame`, `ndpi_mcu_starts` | region decoding: the chunks a rectangle overlaps, decoded-chunk cache, NDPI per-interval JPEG header and restart-interval offsets |
| `NdpiHeader`, `header`, `tile_width`, `tile_height`, `MCU_STARTS`, `MCU_STARTS_HIGH`, `parse_header`, `tiled_layout`, `frame` | NDPI full-resolution page read by restart intervals |
| `JpegDecision`, `color`, `conflict`, `jpeg_color`, `jpeg_page_supported` | JPEG colour choice per chunk (§ JPEG colour) |
| `SampleSelect` { `All`, `One` }, `read_page`, `MAX_PLANE_BYTES` | plane decoding |
| `OmeDocument`, `parse`, `parse_ome_xml`, `schema`, `creator`, `binary_only`, `images`, `instruments`, `annotations` | parsed OME-XML |
| `OmeBinaryOnly`, `metadata_file` | `BinaryOnly` reference |
| `OmeImage`, `description`, `acquisition_date`, `instrument_ref`, `objective_ref`, `annotation_refs`, `pixels` | `Image` |
| `OmePixels`, `dimension_order`, `size_x`, `size_y`, `size_z`, `size_c`, `size_t`, `physical_size_x`, `physical_size_y`, `physical_size_z`, `time_increment`, `significant_bits`, `big_endian`, `interleaved`, `channels`, `tiff_data`, `planes`, `effective_size_c`, `plane_axes`, `plane_at`, `linear_of`, `plane_count` | `Pixels` and plane rasterisation |
| `OmeChannel`, `fluor`, `excitation_wavelength`, `emission_wavelength`, `color`, `acquisition_mode`, `illumination_type`, `contrast_method`, `pinhole_size`, `emission_filter_refs`, `filter_set_ref` | `Channel` (its `LightPath` emission filters, `FilterSetRef`; `annotation_refs` as for `Image`) |
| `OmeFilter`, `cut_in_nm`, `cut_out_nm`, `OmeFilterSet`, `filters`, `filter_sets` | `Instrument/Filter` (`TransmittanceRange`) and `FilterSet` |
| `NORMALIZED_NS`, `normalized_value`, `instrument_of`, `channel_mode`, `channel_band`, `acquired_at`, `objective_and_instrument` | our export's normalized annotations and the `OmeDocument` → model helpers shared with the OME-Zarr reader |
| `OmeTiffData`, `ifd`, `first_c`, `first_z`, `first_t`, `file_name` | `TiffData` and its `UUID` |
| `OmePlane`, `the_c`, `the_z`, `the_t`, `delta_t`, `exposure_time`, `position_x`, `position_y`, `position_z` | `Plane` |
| `OmeInstrument`, `microscope_manufacturer`, `microscope_model`, `objectives`, `detectors`, `OmeObjective`, `manufacturer`, `model`, `nominal_magnification`, `lens_na`, `immersion`, `OmeDetector`, `detector_type` | `Instrument` |
| `OmeAnnotation`, `kind`, `namespace` | `StructuredAnnotations` entries |
| `OmeModulo`, `MODULO_NS`, `modulo`, `along`, `type_description`, `unit`, `start`, `step`, `end`, `labels`, `size`, `parent_size`, `parse_modulo`, `modulo_json` | OME Modulo sub-dimensions (`ModuloAlongZ`/`C`/`T`) and `images[].extra.modulo` |
| `length_to_um`, `time_to_s`, `unit_to_um` | unit conversion |
| `ImageJInfo`, `keys`, `channels`, `slices`, `frames`, `hyperstack`, `unit`, `spacing`, `finterval`, `mode`, `parse_description`, `page_of`, `parse_binary` | ImageJ description and binary metadata |
| `LsmInfo`, `LSM_INFO_MAGICS`, `magic`, `record_size`, `dim_x`, `dim_y`, `dim_z`, `dim_channels`, `dim_time`, `data_type`, `voxel_size_x_m`, `voxel_size_y_m`, `voxel_size_z_m`, `scan_type`, `spectral_scan`, `time_interval_s`, `dim_positions`, `dim_tiles`, `channel_names`, `channel_colors`, `time_stamps`, `channel_wavelengths_m`, `stack_axes` | Zeiss LSM record |
| `AperioDescription`, `parse_aperio`, `number`, `acquired_at` | Aperio description (`header`, `keys`, `text` as above) |
| `QpiPage`, `parse_qpi`, `image_type`, `magnification`, `pixel_size_um`, `slide_id`, `acquisition_software`, `instrument_type` | PerkinElmer QPI page XML (`name`, `color`, `objective`, `fields` as above) |
| `MicroManagerPlane`, `parse_micromanager`, `json`, `channel`, `exposure_ms`, `camera`, `time` | Micro-Manager JSON (`pixel_size_um` as above) |
| `id`, `objective` | image id / objective text fields of the parsed conventions |
| `MetamorphStk`, `MetaSeries`, `MetamorphNd` | the MetaMorph sub-formats (`Flavor`) |
| `StkInfo`, `STK_TAG_SETTINGS`, `STK_TAG_PLANES`, `STK_TAG_WAVELENGTHS`, `STK_TAG_PLANE_ARRAYS`, `is_stk`, `parse_stk`, `plane_count`, `z_distance`, `created`, `wavelengths_nm`, `calibrated`, `x_calibration`, `y_calibration`, `calibration_units`, `stage_x`, `stage_y`, `stage_labels`, `absolute_z`, `plane_texts`, `exposure_ms`, `pixel_size_um`, `z_step_um`, `plane_axis`, `to_json` | MetaMorph STK (above) |
| `metamorph_time`, `metamorph_seconds`, `unit_um`, `magnification`, `metaseries_time` | MetaMorph day numbers, calibration units, objective magnification, MetaSeries time text |
| `MetaSeriesPlane`, `parse_metaseries`, `props`, `image_name`, `acquired_local`, `illumination`, `objective_na`, `stage_um`, `wavelength_nm`, `application`, `application_version` | MetaSeries description (above) |
| `NdFile`, `parse_nd`, `version`, `start_time`, `time_points`, `timelapse`, `stages`, `waves`, `wave_do_z`, `wave_in_file_name`, `z_steps`, `z_step_um`, `image_count`, `channel_count`, `file_stem` | MetaMorph `.nd` series (above; `keys`, `description`, `name` as elsewhere) |
| `NisElements`, `NisPage`, `parse_nis`, `NIS_TAG_TIME`, `NIS_TAG_PIXEL_SIZE`, `NIS_TAG_STAGE`, `NIS_TAG_METADATA`, `time_ms`, `unknown` | NIS-Elements TIFF export tags (`pixel_size_um`, `stage_um` as above) |
| `SequenceName`, `SequenceAxis` { `Position`, `Time`, `Z`, `Channel` }, `parse_sequence_name`, `prefix`, `indices`, `extension`, `index`, `axes` | NIS-Elements export file names |
| `MdGel`, `file_tag`, `scale`, `stored`, `units`, `meta`, `prepared_at`, `sample_info`, `linearize`, `info`, `MD_FILE_TAG`, `MD_SCALE_PIXEL`, `MD_LAB_NAME`, `MD_SAMPLE_INFO`, `MD_PREP_DATE`, `MD_PREP_TIME`, `MD_FILE_UNITS`, `md_gel`, `apply_md_gel` | Molecular Dynamics GEL (§ Molecular Dynamics GEL) |

### Performance / robustness merge (2026-09-23)

OME-XML is parsed once at open; companion XML keeps its 256 MiB capped read. LZW strips use bounded output and a reusable 64 KiB per-thread staging buffer.

# Electron microscopy formats

The `openreadout-em` crate has four readers, registered as separate formats:

| format id | reader | file types | notes |
| --- | --- | --- | --- |
| `mrc` | `MrcReader` | `.mrc`, `.mrcs`, `.map`, `.ccp4`, `.rec`, `.st`, `.ali`, `.preali` | open MRC2014 standard: [mrc.md](mrc.md) |
| `dm` | `DmReader` | `.dm3`, `.dm4` | Gatan Digital Micrograph: [dm.md](dm.md) |
| `ser` | `SerReader` | `.ser`, `.emi` | FEI TIA / ES Vision series: [ser.md](ser.md) |
| `emd` | `EmdReader` | `.emd` | Velox EMD (HDF5): [emd.md](emd.md) |

All four report `family = "electron-microscopy"` and fill the same normalized model as the light-microscopy readers: images with `size_x`/`size_y`/`size_z`/`size_t`, physical sizes in µm (EM files store Å, nm or m; conversions are documented per format and the original values are kept in `extra`), `acquired_at` in UTC, `instrument`, and microscope settings (`extra.voltage_kv`, `extra.magnification`, …). `export` to OME-TIFF and OME-Zarr works through the generic exporters.

Crate-level items: `util.rs` holds crate-private helpers (bounded reads, endian decoding, half-float widening, OLE/Unix timestamp conversion); it exports nothing public.

## Vocabulary (public identifiers at the crate root)

| identifier | meaning |
| --- | --- |
| `MrcReader`, `DmReader`, `SerReader`, `EmdReader` | re-exports of the four readers |

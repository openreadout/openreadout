//! Shared helpers for the fuzz targets.
//!
//! The readers open files by path (they seek and read lazily, never slurping the file), so
//! the file-level targets write each input to a per-process scratch file (or, for data sets
//! made of several files, a scratch directory) first.

use std::path::{Path, PathBuf};

use openreadout_core::reader::{FormatReader, PlaneIndex, SNIFF_LEN};

/// Plane cap used under the fuzzer (64 MiB). A few-KB input can legitimately *declare* a
/// multi-GB plane; the readers must refuse it cleanly rather than allocate it, and a low
/// cap keeps libFuzzer's 2 GB RSS limit meaningful.
pub const FUZZ_PLANE_LIMIT: &str = "67108864";

/// Separator between the files of a multi-file input (see [`whole_bundle`]).
pub const BUNDLE_SEP: &[u8] = b"\n=====FUZZ-NEXT-FILE=====\n";

/// Call once from each target's `init` (it runs after libfuzzer-sys installed its
/// abort-on-panic hook).
pub fn init() {
    // Safe in edition 2021; runs before any fuzz iteration (single-threaded).
    std::env::set_var(
        openreadout_core::limits::PLANE_LIMIT_ENV,
        FUZZ_PLANE_LIMIT,
    );
    // Known upstream issue: calamine's cell-reference parser overflows (a panic with debug
    // assertions, as here) on absurd references. The plate reader's workbook loader catches
    // that panic and returns an error, which is the behaviour under test; let it unwind to it
    // instead of aborting. Panics anywhere else (the JPEG XR decoder included, since it is our
    // own safe port) still abort and are reported as crashes.
    let fuzzer_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if info
            .location()
            .is_some_and(|l| l.file().contains("/calamine-"))
        {
            return;
        }
        fuzzer_hook(info);
    }));
}

/// Write `data` to this process's scratch file for `tag` and return its path.
pub fn scratch_file(tag: &str, ext: &str, data: &[u8]) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "openreadout-fuzz-{}-{tag}.{ext}",
        std::process::id()
    ));
    std::fs::write(&p, data).expect("write scratch file");
    p
}

/// Run every read-only `Dataset` operation the CLI and MCP server expose on `path`: `open`
/// -> `info` -> `vendor_metadata` -> `entries` -> `check` -> `read_plane(0, c0 z0 t0)` and
/// its `stats` accumulator ->
/// the first rows of table 0, the first samples of trace 0 sweep 0, spectrum 0,
/// attachment 0 and frame records. Every step may fail; none may panic, abort, hang or
/// exhaust memory.
pub fn exercise(reader: &dyn FormatReader, path: &Path, head: &[u8]) {
    let _ = reader.sniff(&head[..head.len().min(SNIFF_LEN)], path);
    // The assurance profile observes every file `info` describes (docs/assurance.md).
    if let Some(p) = reader.assurance() {
        openreadout_core::assurance::register_profile(p);
    }
    let Ok(mut ds) = reader.open(path) else {
        return;
    };
    if let Ok(info) = ds.info() {
        let _ = serde_json::to_string(&info);
        let a = openreadout_core::assurance::assess_dataset(ds.as_ref(), &info);
        let _ = serde_json::to_string(&a);
        let _ = serde_json::to_string(&openreadout_core::InfoOutput::new(ds.as_ref(), info));
    }
    if let Ok(v) = ds.vendor_metadata() {
        let _ = serde_json::to_string(&v);
    }
    let _ = ds.provenance();
    if let Ok(e) = ds.entries() {
        let _ = serde_json::to_string(&e);
    }
    if let Ok(r) = ds.check() {
        let _ = serde_json::to_string(&r);
    }
    if let Ok(plane) = ds.read_plane(0, PlaneIndex::default()) {
        // What `stats` computes per plane (histogram, percentiles, saturation).
        let acc = openreadout_core::stats::Accumulator::from_plane(&plane);
        let _ = acc.finish(64, openreadout_core::stats::HistogramScale::Linear);
        let _ = acc.finish(64, openreadout_core::stats::HistogramScale::Log);
    }
    // Region and pyramid-level reads (`--region`, `--level`): a small window in the middle
    // and at the far corner of each of the first levels, plus one past the edge.
    if let Ok(info) = ds.info() {
        if let Some(im) = info.images.first() {
            exercise_regions(ds.as_mut(), im);
        }
    }
    let _ = ds.read_table(0, 0, 64);
    let _ = ds.read_trace(0, 0, 0, 4096);
    let _ = ds.read_spectrum(0, 0);
    if ds.attachments().is_ok_and(|a| !a.is_empty()) {
        let _ = ds.read_attachment(0);
    }
    let _ = ds.frames(0, Some(16));
    // Strict mode (`--strict`): the wrapper assesses the file and gates every read.
    if let Ok(mut strict) = openreadout_core::strict::StrictDataset::wrap(ds) {
        let _ = strict.file_assurance();
        let _ = strict.info();
        let _ = strict.read_plane(0, PlaneIndex::default());
        let _ = strict.read_table(0, 0, 8);
        let _ = strict.read_trace(0, 0, 0, 64);
        let _ = strict.read_spectrum(0, 0);
        let _ = strict.check();
    }
}

/// Region and pyramid-level reads (`--region`, `--level`) of plane 0 of image 0: a small
/// window in the middle and at the far corner of each of the first three levels, one window
/// past the right edge, and the whole smallest level.
fn exercise_regions(
    ds: &mut dyn openreadout_core::reader::Dataset,
    im: &openreadout_core::model::ImageInfo,
) {
    use openreadout_core::Region;
    for lv in openreadout_core::region::levels_of(im).iter().take(3) {
        let (w, h) = (lv.size_x.min(1 << 16), lv.size_y.min(1 << 16));
        let (rw, rh) = (w.clamp(1, 7), h.clamp(1, 5));
        let (xmax, ymax) = (w.saturating_sub(rw), h.saturating_sub(rh));
        for (x, y) in [(w / 2, h / 2), (xmax, ymax)] {
            let r = Region::new(x.min(xmax), y.min(ymax), rw, rh);
            let _ = ds.read_region(0, PlaneIndex::default(), lv.level, r);
        }
        let _ = ds.read_region(0, PlaneIndex::default(), lv.level, Region::new(w, 0, 1, 1));
    }
    if im.pyramid_levels > 1 {
        let _ = ds.read_plane_level(0, PlaneIndex::default(), im.pyramid_levels - 1);
    }
}

/// Single-file data set: the fuzz input is the whole file, written with extension `ext`.
pub fn whole_file(reader: &dyn FormatReader, tag: &str, ext: &str, data: &[u8]) {
    let path = scratch_file(tag, ext, data);
    exercise(reader, &path, data);
}

/// Multi-file data set (a SpikeGLX `.bin` + `.meta` pair, a Bruker experiment directory,
/// ...): the fuzz input is split at [`BUNDLE_SEP`] and part `i` is written to `names[i]`
/// (relative, may contain `/`) inside a per-process scratch directory; missing parts are
/// written empty. The reader then opens `names[open]`, or, when `open` is `None`, the
/// subdirectory `root` of the scratch directory (`""`: the scratch directory itself; a name
/// such as `"s.D"` when the reader wants a directory extension).
pub fn whole_bundle(
    reader: &dyn FormatReader,
    tag: &str,
    root: &str,
    names: &[&str],
    open: Option<usize>,
    data: &[u8],
) {
    let dir = std::env::temp_dir().join(format!("openreadout-fuzz-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut parts = split(data, BUNDLE_SEP);
    parts.resize(names.len(), &[]);
    for (name, part) in names.iter().zip(&parts) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create scratch directory");
        }
        std::fs::write(&p, part).expect("write scratch file");
    }
    let path = open.map_or_else(|| dir.join(root), |i| dir.join(names[i]));
    exercise(reader, &path, open.map_or(&[][..], |i| parts[i]));
}

fn split<'a>(data: &'a [u8], sep: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut rest = data;
    while let Some(i) = rest.windows(sep.len()).position(|w| w == sep) {
        out.push(&rest[..i]);
        rest = &rest[i + sep.len()..];
    }
    out.push(rest);
    out
}

/// Split a size hint off the front of a codec input: the first three bytes (little-endian)
/// give an expected decoded length up to 16 MiB; the rest is the payload.
pub fn split_expected(data: &[u8]) -> (usize, &[u8]) {
    if data.len() < 3 {
        return (0, data);
    }
    let n = usize::from(data[0]) | usize::from(data[1]) << 8 | usize::from(data[2]) << 16;
    (n, &data[3..])
}

//! Shared by `tests/sources.rs` (committed fixtures) and the corpus harness
//! (`openreadout-corpus-tests/tests/sources.rs`): open a file from its path and from byte
//! sources, and require identical answers.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::Registry;
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::{
    ByteSource, CachedSource, CallbackSource, Fs, Input, LocalFile, MemFs, MemSource,
};

/// Everything a reader answers cheaply, as one JSON value (errors as their code).
pub fn answers(ds: &mut dyn Dataset) -> serde_json::Value {
    fn val<T: serde::Serialize>(r: openreadout_core::Result<T>) -> serde_json::Value {
        match r {
            Ok(v) => serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
            Err(e) => serde_json::json!({ "error": e.code() }),
        }
    }
    // Tables, traces and spectra are not `Serialize`; compare a hash of their `Debug` form.
    fn dbg<T: std::fmt::Debug>(r: openreadout_core::Result<T>) -> serde_json::Value {
        match r {
            Ok(v) => serde_json::Value::from(format!(
                "{:032x}",
                xxhash_rust::xxh3::xxh3_128(format!("{v:?}").as_bytes())
            )),
            Err(e) => serde_json::json!({ "error": e.code() }),
        }
    }
    let info = ds.info();
    let n_images = info.as_ref().map_or(0, |i| i.images.len());
    let plane = if n_images > 0 {
        match ds.read_plane(0, PlaneIndex::default()) {
            Ok(p) => serde_json::json!({
                "width": p.width,
                "height": p.height,
                "bytes": p.data.len(),
                "xxh3": format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&p.data)),
            }),
            Err(e) => serde_json::json!({ "error": e.code() }),
        }
    } else {
        serde_json::Value::Null
    };
    serde_json::json!({
        "info": val(info),
        "vendor": val(ds.vendor_metadata()),
        "entries": val(ds.entries()),
        "check": val(ds.check()),
        "plane": plane,
        "table": dbg(ds.read_table(0, 0, 32)),
        "trace": dbg(ds.read_trace(0, 0, 0, 256)),
        "spectrum": dbg(ds.read_spectrum(0, 0)),
        "members": ds.member_files(),
    })
}

/// Files that make up the data set at `path`: the directory itself for a folder format;
/// otherwise every file in the same directory and below, up to `budget` bytes in total.
pub fn unit_files(path: &Path, budget: u64) -> Option<Vec<PathBuf>> {
    let root = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()? {
            let p = e.ok()?.path();
            let m = std::fs::metadata(&p).ok()?;
            if m.is_dir() {
                stack.push(p);
            } else {
                total += m.len();
                if total > budget {
                    return None;
                }
                out.push(p);
            }
        }
    }
    Some(out)
}

/// The same files in memory, under the same (absolute) paths.
pub fn mirror(files: &[PathBuf]) -> Fs {
    let mut fs = MemFs::new();
    for p in files {
        let bytes = std::fs::read(p).unwrap();
        fs.insert(p, Arc::new(MemSource::new(p.display().to_string(), bytes)));
    }
    Fs::new(Arc::new(fs))
}

/// A directory namespace whose files all read through cached host callbacks.
pub fn callback_mirror(files: &[PathBuf]) -> Fs {
    let mut fs = MemFs::new();
    for p in files {
        let file = LocalFile::open(p).unwrap();
        let cb = CallbackSource::new(
            p.display().to_string(),
            file.size().unwrap(),
            move |off, buf| file.read_at(off, buf),
        );
        fs.insert(p, Arc::new(CachedSource::new(Arc::new(cb), 4096, 8)));
    }
    Fs::new(Arc::new(fs))
}

/// One file behind a host callback with a small block cache (many round trips).
pub fn callback(path: &Path) -> Input {
    let bytes = Arc::new(std::fs::read(path).unwrap());
    let size = bytes.len() as u64;
    let cb = CallbackSource::new(path.display().to_string(), size, move |off, buf| {
        let Ok(start) = usize::try_from(off) else {
            return Err(io::Error::other("offset"));
        };
        let n = buf.len().min(bytes.len().saturating_sub(start));
        buf[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    });
    let cached: Arc<dyn ByteSource> = Arc::new(CachedSource::new(Arc::new(cb), 4096, 8));
    Input::from_source(path, cached)
}

/// Compare the answers from the path with those from `input`. `Ok(false)` when the reader
/// does not read byte sources (and said so cleanly).
pub fn compare(reg: &Registry, path: &Path, input: &Input) -> Result<bool, String> {
    let (reader, det) = reg.detect(path).map_err(|e| format!("detect: {e}"))?;
    let (reader2, det2) = reg
        .detect_input(input)
        .map_err(|e| format!("detect_input: {e}"))?;
    if det.format_id != det2.format_id {
        return Err(format!(
            "detected {} from the path but {} from the source",
            det.format_id, det2.format_id
        ));
    }
    let _ = reader2;
    let mut from_path = reader.open(path).map_err(|e| format!("open: {e}"))?;
    let mut from_source = match reader.open_input(input) {
        Ok(ds) => ds,
        Err(e) if !reader.reads_any_source() && e.exit_code() == 6 => return Ok(false),
        Err(e) => return Err(format!("open_input: {e}")),
    };
    if !reader.reads_any_source() {
        return Err("open_input succeeded but reads_any_source() is false".into());
    }
    let a = answers(from_path.as_mut());
    let b = answers(from_source.as_mut());
    if a != b {
        let (a, b) = (a.as_object().unwrap(), b.as_object().unwrap());
        let diff: Vec<String> = a
            .keys()
            .filter(|k| a.get(*k) != b.get(*k))
            .map(|k| {
                // Show both sides around the first difference.
                let (x, y) = (
                    a.get(k).map(ToString::to_string).unwrap_or_default(),
                    b.get(k).map(ToString::to_string).unwrap_or_default(),
                );
                let at = x
                    .bytes()
                    .zip(y.bytes())
                    .position(|(p, q)| p != q)
                    .unwrap_or(x.len().min(y.len()));
                let window = |s: &str| {
                    let start = s.floor_char_boundary(at.saturating_sub(120));
                    let end = s.ceil_char_boundary((at + 200).min(s.len()));
                    s[start..end].to_string()
                };
                format!(
                    "{k} (first difference at byte {at}): path …{}… / source …{}…",
                    window(&x),
                    window(&y)
                )
            })
            .collect();
        return Err(diff.join("\n    "));
    }
    Ok(true)
}

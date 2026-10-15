//! Helpers for commands that write files: refusing an output that is one of the inputs, and
//! writing through a temporary file that is read back before it is renamed into place.
//!
//! Every writer (export, extract, figures, previews, batch and index outputs, assay tables)
//! calls [`refuse_inputs`] before it writes, so a path that names the input in another way
//! (`data/../data/x.tiff`, a symbolic link, a hard link) is refused as well as the input's own
//! spelling.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, Result};

/// The phrase every "output is an input" error carries; the usage hint keys on it.
pub const OUTPUT_IS_INPUT: &str = "raw files are never modified";

/// True when `a` and `b` name the same file or directory: the same spelling, the same real
/// path (see [`real_path`]; `missing/../x` is `x`, because writing it creates `missing`), or
/// (on Unix) the same device and inode.
pub fn same_file(a: &Path, b: &Path) -> bool {
    if a == b || real_path(a) == real_path(b) {
        return true;
    }
    #[cfg(unix)]
    if let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) {
        use std::os::unix::fs::MetadataExt;
        return ma.dev() == mb.dev() && ma.ino() == mb.ino();
    }
    false
}

/// `p` with symbolic links and `..` resolved, also when `p` does not exist yet: the deepest
/// existing ancestor is resolved and the rest of `p` appended (`..` in the missing part is
/// resolved by name).
pub fn real_path(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(base) {
            let mut out = real;
            for c in rest.iter().rev() {
                match c {
                    std::path::Component::ParentDir => {
                        out.pop();
                    }
                    std::path::Component::CurDir => {}
                    c => out.push(c),
                }
            }
            return out;
        }
        match (base.parent(), base.components().next_back()) {
            (Some(parent), Some(last)) => {
                rest.push(last);
                base = parent;
            }
            _ => return abs,
        }
    }
}

/// True when `output` is an existing file inside the directory `dir` (a directory data set
/// such as a Bruker `.d`): replacing it would change the raw data.
fn inside(output: &Path, dir: &Path) -> bool {
    if !dir.is_dir() || !output.exists() {
        return false;
    }
    matches!(
        (std::fs::canonicalize(output), std::fs::canonicalize(dir)),
        (Ok(o), Ok(d)) if o.starts_with(&d)
    )
}

/// Refuse `output` when it is any of `inputs` (see [`same_file`]), or an existing file inside
/// an input that is a directory.
pub fn refuse_inputs<'a, I>(output: &Path, inputs: I) -> Result<()>
where
    I: IntoIterator<Item = &'a Path>,
{
    for input in inputs {
        if same_file(output, input) || inside(output, input) {
            return Err(Error::Usage(format!(
                "output {} is the input file {}; {OUTPUT_IS_INPUT}",
                output.display(),
                input.display()
            )));
        }
    }
    Ok(())
}

/// Refuse `output` when it is the data set opened from `input` or one of its member files.
pub fn refuse_dataset(output: &Path, input: &Path, ds: &dyn crate::Dataset) -> Result<()> {
    let members = ds.member_files();
    refuse_inputs(
        output,
        std::iter::once(input).chain(members.iter().map(PathBuf::as_path)),
    )
}

/// A temporary name next to `path` that no other writer in this or another process uses:
/// `.NAME.tmp-PID-N`, where N counts up within the process.
pub fn temp_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .map_or_else(|| "out".into(), |n| n.to_string_lossy().into_owned());
    path.with_file_name(format!(".{name}.tmp-{}-{n}", std::process::id()))
}

/// Write `bytes` to `path` through a temporary file that is read back and compared before it
/// is renamed into place. Refuses `path` when it is one of `inputs`, or when it exists and
/// `overwrite` is false; creates the folder when it is missing. The temporary file is removed
/// when any step fails.
pub fn write_verified(path: &Path, bytes: &[u8], overwrite: bool, inputs: &[&Path]) -> Result<()> {
    refuse_inputs(path, inputs.iter().copied())?;
    if path.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            path.display()
        )));
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let tmp = temp_path(path);
    let result = (|| {
        std::fs::write(&tmp, bytes).map_err(|e| Error::io(&tmp, e))?;
        let back = std::fs::read(&tmp).map_err(|e| Error::io(&tmp, e))?;
        if back != bytes {
            return Err(Error::Other(format!(
                "{}: read-back differs from what was written",
                tmp.display()
            )));
        }
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_is_refused_however_it_is_spelled() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let input = data.join("x.ome.tiff");
        std::fs::write(&input, b"raw").unwrap();
        let dotted = d.path().join("data/../data/x.ome.tiff");
        let dot = d.path().join("data/./x.ome.tiff");
        // writing through a folder that does not exist yet creates it, then lands on the input
        let missing = d.path().join("data/new/../x.ome.tiff");
        for out in [&input, &dotted, &dot, &missing] {
            let e = refuse_inputs(out, [input.as_path()]).unwrap_err();
            assert_eq!(e.exit_code(), 2, "{}", out.display());
            assert!(e.to_string().contains(OUTPUT_IS_INPUT));
        }
        #[cfg(unix)]
        {
            let link = d.path().join("link.tiff");
            std::os::unix::fs::symlink(&input, &link).unwrap();
            let hard = d.path().join("hard.tiff");
            std::fs::hard_link(&input, &hard).unwrap();
            assert!(refuse_inputs(&link, [input.as_path()]).is_err());
            assert!(refuse_inputs(&hard, [input.as_path()]).is_err());
        }
        let other = data.join("x.png");
        refuse_inputs(&other, [input.as_path()]).unwrap();
        // real_path resolves `..` in parts that do not exist yet
        assert_eq!(
            real_path(&d.path().join("data/new/../x.ome.tiff")),
            std::fs::canonicalize(&input).unwrap()
        );
        // a file inside a directory data set: new files are fine, existing ones are refused
        refuse_inputs(&other, [data.as_path()]).unwrap();
        assert!(refuse_inputs(&input, [data.as_path()]).is_err());
        // write_verified refuses it even with overwrite, and the input is untouched
        assert!(write_verified(&dotted, b"new", true, &[&input]).is_err());
        assert_eq!(std::fs::read(&input).unwrap(), b"raw");
    }

    #[test]
    fn writes_through_unique_temporary_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/out.csv");
        assert_ne!(temp_path(&p), temp_path(&p));
        write_verified(&p, b"a,b\n", false, &[]).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"a,b\n");
        assert!(write_verified(&p, b"x", false, &[]).is_err());
        write_verified(&p, b"x", true, &[]).unwrap();
        // nothing left behind
        let names: Vec<_> = std::fs::read_dir(d.path().join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }
}

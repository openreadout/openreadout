//! See tests/.
#![forbid(unsafe_code)]

/// ProteoWizard builds up to this one (3.0.20239 of August 2020 is the latest in the corpus)
/// leave Thermo peaks flagged as reference/background ions out of their exports; builds from
/// 3.0.20286 (October 2020) on keep them (`docs/formats/thermo-raw.md` § Centroid view).
pub const PWIZ_LAST_EXCLUDING_BUILD: u32 = 20_239;

/// ThermoRawFileParser up to this major.minor (1.3.4 is the latest in the corpus) leaves the
/// flagged peaks out too; 1.4.2 keeps them (`docs/formats/thermo-raw.md` § Centroid view).
pub const TRFP_LAST_EXCLUDING: (u32, u32) = (1, 3);

/// Whether an export written by `export_software` (the oracle's `[id, version]` or
/// `[type, name, version]` lists) left flagged Thermo peaks out: any ProteoWizard 2.x, or 3.0.x
/// up to [`PWIZ_LAST_EXCLUDING_BUILD`]; ThermoRawFileParser up to [`TRFP_LAST_EXCLUDING`].
pub fn export_excludes_flagged_peaks(export_software: &[Vec<String>]) -> bool {
    let trfp = export_software.iter().any(|s| {
        s.iter().any(|x| x == "ThermoRawFileParser")
            && s.last().is_some_and(|v| {
                let parts: Vec<u32> = v.split('.').filter_map(|p| p.parse().ok()).collect();
                matches!(parts.as_slice(), [major, minor, ..] if (*major, *minor) <= TRFP_LAST_EXCLUDING)
            })
    });
    trfp || export_software.iter().any(|s| {
        let name = s.first().map(String::as_str).unwrap_or_default();
        let pwiz = s
            .iter()
            .any(|x| x == "pwiz" || x == "ProteoWizard" || x == "ProteoWizard software")
            || name.starts_with("pwiz");
        let Some(version) = s.last() else {
            return false;
        };
        let parts: Vec<u32> = version.split('.').filter_map(|p| p.parse().ok()).collect();
        pwiz && match parts.as_slice() {
            [major, ..] if *major < 3 => true,
            [3, 0, build, ..] => *build <= PWIZ_LAST_EXCLUDING_BUILD,
            _ => false,
        }
    })
}

/// Committed ground truth on disk: `<name>.json`, or `<name>.json.gz` when the JSON exceeds 1 MiB
/// (`cargo xtask corpus compress`, `oracle/oracle_json.py`). Every path argument names the logical
/// `<name>.json`; the file on disk may be either, never both.
pub mod oracle_json {
    use std::io::Read;
    use std::path::{Path, PathBuf};

    fn gz(path: &Path) -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(".gz");
        PathBuf::from(s)
    }

    /// The file that holds `path` (`<name>.json`): itself or `<name>.json.gz`, or `None`.
    ///
    /// # Panics
    /// When both are present: one must go (`cargo xtask corpus compress` resolves it).
    #[must_use]
    pub fn resolve(path: &Path) -> Option<PathBuf> {
        let packed = gz(path);
        match (path.is_file(), packed.is_file()) {
            (true, true) => panic!(
                "both {} and {} exist; keep one (cargo xtask corpus compress)",
                path.display(),
                packed.display()
            ),
            (true, false) => Some(path.to_path_buf()),
            (false, true) => Some(packed),
            (false, false) => None,
        }
    }

    /// Whether `<name>.json` or `<name>.json.gz` exists.
    #[must_use]
    pub fn exists(path: &Path) -> bool {
        resolve(path).is_some()
    }

    /// The JSON text of `<name>.json` (or of `<name>.json.gz`, decompressed).
    ///
    /// # Errors
    /// Neither file exists, reading fails, or the gzip stream or its UTF-8 is invalid.
    pub fn read_to_string(path: &Path) -> std::io::Result<String> {
        let Some(p) = resolve(path) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} (nor {}.gz)", path.display(), path.display()),
            ));
        };
        if p.extension().is_some_and(|e| e == "gz") {
            let mut s = String::new();
            flate2::read::GzDecoder::new(std::fs::File::open(&p)?).read_to_string(&mut s)?;
            Ok(s)
        } else {
            std::fs::read_to_string(&p)
        }
    }

    /// The logical `<name><suffix>` paths in `dir` (suffix ending in `.json`), stored plain or as
    /// `.gz`, sorted. Empty when `dir` is unreadable.
    ///
    /// # Panics
    /// When a name is stored both ways.
    #[must_use]
    #[allow(clippy::case_sensitive_file_extension_comparisons)] // exact spellings, as written
    pub fn files(dir: &Path, suffix: &str) -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let gz_suffix = format!("{suffix}.gz");
        let mut out: Vec<PathBuf> = Vec::new();
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(stem) = name
                .strip_suffix(".gz")
                .filter(|_| name.ends_with(&gz_suffix))
            {
                out.push(p.with_file_name(stem));
            } else if name.ends_with(suffix) && p.is_file() {
                out.push(p);
            }
        }
        out.sort();
        if let Some(w) = out.windows(2).find(|w| w[0] == w[1]) {
            panic!(
                "both {} and {}.gz exist; keep one (cargo xtask corpus compress)",
                w[0].display(),
                w[0].display()
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::export_excludes_flagged_peaks;

    fn sw(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter()
            .map(|s| s.iter().map(|x| (*x).to_string()).collect())
            .collect()
    }

    #[test]
    fn pwiz_versions() {
        assert!(export_excludes_flagged_peaks(&sw(&[&[
            "pwiz",
            "3.0.19280"
        ]])));
        assert!(export_excludes_flagged_peaks(&sw(&[&[
            "conversion",
            "ProteoWizard",
            "2.2.3017"
        ]])));
        assert!(export_excludes_flagged_peaks(&sw(&[&[
            "pwiz",
            "3.0.20239"
        ]])));
        assert!(!export_excludes_flagged_peaks(&sw(&[&[
            "pwiz",
            "3.0.20286"
        ]])));
        assert!(!export_excludes_flagged_peaks(&sw(&[&[
            "Xcalibur", "2.4 SP1"
        ]])));
        assert!(!export_excludes_flagged_peaks(&sw(&[&[
            "pwiz_Reader_ABI",
            "3.0.24285"
        ]])));
        assert!(export_excludes_flagged_peaks(&sw(&[&[
            "ThermoRawFileParser",
            "1.3.4"
        ]])));
        assert!(!export_excludes_flagged_peaks(&sw(&[&[
            "ThermoRawFileParser",
            "1.4.2"
        ]])));
    }

    #[test]
    fn oracle_json_reads_plain_and_gz_under_one_name() {
        use std::io::Write;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.json"), "{\"a\":1}").unwrap();
        let mut enc = flate2::write::GzEncoder::new(
            std::fs::File::create(d.path().join("b.json.gz")).unwrap(),
            flate2::Compression::default(),
        );
        enc.write_all(b"{\"b\":2}").unwrap();
        enc.finish().unwrap();
        std::fs::write(d.path().join("c.xic.json"), "{}").unwrap();
        let files = super::oracle_json::files(d.path(), ".json");
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["a.json", "b.json", "c.xic.json"]);
        assert_eq!(
            super::oracle_json::read_to_string(&d.path().join("b.json")).unwrap(),
            "{\"b\":2}"
        );
        assert!(super::oracle_json::exists(&d.path().join("a.json")));
        assert!(!super::oracle_json::exists(&d.path().join("z.json")));
        std::fs::write(d.path().join("a.json.gz"), b"").unwrap();
        assert!(std::panic::catch_unwind(|| super::oracle_json::files(d.path(), ".json")).is_err());
    }
}

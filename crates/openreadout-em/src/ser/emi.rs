//! The `.emi` sidecar: only its embedded `<ObjectInfo>` XML document is read.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use openreadout_core::bytes::{find, latin1};
use openreadout_core::source::Fs;

use crate::util::Blob;

/// Largest `.emi` scanned for the XML document.
const MAX_EMI_SCAN: u64 = 512 << 20;
/// Bytes at the end of the `.emi` searched first.
const TAIL_SCAN: u64 = 16 << 20;

/// Metadata found in an `.emi` file.
#[derive(Debug, Clone, PartialEq)]
pub struct EmiInfo {
    pub path: PathBuf,
    /// The `<ObjectInfo>` document as text.
    pub xml: String,
    /// Byte offset of `<ObjectInfo>` in the `.emi`.
    pub offset: u64,
}

impl EmiInfo {
    /// Locate and extract `<ObjectInfo>…</ObjectInfo>`; `None` when the file has none.
    pub fn read(path: &Path) -> Option<EmiInfo> {
        Self::read_in(&Fs::local(), path)
    }

    /// [`EmiInfo::read`] in the namespace `fs`.
    pub(crate) fn read_in(fs: &Fs, path: &Path) -> Option<EmiInfo> {
        let mut blob = Blob::open(fs, path).ok()?;
        // The document sits after the (large) data copy in the files we have seen: try the tail first.
        let tail_start = blob.len.saturating_sub(TAIL_SCAN);
        let (base, bytes) = {
            let tail = blob.read_upto(tail_start, TAIL_SCAN).ok()?;
            if find(&tail, b"<ObjectInfo>").is_some() {
                (tail_start, tail)
            } else {
                (0, blob.read_upto(0, blob.len.min(MAX_EMI_SCAN)).ok()?)
            }
        };
        let start = find(&bytes, b"<ObjectInfo>")?;
        let end = find(&bytes[start..], b"</ObjectInfo>")? + start + b"</ObjectInfo>".len();
        let raw = &bytes[start..end];
        let start = start + usize::try_from(base).ok()?;
        let xml = match std::str::from_utf8(raw) {
            Ok(s) => s.to_string(),
            Err(_) => latin1(raw),
        };
        Some(EmiInfo {
            path: path.to_path_buf(),
            xml,
            offset: start as u64,
        })
    }

    /// `ExperimentalDescription` label/value/unit triples, in file order.
    pub fn description(&self) -> Vec<(String, String, String)> {
        let Ok(doc) = roxmltree::Document::parse(&self.xml) else {
            return Vec::new();
        };
        doc.descendants()
            .filter(|n| n.has_tag_name("ExperimentalDescription"))
            .flat_map(|n| n.descendants().filter(|d| d.has_tag_name("Data")))
            .map(|d| {
                let child = |name: &str| {
                    d.children()
                        .find(|c| c.has_tag_name(name))
                        .and_then(|c| c.text())
                        .unwrap_or("")
                        .trim()
                        .to_string()
                };
                (child("Label"), child("Value"), child("Unit"))
            })
            .collect()
    }

    /// Text of the first element named `name`.
    pub fn field(&self, name: &str) -> Option<String> {
        let doc = roxmltree::Document::parse(&self.xml).ok()?;
        doc.descendants()
            .find(|n| n.has_tag_name(name))
            .and_then(|n| n.text())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// True when the XML parses.
    pub fn well_formed(&self) -> bool {
        roxmltree::Document::parse(&self.xml).is_ok()
    }

    /// Our summary for `images[].extra.emi`.
    pub fn summary(&self) -> Value {
        let mut m = Map::new();
        for (label, value, unit) in self.description() {
            if label.is_empty() {
                continue;
            }
            m.insert(label, json!({"value": value, "unit": unit}));
        }
        json!({
            "file": self.path.file_name().map(|n| n.to_string_lossy().to_string()),
            "accelerating_voltage_v": self.field("AcceleratingVoltage").and_then(|v| v.parse::<f64>().ok()),
            "acquire_date": self.field("AcquireDate"),
            "experimental_description": Value::Object(m),
        })
    }
}

/// The `.emi` of a series file `<stem>_<n>.ser`: `<stem>.emi` in the same directory
/// (matched case-insensitively).
pub fn sidecar_of(ser: &Path) -> Option<PathBuf> {
    sidecar_of_in(&Fs::local(), ser)
}

/// [`sidecar_of`] in the namespace `fs`.
pub(crate) fn sidecar_of_in(fs: &Fs, ser: &Path) -> Option<PathBuf> {
    let stem = ser.file_stem()?.to_string_lossy().to_string();
    let base = stem.rsplit_once('_').map_or(stem.as_str(), |(b, n)| {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            b
        } else {
            stem.as_str()
        }
    });
    let dir = ser.parent().unwrap_or_else(|| Path::new("."));
    let want = format!("{base}.emi");
    let direct = dir.join(&want);
    if fs.is_file(&direct) {
        return Some(direct);
    }
    fs.read_dir(if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    })
    .ok()?
    .filter_map(std::result::Result::ok)
    .map(|e| e.path())
    .find(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&want))
    })
}

/// The series files `<stem>_<n>.ser` that belong to `<stem>.emi`, ordered by `n`.
pub fn series_of(emi: &Path) -> Vec<PathBuf> {
    series_of_in(&Fs::local(), emi)
}

/// [`series_of`] in the namespace `fs`.
pub(crate) fn series_of_in(fs: &Fs, emi: &Path) -> Vec<PathBuf> {
    let Some(stem) = emi.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return Vec::new();
    };
    let dir = emi.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!("{}_", stem.to_lowercase());
    let mut found: Vec<(u32, PathBuf)> = fs
        .read_dir(if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        })
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_lowercase();
            let n = name.strip_prefix(&prefix)?.strip_suffix(".ser")?;
            n.parse::<u32>().ok().map(|n| (n, p.clone()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_object_info_and_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let emi = dir.path().join("Scan.emi");
        let mut bytes = vec![0x4a, 0x4b, 0, 2, 9, 9];
        bytes.extend_from_slice(b"<ObjectInfo><ExperimentalConditions><MicroscopeConditions><AcceleratingVoltage>200000</AcceleratingVoltage></MicroscopeConditions></ExperimentalConditions><ExperimentalDescription><Root><Data><Label>Microscope</Label><Value>TEM 1</Value><Unit></Unit></Data></Root></ExperimentalDescription></ObjectInfo>");
        bytes.extend_from_slice(&[0, 1, 2]);
        std::fs::write(&emi, &bytes).unwrap();
        std::fs::write(dir.path().join("Scan_2.ser"), b"x").unwrap();
        std::fs::write(dir.path().join("Scan_1.ser"), b"x").unwrap();
        let info = EmiInfo::read(&emi).unwrap();
        assert!(info.well_formed());
        assert_eq!(info.offset, 6);
        assert_eq!(info.field("AcceleratingVoltage").as_deref(), Some("200000"));
        assert_eq!(
            info.description(),
            vec![("Microscope".into(), "TEM 1".into(), String::new())]
        );
        let s = series_of(&emi);
        assert_eq!(s.len(), 2);
        assert!(s[0].ends_with("Scan_1.ser"));
        assert_eq!(sidecar_of(&s[1]).unwrap(), emi);
    }
}

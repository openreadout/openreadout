//! Any other HDF5 file: `info --view structure` lists groups, datasets and attributes; `info`
//! summarizes; nothing is decoded as images or traces. See `docs/formats/hdf5.md`.

use std::path::{Path, PathBuf};

use openreadout_core::model::{CheckReport, DetectConfidence, FileInfo, FormatDescriptor, LsEntry};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Finding, Plane, Result};
use serde_json::{Value, json};

use crate::h5util::{H5Node, looks_like_hdf5, open_h5_in, walk};

/// Format id used on the command line and in JSON.
pub const HDF5_FORMAT_ID: &str = "hdf5";

/// Objects listed by `info --view structure` (and summarized by `info`) at most.
pub const MAX_NODES: usize = 100_000;

/// The generic HDF5 reader (a loose detector: registered after the HDF5-based formats).
#[derive(Debug, Default, Clone, Copy)]
pub struct Hdf5Reader;

impl FormatReader for Hdf5Reader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::HDF5)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: HDF5_FORMAT_ID.into(),
            name: "HDF5 (generic)".into(),
            vendor: "open standard (The HDF Group)".into(),
            extensions: vec!["h5".into(), "hdf5".into(), "he5".into(), "hdf".into()],
            family: "container".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::HDF5.confidence,
            known_gaps: vec![
                "Structure only: groups, datasets (shape, type) and attributes are listed; no dataset is decoded as an image, table or trace".into(),
                "HDF4 (`.hdf` without the HDF5 signature) is not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        looks_like_hdf5(head).then_some(Detection {
            format_id: HDF5_FORMAT_ID,
            confidence: DetectConfidence::Likely,
            note: Some("HDF5 signature; no more specific HDF5-based format recognised".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(Hdf5Dataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(Hdf5Dataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened HDF5 file.
pub struct Hdf5Dataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file: hdf5_pure::File,
    pub nodes: Vec<H5Node>,
    pub truncated_listing: bool,
}

impl std::fmt::Debug for Hdf5Dataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hdf5Dataset")
            .field("path", &self.path)
            .field("nodes", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

impl Hdf5Dataset {
    /// Open and walk the object tree (at most [`MAX_NODES`] objects).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let file = open_h5_in(fs, path, HDF5_FORMAT_ID)?;
        let (nodes, truncated_listing) =
            walk(&file, crate::h5util::walk_limit(fs, path, MAX_NODES));
        Ok(Hdf5Dataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file,
            nodes,
            truncated_listing,
        })
    }
}

impl Dataset for Hdf5Dataset {
    fn info(&self) -> Result<FileInfo> {
        let groups = self.nodes.iter().filter(|n| n.is_group).count();
        let datasets = self.nodes.len() - groups;
        let sb = self.file.superblock();
        let mut notes = vec![format!(
            "{groups} groups and {datasets} datasets{}; `info --view structure` lists them with shapes, types and attributes",
            if self.truncated_listing {
                " (listing capped)"
            } else {
                ""
            }
        )];
        let top: Vec<String> = self
            .nodes
            .iter()
            .filter(|n| n.path.matches('/').count() == 1 && n.path != "/")
            .map(|n| n.path.clone())
            .take(20)
            .collect();
        if !top.is_empty() {
            notes.push(format!("top level: {}", top.join(", ")));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: Hdf5Reader.descriptor(),
            format_version: Some(format!("superblock version {}", sb.version)),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let root = self
            .nodes
            .first()
            .map_or(Value::Null, |n| Value::Object(n.attributes.clone()));
        Ok(json!({"root_attributes": root}))
    }

    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .nodes
            .iter()
            .map(|n| LsEntry {
                kind: if n.is_group { "group" } else { "dataset" }.into(),
                name: n.path.clone(),
                offset: None,
                size: None,
                image: None,
                details: if n.is_group {
                    json!({"attributes": n.attributes})
                } else {
                    json!({"shape": n.shape, "dtype": n.dtype, "attributes": n.attributes})
                },
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            HDF5_FORMAT_ID,
            "pixel data of a generic HDF5 file",
            "No HDF5-based format this tool knows was recognised; `info --view structure` lists the datasets, which h5py or another HDF5 tool can read.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), HDF5_FORMAT_ID);
        r.performed("HDF5 superblock and the object tree parse (hdf5-pure)");
        r.performed("the file is at least as long as the superblock's end-of-file address");
        let flen = self.fs.metadata(&self.path).map_or(0, |m| m.len());
        let sb = self.file.superblock();
        let eof = sb.base_address.get().saturating_add(sb.eof_address);
        if eof > flen {
            r.push(
                Finding::error(
                    "truncated",
                    format!("file is {flen} bytes but its HDF5 superblock says it ends at {eof}"),
                )
                .at(flen),
            );
        }
        if self.truncated_listing {
            r.push(Finding::info(
                "listing_capped",
                format!(
                    "more than {MAX_NODES} objects, groups nested more than 32 deep (links may form a cycle) or paths over 4096 bytes; only part of the tree was walked"
                ),
            ));
        }
        Ok(r)
    }
}

//! Hamamatsu NDPI sets (`.ndpis`): a short text file that lists one NDPI per fluorescence
//! channel of a NanoZoomer scan. The set is read as one image with one channel per listed file;
//! each member is opened with the NDPI reader of this crate and channel `c` is image 0 of member
//! `c`. See `docs/formats/tiff.md` § NDPI sets and `docs/provenance/tiff.md` (2026-10-07).

use std::path::{Path, PathBuf};

use openreadout_core::model::{AttachmentInfo, CheckReport, FileInfo, Finding, ImageInfo, LsEntry};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::Region;
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Map, Value, json};

use crate::dataset::{TiffDataset, dir_of};
use crate::{FORMAT_ID, TiffReader};

/// The first line of every NDPI set.
pub const NDPIS_HEADER: &str = "[NanoZoomer Digital Pathology Image Set]";

/// Most files a set may list (a scanner has a handful of filter sets).
const MAX_MEMBERS: usize = 64;

/// Does `head` (the first bytes of a file) start like an NDPI set?
pub(crate) fn looks_like_ndpis(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    head.trim_ascii_start().starts_with(NDPIS_HEADER.as_bytes())
}

/// A parsed NDPI set: its `key=value` lines as written and the listed file names in channel
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct NdpisFile {
    /// Every `key=value` line, by key (values as text).
    pub keys: Map<String, Value>,
    /// The member file names, relative to the set's folder: `Image0`, `Image1`, ...
    pub images: Vec<String>,
}

/// Parse the text of an NDPI set. `None` when the first line is not the set header or the set
/// lists no file.
pub fn parse_ndpis(text: &str) -> Option<NdpisFile> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next()? != NDPIS_HEADER {
        return None;
    }
    let mut keys = Map::new();
    for line in lines {
        if let Some((k, v)) = line.split_once('=') {
            keys.insert(k.trim().to_string(), Value::String(v.trim().to_string()));
        }
    }
    let image = |k: usize| {
        keys.get(&format!("Image{k}"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let declared = keys
        .get("NoImages")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok());
    let images: Vec<String> = match declared {
        Some(n) => (0..n.min(MAX_MEMBERS)).map_while(image).collect(),
        None => (0..MAX_MEMBERS).map_while(image).collect(),
    };
    if images.is_empty() {
        return None;
    }
    Some(NdpisFile { keys, images })
}

/// One listed file: where it should be and, once opened, its NDPI data set.
struct Member {
    name: String,
    path: PathBuf,
    ds: Option<TiffDataset>,
    /// Why the member is not open: missing, or not a readable NDPI.
    error: Option<String>,
    missing: bool,
}

/// An open NDPI set (see the module documentation).
pub struct NdpisDataset {
    path: PathBuf,
    file_len: u64,
    set: NdpisFile,
    members: Vec<Member>,
    /// The first member that opened: the set's geometry, metadata and attachments.
    first: usize,
    image: ImageInfo,
    notes: Vec<String>,
}

/// The message for a listed file that is not there. `openreadout_core::Error::hint` turns
/// "of the data set is missing" into advice to copy the whole set.
fn missing_message(name: &str, path: &Path) -> String {
    format!(
        "file '{name}' of the data set is missing (the NDPI set lists it; looked for {})",
        path.display()
    )
}

impl std::fmt::Debug for NdpisDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NdpisDataset")
            .field("path", &self.path)
            .field("set", &self.set)
            .field("first", &self.first)
            .finish_non_exhaustive()
    }
}

impl NdpisDataset {
    /// Open the set at `input` and every member it lists that is there.
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let bytes = openreadout_core::limits::read_metadata_file(FORMAT_ID, fs, path)?;
        let text = String::from_utf8_lossy(&bytes);
        let set = parse_ndpis(&text).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("not an NDPI set (no \"{NDPIS_HEADER}\" line, or no ImageN= lines)"),
            )
        })?;
        let dir = dir_of(path);
        let members: Vec<Member> = set
            .images
            .iter()
            .map(|name| open_member(fs, &dir, name))
            .collect();
        let Some(first) = members.iter().position(|m| m.ds.is_some()) else {
            let m = &members[0];
            return Err(Error::corrupt(
                FORMAT_ID,
                match &m.error {
                    Some(e) if !m.missing => format!(
                        "none of the {} files this NDPI set lists could be opened ('{}': {e})",
                        members.len(),
                        m.name
                    ),
                    _ => missing_message(&m.name, &m.path),
                },
            ));
        };
        let mut ds = NdpisDataset {
            path: path.to_path_buf(),
            file_len: bytes.len() as u64,
            set,
            members,
            first,
            image: ImageInfo::new(0, 0, 0, openreadout_core::PixelType::Uint8),
            notes: Vec::new(),
        };
        ds.image = ds.combined_image()?;
        Ok(ds)
    }

    /// Image 0 of the first open member, with one channel per listed file. Members that
    /// opened must agree with it on size, sample type, Z and T.
    fn combined_image(&mut self) -> Result<ImageInfo> {
        let base = self.member_image(self.first)?;
        let mut channels = Vec::with_capacity(self.members.len());
        for c in 0..self.members.len() {
            let mut ch = if self.members[c].ds.is_some() {
                let im = self.member_image(c)?;
                let same = (im.size_x, im.size_y, im.size_z, im.size_t)
                    == (base.size_x, base.size_y, base.size_z, base.size_t)
                    && im.pixel_type == base.pixel_type
                    && im.samples_per_pixel == base.samples_per_pixel;
                if !same {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "the files of the NDPI set differ: '{}' is {} but '{}' is {}",
                            self.members[c].name,
                            geometry(&im),
                            self.members[self.first].name,
                            geometry(&base)
                        ),
                    ));
                }
                im.channels.into_iter().next().unwrap_or_default()
            } else {
                openreadout_core::model::ChannelInfo::default()
            };
            ch.index = c as u32;
            if ch.name.is_none() {
                ch.name = Some(stem_of(&self.members[c].name));
            }
            channels.push(ch);
        }
        let mut image = base;
        image.size_c = self.members.len() as u32;
        image.channels = channels;
        if image.name.is_none() {
            image.name = self
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string());
        }
        let files: Vec<&str> = self.members.iter().map(|m| m.name.as_str()).collect();
        image.extra.insert("files".into(), json!(files));
        let missing: Vec<&str> = self
            .members
            .iter()
            .filter(|m| m.ds.is_none())
            .map(|m| m.name.as_str())
            .collect();
        if !missing.is_empty() {
            self.notes.push(format!(
                "{} of the {} files this NDPI set lists could not be opened ({}); reads of their channels fail (see `check`)",
                missing.len(),
                self.members.len(),
                missing.join(", ")
            ));
        }
        self.notes.push(format!(
            "Hamamatsu NDPI set: {} file(s) as channels; each channel is its file's image as stored ({} sample(s) per pixel: NanoZoomer fluorescence files hold an RGB rendering in the channel's display colour)",
            self.members.len(),
            image.samples_per_pixel
        ));
        Ok(image.finish())
    }

    fn member_image(&self, c: usize) -> Result<ImageInfo> {
        let ds = self.members[c]
            .ds
            .as_ref()
            .ok_or_else(|| self.not_open(c))?;
        ds.info()?.images.into_iter().next().ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("'{}' holds no image", self.members[c].name),
            )
        })
    }

    fn not_open(&self, c: usize) -> Error {
        let m = &self.members[c];
        if m.missing {
            Error::corrupt(FORMAT_ID, missing_message(&m.name, &m.path))
        } else {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "'{}' of the NDPI set could not be opened: {}",
                    m.name,
                    m.error.as_deref().unwrap_or("unknown error")
                ),
            )
        }
    }

    /// The member holding channel `index.c` of image 0, and the plane index inside it.
    fn route(&mut self, image: u32, index: PlaneIndex) -> Result<(&mut TiffDataset, PlaneIndex)> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image {image} out of range (the NDPI set has 1 image)"
            )));
        }
        let c = index.c as usize;
        if c >= self.members.len() {
            return Err(Error::Usage(format!(
                "channel {c} out of range (the NDPI set has {} channels, 0..{})",
                self.members.len(),
                self.members.len()
            )));
        }
        if self.members[c].ds.is_none() {
            return Err(self.not_open(c));
        }
        let ds = self.members[c].ds.as_mut().expect("checked above");
        Ok((ds, PlaneIndex { c: 0, ..index }))
    }

    fn first_ds(&self) -> &TiffDataset {
        self.members[self.first]
            .ds
            .as_ref()
            .expect("the first member is open")
    }

    /// `check` (or its header-only form) of every member, findings prefixed with the file name;
    /// a missing member is `missing_file`.
    fn check_members(&mut self, headers_only: bool) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        rep.performed(format!(
            "NDPI set: the {} listed files exist, open as NDPI and agree on size, sample type, Z and T",
            self.members.len()
        ));
        for m in &mut self.members {
            let Some(ds) = m.ds.as_mut() else {
                let (code, msg) = if m.missing {
                    ("missing_file", missing_message(&m.name, &m.path))
                } else {
                    (
                        "unreadable_file",
                        format!(
                            "'{}': {}",
                            m.name,
                            m.error.as_deref().unwrap_or("not readable")
                        ),
                    )
                };
                rep.push(Finding::error(code, msg));
                continue;
            };
            let sub = if headers_only {
                ds.check_headers()?
            } else {
                ds.check()?
            };
            for p in sub.checks_performed {
                let p = format!("each listed file: {p}");
                if !rep.checks_performed.contains(&p) {
                    rep.performed(p);
                }
            }
            for mut f in sub.findings {
                f.message = format!("'{}': {}", m.name, f.message);
                rep.push(f);
            }
        }
        Ok(rep)
    }
}

/// Open one listed file: missing, not an NDPI, or open.
fn open_member(fs: &Fs, dir: &Path, name: &str) -> Member {
    let path = dir.join(name);
    let mut m = Member {
        name: name.to_string(),
        path: path.clone(),
        ds: None,
        error: None,
        missing: false,
    };
    if !fs.is_file(&path) {
        m.missing = true;
        return m;
    }
    match TiffDataset::open(&Input::new(path, fs.clone())) {
        Ok(ds) => m.ds = Some(ds),
        Err(e) => m.error = Some(e.to_string()),
    }
    m
}

/// Size, Z, T and samples of an image, for the message when members differ.
fn geometry(im: &ImageInfo) -> String {
    format!(
        "{} x {} (z {}, t {}, {} x {})",
        im.size_x,
        im.size_y,
        im.size_z,
        im.size_t,
        im.pixel_type.ome_name(),
        im.samples_per_pixel
    )
}

/// A member's file name without its `.ndpi` extension.
fn stem_of(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    match lower.strip_suffix(".ndpi") {
        Some(s) => base[..s.len()].trim_end().to_string(),
        None => base.to_string(),
    }
}

impl Dataset for NdpisDataset {
    fn info(&self) -> Result<FileInfo> {
        let first = self.first_ds().info()?;
        let mut notes = vec!["TIFF sub-format: hamamatsu-ndpis".to_string()];
        notes.extend(self.notes.iter().cloned());
        notes.extend(
            first
                .notes
                .into_iter()
                .filter(|n| !n.starts_with("TIFF sub-format: ")),
        );
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: TiffReader.descriptor(),
            format_version: first.format_version,
            images: vec![self.image.clone()],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: self.image.plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut members = Vec::with_capacity(self.members.len());
        for m in &self.members {
            let v = match &m.ds {
                Some(ds) => ds.vendor_metadata()?,
                None => Value::Null,
            };
            members.push(json!({"file": m.name, "vendor": v}));
        }
        Ok(json!({"ndpis": Value::Object(self.set.keys.clone()), "members": members}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = self.first_ds().provenance();
        for (k, s) in [
            ("images[].size_c", Source::Inferred),
            ("images[].channels[].name", Source::PriorArt),
            ("images[].extra.files", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: crate::files::file_name_of(&self.path),
            offset: None,
            size: Some(self.file_len),
            image: None,
            details: json!({"ndpis": Value::Object(self.set.keys.clone())}),
        }];
        for (c, m) in self.members.iter().enumerate() {
            out.push(LsEntry {
                kind: "file".into(),
                name: m.name.clone(),
                offset: None,
                size: m.ds.as_ref().map(|d| d.file_len),
                image: Some(0),
                details: json!({"channel": c, "present": !m.missing, "error": m.error}),
            });
            if let Some(ds) = &m.ds {
                for mut e in ds.entries()? {
                    e.name = format!("{}: {}", m.name, e.name);
                    out.push(e);
                }
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let (ds, idx) = self.route(image, index)?;
        ds.read_plane(0, idx)
    }

    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        let (ds, idx) = self.route(image, index)?;
        ds.read_plane_level(0, idx, level)
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        let (ds, idx) = self.route(image, index)?;
        ds.read_region(0, idx, level, region)
    }

    fn check(&mut self) -> Result<CheckReport> {
        self.check_members(false)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        self.check_members(true)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        self.members
            .iter()
            .filter(|m| !m.missing)
            .map(|m| m.path.clone())
            .collect()
    }

    /// The first open member's attachments (the macro and map images of the slide).
    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        self.first_ds().attachments()
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let first = self.first;
        self.members[first]
            .ds
            .as_mut()
            .expect("the first member is open")
            .read_attachment(index)
    }

    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        self.first_ds().assurance_observations()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: &str = "[NanoZoomer Digital Pathology Image Set]\r\nNoImages=3\r\nImage0=test3-DAPI 2 (387) .ndpi\r\nImage1=test3-FITC 2 (485).ndpi\r\nImage2=test3-TRITC 2 (560).ndpi\r\n";

    #[test]
    fn parses_the_listed_files_in_order() {
        let s = parse_ndpis(SET).unwrap();
        assert_eq!(
            s.images,
            [
                "test3-DAPI 2 (387) .ndpi",
                "test3-FITC 2 (485).ndpi",
                "test3-TRITC 2 (560).ndpi"
            ]
        );
        assert_eq!(s.keys["NoImages"], "3");
        assert!(looks_like_ndpis(SET.as_bytes()));
        assert!(looks_like_ndpis(format!("\u{feff}{SET}").as_bytes()));
        assert_eq!(stem_of(&s.images[0]), "test3-DAPI 2 (387)");
        assert_eq!(stem_of("sub\\x.NDPI"), "x");
    }

    #[test]
    fn without_no_images_every_consecutive_image_key_counts() {
        let s = parse_ndpis("[NanoZoomer Digital Pathology Image Set]\nImage0=a.ndpi\nImage1=b.ndpi\nImage3=d.ndpi\n").unwrap();
        assert_eq!(s.images, ["a.ndpi", "b.ndpi"]);
        // NoImages larger than the keys present stops at the first gap.
        let s =
            parse_ndpis("[NanoZoomer Digital Pathology Image Set]\nNoImages=5\nImage0=a.ndpi\n")
                .unwrap();
        assert_eq!(s.images, ["a.ndpi"]);
    }

    #[test]
    fn other_text_is_not_a_set() {
        assert!(parse_ndpis("[Other]\nImage0=a.ndpi\n").is_none());
        assert!(parse_ndpis("[NanoZoomer Digital Pathology Image Set]\nNoImages=0\n").is_none());
        assert!(parse_ndpis("").is_none());
        assert!(!looks_like_ndpis(b"II*\0"));
    }
}

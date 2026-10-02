//! The two ways FluoView stores the same set of files: OIF (a main `.oif` settings file plus a
//! `<name>.oif.files` folder next to it) and OIB (every file packed into one MS-CFB compound
//! file, with `OibInfo.txt` mapping stream names back to file names). Layout:
//! `docs/formats/oif.md` § Containers.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use openreadout_core::cfb::{Cfb, CfbEntry};
use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use crate::settings::{MAX_SETTINGS_BYTES, Settings};

/// Which container a data set comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// `.oib`: one compound file.
    Oib,
    /// `.oif`: settings file plus `.oif.files` folder.
    Oif,
}

impl StoreKind {
    /// The format id (`oib` or `oif`).
    pub fn format_id(self) -> &'static str {
        match self {
            StoreKind::Oib => crate::OIB_FORMAT_ID,
            StoreKind::Oif => crate::OIF_FORMAT_ID,
        }
    }
}

/// Where a member's bytes live.
#[derive(Debug, Clone)]
pub enum MemberSource {
    /// A stream of the compound file.
    Stream(CfbEntry),
    /// A file in the `.oif.files` folder.
    File(PathBuf),
}

/// One file of the data set (TIFF plane, property file, LUT, ROI, thumbnail).
#[derive(Debug, Clone)]
pub struct Member {
    /// File name as FluoView wrote it, e.g. `s_C001Z002.tif` (the folder part is dropped: a data
    /// set has one storage folder).
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Where the bytes are.
    pub source: MemberSource,
}

/// An opened OIB compound file or OIF folder.
#[derive(Debug)]
pub struct Store {
    /// Container kind.
    pub kind: StoreKind,
    /// The path that was opened (`.oib` or `.oif`).
    pub path: PathBuf,
    /// File name of the main settings file (`<name>.oif`).
    pub main_name: String,
    /// The main settings file.
    pub main: Settings,
    /// `OibInfo.txt` (OIB only).
    pub oib_info: Option<Settings>,
    /// Members by lower-case file name.
    pub members: BTreeMap<String, Member>,
    /// Structural problems found while opening (missing streams, missing folder, ...).
    pub problems: Vec<String>,
    cfb: Option<Cfb>,
    /// Where the container and its folder are read from.
    pub(crate) fs: Fs,
}

fn corrupt(kind: StoreKind, d: impl Into<String>) -> Error {
    Error::corrupt(kind.format_id(), d.into())
}

fn base_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

impl Store {
    /// Open either container; `kind` comes from the reader that recognised the file.
    pub fn open(path: &Path, kind: StoreKind) -> Result<Store> {
        Self::open_in(&Fs::local(), path, kind)
    }

    /// [`Store::open`] in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path, kind: StoreKind) -> Result<Store> {
        match kind {
            StoreKind::Oib => Self::open_oib(fs, path),
            StoreKind::Oif => Self::open_oif(fs, path),
        }
    }

    fn open_oib(fs: &Fs, path: &Path) -> Result<Store> {
        let kind = StoreKind::Oib;
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let cfb = Cfb::open(&mut f, path, kind.format_id())?;
        let info_entry = cfb.stream("OibInfo.txt").cloned().ok_or_else(|| {
            if cfb.problems.is_empty() {
                Error::unsupported(
                    kind.format_id(),
                    "a compound file without an OibInfo.txt stream",
                    "This is an OLE2 compound file but not an Olympus OIB (no OibInfo.txt).",
                )
            } else {
                // a damaged directory: the stream list itself cannot be trusted
                corrupt(
                    kind,
                    format!(
                        "no OibInfo.txt stream reachable in a damaged compound file: {}",
                        cfb.problems.join("; ")
                    ),
                )
            }
        })?;
        let info_bytes = cfb.read(&mut f, path, &info_entry, MAX_SETTINGS_BYTES)?;
        let info = Settings::parse(&info_bytes)
            .ok_or_else(|| corrupt(kind, "OibInfo.txt is not a settings file"))?;
        let save = info
            .section("OibSaveInfo")
            .ok_or_else(|| corrupt(kind, "OibInfo.txt has no [OibSaveInfo] section"))?
            .clone();
        let main_stream = save
            .text("MainFileName")
            .ok_or_else(|| corrupt(kind, "OibInfo.txt names no MainFileName"))?;
        let mut problems = cfb.problems.clone();
        let mut members = BTreeMap::new();
        let mut main_name = None;
        let mut main_entry = None;
        for (key, value) in &save.entries {
            if !key.starts_with("Stream") {
                continue;
            }
            let value = crate::settings::unquote(value);
            // `Storage00001/s_C001.tif` → the stream `Stream00001` sits in storage `Storage00001`
            let cfb_path = match value.rsplit_once('/') {
                Some((folder, _)) => format!("{folder}/{key}"),
                None => key.clone(),
            };
            let entry = cfb.stream(&cfb_path).cloned().or_else(|| {
                cfb.entries
                    .iter()
                    .find(|e| e.is_stream && base_name(&e.path).eq_ignore_ascii_case(key))
                    .cloned()
            });
            let Some(entry) = entry else {
                problems.push(format!(
                    "OibInfo.txt lists {key} ({value}) but the compound file has no such stream"
                ));
                continue;
            };
            if *key == main_stream {
                main_name = Some(value.to_string());
                main_entry = Some(entry);
                continue;
            }
            let name = base_name(value).to_string();
            members.insert(
                name.to_ascii_lowercase(),
                Member {
                    name,
                    size: entry.size,
                    source: MemberSource::Stream(entry),
                },
            );
        }
        let main_entry = main_entry.ok_or_else(|| {
            corrupt(
                kind,
                format!("the main settings stream {main_stream} named by OibInfo.txt is missing"),
            )
        })?;
        let main_bytes = cfb.read(&mut f, path, &main_entry, MAX_SETTINGS_BYTES)?;
        let main = Settings::parse(&main_bytes)
            .ok_or_else(|| corrupt(kind, "the main .oif stream is not a settings file"))?;
        Ok(Store {
            kind,
            path: path.to_path_buf(),
            main_name: main_name.unwrap_or_default(),
            main,
            oib_info: Some(info),
            members,
            problems,
            cfb: Some(cfb),
            fs: fs.clone(),
        })
    }

    fn open_oif(fs: &Fs, path: &Path) -> Result<Store> {
        let kind = StoreKind::Oif;
        let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
        if meta.len() > MAX_SETTINGS_BYTES {
            return Err(corrupt(
                kind,
                format!(
                    "{} bytes is too large for an .oif settings file",
                    meta.len()
                ),
            ));
        }
        let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
        let main = Settings::parse(&bytes).ok_or_else(|| corrupt(kind, "not a settings file"))?;
        let main_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let storage = path.with_file_name(format!("{main_name}.files"));
        let mut problems = Vec::new();
        let mut members = BTreeMap::new();
        match fs.read_dir(&storage) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let p = e.path();
                    let Ok(m) = e.metadata() else { continue };
                    if !m.is_file() {
                        continue;
                    }
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue; // .DS_Store and similar
                    }
                    members.insert(
                        name.to_ascii_lowercase(),
                        Member {
                            name,
                            size: m.len(),
                            source: MemberSource::File(p),
                        },
                    );
                }
            }
            Err(_) => problems.push(format!(
                "the data folder {main_name}.files next to the .oif file is missing: no planes can be read"
            )),
        }
        Ok(Store {
            kind,
            path: path.to_path_buf(),
            main_name,
            main,
            oib_info: None,
            members,
            problems,
            cfb: None,
            fs: fs.clone(),
        })
    }

    /// The member with this file name (case-insensitive).
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.get(&name.to_ascii_lowercase())
    }

    /// Read up to `limit` bytes of a member.
    pub fn read(&self, m: &Member, limit: u64) -> Result<Vec<u8>> {
        match &m.source {
            MemberSource::Stream(e) => {
                let cfb = self
                    .cfb
                    .as_ref()
                    .ok_or_else(|| corrupt(self.kind, "compound file not open"))?;
                let mut f = self
                    .fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?;
                cfb.read(&mut f, &self.path, e, limit)
            }
            MemberSource::File(p) => {
                let f = self.fs.open(p).map_err(|e| Error::io(p, e))?;
                let mut out = Vec::new();
                f.take(limit)
                    .read_to_end(&mut out)
                    .map_err(|e| Error::io(p, e))?;
                Ok(out)
            }
        }
    }

    /// Read and parse a settings member (`.pty`, `.roi`, `.lut`).
    pub fn settings(&self, name: &str) -> Option<Settings> {
        let m = self.member(name)?;
        Settings::parse(&self.read(m, MAX_SETTINGS_BYTES).ok()?)
    }

    /// The path a member is known by in messages (`file.oib:s_C001.tif` or the file path).
    pub fn label(&self, m: &Member) -> PathBuf {
        match &m.source {
            MemberSource::Stream(_) => PathBuf::from(format!("{}:{}", self.path.display(), m.name)),
            MemberSource::File(p) => p.clone(),
        }
    }
}

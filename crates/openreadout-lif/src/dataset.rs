//! `Dataset` implementation: normalized metadata, listing, plane reads, integrity checks.
//!
//! One dataset can span several files: a `.lif` and its `.lifext` sidecar, or an XLEF/XLCF tree
//! of XLIF files whose pixels live in `.lof` files. Every image node is resolved at open time to
//! a [`Storage`] (byte ranges in files); plane reads go through that.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use openreadout_core::limits::plane_len;
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry, MosaicInfo,
    ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::filetime_to_iso8601;
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::json;

use crate::container::{ContainerKind, LifFile};
use crate::mosaic::{MosaicLayout, layout};
use crate::storage::{FileTable, Segment, Storage};
use crate::xlef::{XmlContainer, XmlKind, resolve_reference_in};
use crate::xml::{Axis, ChannelDesc, DimensionDesc, ImageNode, TileScan, parse_images};
use crate::{FORMAT_ID, LifReader};

/// Deepest XLEF → XLCF → XLIF nesting we follow.
const MAX_DEPTH: usize = 16;

/// Why an image's pixels cannot be located.
#[derive(Debug, Clone)]
enum StorageProblem {
    /// The XML names a memory block that is not in the file (or its sidecar).
    MissingBlock(String),
    /// An XLIF frame file is absent.
    MissingFile(String),
    /// An XLIF frame is stored in a file type we do not decode (TIFF, JPEG, PNG, BMP).
    UnsupportedFrames(String),
}

/// A binary container that is part of this dataset.
#[derive(Debug)]
struct Loaded {
    file: usize,
    lif: LifFile,
    /// `main`, `sidecar`, `referenced` or `parent`.
    role: &'static str,
}

/// One image node with its resolved storage and display path.
#[derive(Debug)]
struct Entry {
    node: ImageNode,
    display: String,
    storage: std::result::Result<Storage, StorageProblem>,
    /// Index into `LifDataset::containers` of the file whose XML described this node.
    container: Option<usize>,
    /// Tile scan to place mosaic tiles with (own, or inherited from a LIFEXT parent image).
    tile_scan: Option<TileScan>,
    /// Display path of the parent image, for LIFEXT nodes whose `.lif` is present.
    parent_image: Option<String>,
}

/// One exposed image = one image node, or one element of its split axes (rotation, ...).
#[derive(Debug, Clone, Copy)]
struct Exposed {
    entry: usize,
    split: u32,
}

/// How samples are stored and how we return them.
#[derive(Debug, Clone, Copy)]
struct SampleFormat {
    pixel_type: PixelType,
    stored_bytes: u64,
    /// IEEE half floats, widened to f32 on read.
    half: bool,
}

/// Per-node plane geometry after mapping lambda to C, slices to T and other axes to images.
#[derive(Debug, Clone)]
struct Geometry {
    sx: u32,
    sy: u32,
    size_c: u32,
    size_z: u32,
    size_t: u32,
    lambda: u32,
    /// Axes folded into T, innermost first: (bytes_inc, count).
    folded: Vec<(Axis, u64, u32)>,
    /// Axes split into separate images, innermost first: (axis, bytes_inc, count).
    split: Vec<(Axis, u64, u32)>,
    split_count: u32,
    mosaic: u32,
    fmt: SampleFormat,
}

/// An opened LIF-family dataset.
#[derive(Debug)]
pub struct LifDataset {
    path: PathBuf,
    /// What the user opened: `lif`, `lifext`, `lof`, `xlif`, `xlef`, `xlcf` or `xllf`.
    kind: &'static str,
    files: FileTable,
    containers: Vec<Loaded>,
    xml_containers: Vec<XmlContainer>,
    entries: Vec<Entry>,
    exposed: Vec<Exposed>,
    /// LIFEXT sidecar images of a `.lif` (listed, checked, not exposed as images).
    sidecar_nodes: Vec<ImageNode>,
    /// Problems found while following references (missing files, cycles, ...).
    open_findings: Vec<Finding>,
    root_xml: String,
    format_version: Option<u32>,
}

/// Detection window `[start, end]` (nm) of λ step `l` whose λ coordinate is `start_nm`: the
/// hardware setting's `LambdaEmission` windows when they describe this λ dimension (same count,
/// first window starting within 0.5 nm of the λ origin), else unknown.
fn lambda_band(
    w: Option<crate::xml::LambdaWindows>,
    count: u32,
    l: u32,
    start_nm: f64,
) -> Option<[f64; 2]> {
    let w = w?;
    let first = start_nm - w.step_nm * f64::from(l);
    (w.step_count == count && (first - w.begin_nm).abs() <= 0.5).then(|| {
        let a = w.begin_nm + w.step_nm * f64::from(l);
        [a, a + w.bandwidth_nm]
    })
}

fn sample_format(ch: &ChannelDesc) -> Result<SampleFormat> {
    let f = |pixel_type: PixelType, stored_bytes: u64, half: bool| SampleFormat {
        pixel_type,
        stored_bytes,
        half,
    };
    Ok(match (ch.resolution_bits, ch.data_type) {
        (1..=8, 0) => f(PixelType::Uint8, 1, false),
        (9..=16, 0) => f(PixelType::Uint16, 2, false),
        (17..=32, 0) => f(PixelType::Uint32, 4, false),
        (16, 1) => f(PixelType::Float, 2, true),
        (32, 1) => f(PixelType::Float, 4, false),
        (64, 1) => f(PixelType::Double, 8, false),
        (bits, dt) => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("channel resolution {bits} bits with data type {dt}"),
                "Only 8/12/16/32-bit integer and 16/32/64-bit float channels are supported.",
            ));
        }
    })
}

// Half-float widening is shared with the EM readers (`openreadout_core::pixel`).
use openreadout_core::pixel::widen_half;

/// Most tiles accepted in one tile scan (real scans have up to a few thousand).
const MAX_TILES: u32 = 1 << 20;

fn geometry(node: &ImageNode) -> Result<Geometry> {
    let ch0 = node.channels.first().ok_or_else(|| {
        Error::corrupt(FORMAT_ID, format!("image '{}' has no channels", node.path))
    })?;
    let fmt = sample_format(ch0)?;
    let lambda = node.count(Axis::Lambda);
    let size_c = u32::try_from(node.channels.len())
        .ok()
        .and_then(|n| n.checked_mul(lambda))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "channel count overflows"))?;
    // Counts divide plane indices below; a corrupt `NumberOfElements="0"` counts as 1.
    let mut folded = vec![(Axis::T, node.inc(Axis::T), node.count(Axis::T).max(1))];
    let mut split = Vec::new();
    for d in &node.dimensions {
        match d.axis {
            Axis::XtSlices | Axis::TSlices if d.count > 1 => {
                folded.push((d.axis, d.bytes_inc, d.count));
            }
            Axis::Rotation | Axis::Other(_) if d.count > 1 => {
                split.push((d.axis, d.bytes_inc, d.count));
            }
            _ => {}
        }
    }
    // Fold XT slices inside T slices: T innermost, then XT slices, then T slices.
    folded.sort_by_key(|(a, _, _)| match a {
        Axis::T => 0,
        Axis::XtSlices => 1,
        _ => 2,
    });
    let size_t = folded
        .iter()
        .try_fold(1u32, |acc, (_, _, n)| acc.checked_mul(*n))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "time/slice count overflows"))?;
    let split_count = split
        .iter()
        .try_fold(1u32, |acc, (_, _, n)| acc.checked_mul(*n))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "split-axis count overflows"))?;
    let mosaic = node.count(Axis::Mosaic);
    if mosaic > MAX_TILES {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "image '{}' declares {mosaic} mosaic tiles (limit {MAX_TILES})",
                node.path
            ),
        ));
    }
    Ok(Geometry {
        sx: node.count(Axis::X),
        sy: node.count(Axis::Y),
        size_c,
        size_z: node.count(Axis::Z),
        size_t,
        lambda,
        folded,
        split,
        split_count,
        mosaic,
        fmt,
    })
}

/// Byte offset (within the memory block) of sample (0, 0) of one tile plane.
#[allow(clippy::many_single_char_names)]
fn plane_offset(
    node: &ImageNode,
    g: &Geometry,
    c: u32,
    z: u32,
    t: u32,
    split: u32,
    tile: u32,
) -> Option<u64> {
    let lambda = g.lambda.max(1);
    let ch = node.channels.get((c / lambda) as usize)?;
    let l = c % lambda;
    let mut off = ch
        .bytes_inc
        .checked_add(u64::from(l).checked_mul(node.inc(Axis::Lambda))?)?
        .checked_add(u64::from(z).checked_mul(node.inc(Axis::Z))?)?
        .checked_add(u64::from(tile).checked_mul(node.inc(Axis::Mosaic))?)?;
    let mut rest = t;
    for (_, inc, n) in &g.folded {
        let i = rest % n;
        rest /= n;
        off = off.checked_add(u64::from(i).checked_mul(*inc)?)?;
    }
    let mut rest = split;
    for (_, inc, n) in &g.split {
        let i = rest % n;
        rest /= n;
        off = off.checked_add(u64::from(i).checked_mul(*inc)?)?;
    }
    Some(off)
}

/// Bytes a node's geometry needs from its memory block (largest address + one sample).
fn bytes_needed(node: &ImageNode, bps: u64) -> u64 {
    let mut need: u64 = 0;
    for d in &node.dimensions {
        need =
            need.saturating_add(u64::from(d.count.saturating_sub(1)).saturating_mul(d.bytes_inc));
    }
    need.saturating_add(node.channels.iter().map(|c| c.bytes_inc).max().unwrap_or(0))
        .saturating_add(bps)
}

/// Physical pixel size in µm. A step of 0, or of 1 cm or more, is not a calibration: LAS X
/// writes pixel indices labelled "m" for FLIM result images (Length = N - 1), and zero-length
/// Z stacks; both are reported as unknown.
fn um_step(d: Option<&DimensionDesc>) -> Option<f64> {
    d.filter(|d| d.unit.eq_ignore_ascii_case("m"))
        .and_then(DimensionDesc::step)
        .filter(|m| *m > 0.0 && *m < 1e-2)
        .map(|m| m * 1e6)
}

fn metre_step(d: Option<&DimensionDesc>) -> Option<f64> {
    d.filter(|d| d.unit.eq_ignore_ascii_case("m"))
        .and_then(DimensionDesc::step)
        .filter(|s| *s > 0.0)
}

fn lut_color(lut: &str) -> Option<&'static str> {
    match lut.to_ascii_lowercase().as_str() {
        "red" => Some("#FF0000"),
        "green" => Some("#00FF00"),
        "blue" => Some("#0000FF"),
        "cyan" => Some("#00FFFF"),
        "magenta" => Some("#FF00FF"),
        "yellow" => Some("#FFFF00"),
        "gray" | "grey" | "white" => Some("#FFFFFF"),
        _ => None,
    }
}

/// "LAS X [ BETA ] 3.1.0.15531" → ("LAS X [ BETA ]", "3.1.0.15531")
fn split_software(s: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(s) = s else { return (None, None) };
    let s = s.trim();
    if let Some(idx) = s.rfind(' ') {
        let (name, ver) = s.split_at(idx);
        if ver.trim().chars().all(|c| c.is_ascii_digit() || c == '.') {
            return (Some(name.trim().to_string()), Some(ver.trim().to_string()));
        }
    }
    (Some(s.to_string()), None)
}

/// For an interleaved RGB image (three channels tagged red 1, green 2, blue 3 at sample
/// positions 0..3 of a pixel): which sample of an RGB frame image goes to each position. LAS X
/// keeps its blue, green, red memory order in the XLIF while the frame image is ordinary RGB.
fn rgb_remap(channels: &[ChannelDesc]) -> Option<Vec<usize>> {
    if channels.len() != 3 {
        return None;
    }
    let bps: u64 = if channels.iter().all(|c| c.resolution_bits <= 8) {
        1
    } else {
        2
    };
    let mut map = vec![usize::MAX; 3];
    for c in channels {
        let pos = usize::try_from(c.bytes_inc / bps).ok()?;
        if c.bytes_inc % bps != 0 || !(1..=3).contains(&c.channel_tag) || pos > 2 {
            return None;
        }
        map[pos] = c.channel_tag as usize - 1;
    }
    let mut seen = map.clone();
    seen.sort_unstable();
    (seen == [0, 1, 2]).then_some(map)
}

fn file_ext(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".ome.tif") || name.ends_with(".ome.tiff") {
        return "ome.tif".into();
    }
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// `<stem>.lifext` next to a `.lif`, matched case-insensitively.
fn sidecar_of(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let want = format!("{stem}.lifext");
    fs.find_in_dir(dir, &want).filter(|p| fs.is_file(p))
}

/// The `.lif` a `.lifext` belongs to (same stem).
fn parent_of_sidecar(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem()?.to_string_lossy().to_string();
    for ext in ["lif", "LIF", "Lif"] {
        let p = dir.join(format!("{stem}.{ext}"));
        if fs.is_file(&p) {
            return Some(p);
        }
    }
    None
}

struct Builder {
    files: FileTable,
    containers: Vec<Loaded>,
    xml_containers: Vec<XmlContainer>,
    entries: Vec<Entry>,
    findings: Vec<Finding>,
    visited: HashSet<PathBuf>,
    fs: Fs,
}

/// A path's canonical form on the local disk (cycle detection); virtual paths as they are.
fn canonical(fs: &Fs, p: &Path) -> PathBuf {
    if fs.is_local() {
        p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
    } else {
        p.to_path_buf()
    }
}

impl Builder {
    fn new(fs: &Fs) -> Self {
        Builder {
            files: FileTable::new(fs.clone()),
            containers: Vec::new(),
            xml_containers: Vec::new(),
            entries: Vec::new(),
            findings: Vec::new(),
            visited: HashSet::new(),
            fs: fs.clone(),
        }
    }

    fn block_storage(&mut self, ci: usize, id: &str) -> Option<Storage> {
        let c = &self.containers[ci];
        let b = c.lif.block(id)?;
        let avail = c.lif.file_len.saturating_sub(b.data_offset);
        Some(Storage::single(
            c.file,
            b.data_offset,
            b.data_len.min(avail),
        ))
    }

    /// Open a binary container and add its image nodes (with `prefix` on their display paths).
    fn add_binary(
        &mut self,
        path: &Path,
        prefix: Option<&str>,
        role: &'static str,
    ) -> Result<usize> {
        let lif = LifFile::open_in(&self.fs, path)?;
        let nodes = parse_images(&lif.xml)?;
        let file = self.files.index_of(path);
        self.containers.push(Loaded { file, lif, role });
        let ci = self.containers.len() - 1;
        for node in nodes {
            let storage = if node.flim.is_some() && node.memory_block_id.is_empty() {
                Err(StorageProblem::MissingBlock(String::new()))
            } else {
                self.block_storage(ci, &node.memory_block_id)
                    .ok_or_else(|| StorageProblem::MissingBlock(node.memory_block_id.clone()))
            };
            let display = match prefix {
                Some(p) if !p.is_empty() => format!("{p}/{}", node.path),
                _ => node.path.clone(),
            };
            let tile_scan = node.tile_scan.clone();
            self.entries.push(Entry {
                node,
                display,
                storage,
                container: Some(ci),
                tile_scan,
                parent_image: None,
            });
        }
        Ok(ci)
    }

    /// Open an XML container and follow its references (depth-limited, cycle-safe).
    fn add_xml(&mut self, path: &Path, prefix: Option<&str>, depth: usize) -> Result<()> {
        let c = XmlContainer::open_in(&self.fs, path)?;
        let dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        match c.kind {
            XmlKind::Xlif => {
                let nodes = parse_images(&c.xml)?;
                for node in nodes {
                    let storage = self.frame_storage(&dir, &node);
                    let display = match prefix {
                        Some(p) if !p.is_empty() => format!("{p}/{}", node.path),
                        _ => node.path.clone(),
                    };
                    let tile_scan = node.tile_scan.clone();
                    self.entries.push(Entry {
                        node,
                        display,
                        storage,
                        container: None,
                        tile_scan,
                        parent_image: None,
                    });
                }
            }
            XmlKind::Xlef | XmlKind::Xlcf | XmlKind::Xllf => {
                // Images of an XLEF's children keep their own paths; a collection prefixes its name.
                let child_prefix = match (c.kind, prefix) {
                    (XmlKind::Xlef, p) => p.map(str::to_string),
                    (_, Some(p)) if !p.is_empty() => Some(format!("{p}/{}", c.name)),
                    _ => Some(c.name.clone()),
                };
                for r in &c.references {
                    let target = resolve_reference_in(&self.fs, &dir, r);
                    if !self.fs.is_file(&target) {
                        self.findings.push(Finding::error(
                            "missing_reference",
                            format!("{} references '{r}', which does not exist", path.display()),
                        ));
                        continue;
                    }
                    let canon = canonical(&self.fs, &target);
                    if !self.visited.insert(canon) || depth >= MAX_DEPTH {
                        self.findings.push(Finding::warning(
                            "reference_cycle",
                            format!(
                                "{} references '{r}' again or too deeply nested; skipped",
                                path.display()
                            ),
                        ));
                        continue;
                    }
                    let res = match file_ext(&target).as_str() {
                        "xlif" | "xlef" | "xlcf" | "xllf" => {
                            self.add_xml(&target, child_prefix.as_deref(), depth + 1)
                        }
                        "lif" | "lof" | "lifext" => self
                            .add_binary(&target, child_prefix.as_deref(), "referenced")
                            .map(|_| ()),
                        other => {
                            self.findings.push(Finding::warning(
                                "unsupported_reference",
                                format!("{} references '{r}' (.{other}), which is not a Leica container", path.display()),
                            ));
                            Ok(())
                        }
                    };
                    if let Err(e) = res {
                        self.findings.push(Finding::error(
                            "bad_reference",
                            format!(
                                "{} references '{r}', which could not be read: {e}",
                                path.display()
                            ),
                        ));
                    }
                }
            }
        }
        self.xml_containers.push(c);
        Ok(())
    }

    /// Storage of an XLIF image: one segment per frame, each frame being a LOF payload.
    fn frame_storage(
        &mut self,
        dir: &Path,
        node: &ImageNode,
    ) -> std::result::Result<Storage, StorageProblem> {
        if node.frames.is_empty() {
            return Err(StorageProblem::MissingBlock(node.memory_block_id.clone()));
        }
        let mut segments = Vec::new();
        for fr in &node.frames {
            let target = resolve_reference_in(&self.fs, dir, &fr.file);
            let ext = file_ext(&target);
            // Single-image frame files: decoded, the decoded samples are the frame's bytes.
            // (Multi-page OME/Aivia TIFF frames — `ome.tif` — are not.)
            if let Some(kind) = crate::frames::FrameKind::from_ext(&ext) {
                if !self.fs.is_file(&target) {
                    return Err(StorageProblem::MissingFile(fr.file.clone()));
                }
                let spec = crate::frames::FrameDecode {
                    kind,
                    size: fr.size,
                    remap: rgb_remap(&node.channels),
                };
                let file = self.files.index_of_frame(&target, spec);
                segments.push(Segment {
                    file,
                    file_offset: 0,
                    virt_offset: fr.offset,
                    len: fr.size,
                });
                continue;
            }
            if ext != "lof" {
                return Err(StorageProblem::UnsupportedFrames(format!(".{ext}")));
            }
            if !self.fs.is_file(&target) {
                return Err(StorageProblem::MissingFile(fr.file.clone()));
            }
            let lof = match LifFile::open_in(&self.fs, &target) {
                Ok(l) => l,
                Err(e) => {
                    self.findings.push(Finding::error(
                        "bad_frame_file",
                        format!("frame file '{}' could not be read: {e}", fr.file),
                    ));
                    return Err(StorageProblem::MissingFile(fr.file.clone()));
                }
            };
            let Some(b) = lof.blocks.first().cloned() else {
                return Err(StorageProblem::MissingFile(fr.file.clone()));
            };
            let file = self.files.index_of(&target);
            let avail = lof.file_len.saturating_sub(b.data_offset);
            segments.push(Segment {
                file,
                file_offset: b.data_offset,
                virt_offset: fr.offset,
                len: fr.size.min(b.data_len).min(avail),
            });
            self.containers.push(Loaded {
                file,
                lif: lof,
                role: "referenced",
            });
        }
        Ok(Storage { segments })
    }
}

impl LifDataset {
    /// Open any member of the family: `.lif`, `.lifext`, `.lof`, `.xlif`, `.xlef`, `.xlcf`, `.xllf`.
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let head = crate::container::read_head(fs, path, 4096)?;
        let mut b = Builder::new(fs);
        b.visited.insert(canonical(fs, path));
        let binary = crate::container::looks_like_lif(&head)
            || crate::container::looks_like_lifext(&head)
            || crate::container::looks_like_lof(&head);
        let (kind, root_xml, format_version) =
            if binary || !crate::xlef::looks_like_xml_container(&head) {
                let ci = b.add_binary(path, None, "main")?;
                let lif_kind = b.containers[ci].lif.kind;
                match lif_kind {
                    ContainerKind::Lif => Self::attach_sidecar(&mut b, ci, path),
                    ContainerKind::Lifext => Self::attach_parent(&mut b, path),
                    ContainerKind::Lof => {}
                }
                let c = &b.containers[ci].lif;
                (
                    c.kind.name(),
                    c.xml.clone(),
                    Some(c.header.container_version),
                )
            } else {
                b.add_xml(path, None, 0)?;
                let root = b
                    .xml_containers
                    .last()
                    .expect("add_xml pushes the root container last");
                let v = root
                    .xml
                    .find("Version=\"")
                    .and_then(|i| root.xml[i + 9..].split('"').next())
                    .and_then(|v| v.parse().ok());
                (root.kind.name(), root.xml.clone(), v)
            };
        let sidecar_nodes = if kind == "lif" {
            b.containers
                .iter()
                .find(|c| c.role == "sidecar")
                .and_then(|c| parse_images(&c.lif.xml).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut exposed = Vec::new();
        for (i, e) in b.entries.iter().enumerate() {
            let n = if e.node.flim.is_some() {
                1
            } else {
                geometry(&e.node).map_or(1, |g| g.split_count.max(1))
            };
            for s in 0..n {
                exposed.push(Exposed { entry: i, split: s });
            }
        }
        Ok(LifDataset {
            path: path.to_path_buf(),
            kind,
            files: b.files,
            containers: b.containers,
            xml_containers: b.xml_containers,
            entries: b.entries,
            exposed,
            sidecar_nodes,
            open_findings: b.findings,
            root_xml,
            format_version,
        })
    }

    /// `.lif` with a `<stem>.lifext` next to it: open the sidecar (header only) and let images
    /// whose memory block is missing from the `.lif` resolve to the sidecar.
    fn attach_sidecar(b: &mut Builder, main: usize, path: &Path) {
        let Some(sc) = sidecar_of(&b.fs, path) else {
            return;
        };
        let Ok(lif) = LifFile::open_in(&b.fs, &sc) else {
            b.findings.push(Finding::error(
                "bad_sidecar",
                format!("{} could not be read", sc.display()),
            ));
            return;
        };
        let file = b.files.index_of(&sc);
        b.containers.push(Loaded {
            file,
            lif,
            role: "sidecar",
        });
        let si = b.containers.len() - 1;
        for i in 0..b.entries.len() {
            if b.entries[i].container == Some(main)
                && matches!(b.entries[i].storage, Err(StorageProblem::MissingBlock(_)))
            {
                let id = b.entries[i].node.memory_block_id.clone();
                if let Some(s) = b.block_storage(si, &id) {
                    b.entries[i].storage = Ok(s);
                }
            }
        }
    }

    /// `.lifext` opened directly: name each image's parent from the `.lif` (if present) and let
    /// mosaic pyramid levels inherit the parent's tile-scan positions.
    fn attach_parent(b: &mut Builder, path: &Path) {
        let Some(pp) = parent_of_sidecar(&b.fs, path) else {
            return;
        };
        let Ok(parent) = LifFile::open_in(&b.fs, &pp) else {
            return;
        };
        let Ok(pnodes) = parse_images(&parent.xml) else {
            return;
        };
        for e in &mut b.entries {
            let Some(pid) = e.node.parent_block_id.clone() else {
                continue;
            };
            let Some(p) = pnodes.iter().find(|n| n.memory_block_id == pid) else {
                continue;
            };
            e.parent_image = Some(p.path.clone());
            if e.tile_scan.is_none() && e.node.count(Axis::Mosaic) > 1 {
                e.tile_scan.clone_from(&p.tile_scan);
            }
        }
        let file = b.files.index_of(&pp);
        b.containers.push(Loaded {
            file,
            lif: parent,
            role: "parent",
        });
    }

    fn mosaic_layout(e: &Entry, g: &Geometry) -> Option<MosaicLayout> {
        (g.mosaic > 1).then(|| {
            layout(
                g.mosaic,
                g.sx,
                g.sy,
                metre_step(e.node.dim(Axis::X)),
                metre_step(e.node.dim(Axis::Y)),
                e.tile_scan.as_ref(),
            )
        })
    }

    fn flim_info(index: u32, e: &Entry) -> ImageInfo {
        let f = e.node.flim.clone().unwrap_or_default();
        let mut info = ImageInfo::new(index, f.size("X"), f.size("Y"), PixelType::Uint16);
        info.name = Some(e.display.clone());
        info.size_z = f.size("Z");
        info.size_c = f.size("C");
        info.size_t = f.size("T");
        let um = |v: Option<f64>| v.filter(|v| *v > 0.0).map(|m| m * 1e6);
        info.physical_size = PhysicalSize::micrometres(
            um(f.voxel_size_m[0]),
            um(f.voxel_size_m[1]),
            if f.size("Z") > 1 {
                um(f.voxel_size_m[2])
            } else {
                None
            },
        );
        info.extra.insert(
            "flim".into(),
            json!({
                "kind": "falcon_tcspc",
                "decoded": false,
                "raw_format": f.raw_format,
                "raw_dimensions": f.raw_dims.iter().map(|(k, v)| json!({"id": k, "size": v})).collect::<Vec<_>>(),
                "histogram_bins": f.histogram_bins(),
                "laser_pulse_frequency_hz": f.laser_pulse_frequency_hz,
                "clock_period_s": f.clock_period_s,
                "pixel_time_s": f.pixel_time_s,
            }),
        );
        info.extra
            .insert("memory_block_id".into(), json!(e.node.memory_block_id));
        if let Some(id) = &e.node.unique_id {
            info.extra.insert("unique_id".into(), json!(id));
        }
        info.finish()
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self, index: u32, ex: Exposed) -> Result<ImageInfo> {
        let e = &self.entries[ex.entry];
        if e.node.flim.is_some() {
            return Ok(Self::flim_info(index, e));
        }
        let node = &e.node;
        let g = geometry(node)?;
        let mosaic = Self::mosaic_layout(e, &g);
        let (w, h) = mosaic
            .as_ref()
            .map_or((g.sx, g.sy), |l| (l.width, l.height));
        let mut info = ImageInfo::new(index, w, h, g.fmt.pixel_type);
        let split_label = self.split_label(&g, ex.split);
        info.name = Some(match &split_label {
            Some(s) => format!("{} [{s}]", e.display),
            None => e.display.clone(),
        });
        info.size_z = g.size_z;
        info.size_t = g.size_t;
        info.size_c = g.size_c;
        // Storage order: channel stride vs z vs t, ascending → dimension order after XY.
        // Axes of size 1 sort last, in the conventional C, Z, T order.
        let c_inc = if node.channels.len() > 1 {
            node.channels[1].bytes_inc
        } else if g.lambda > 1 {
            node.inc(Axis::Lambda)
        } else {
            u64::MAX - 2
        };
        let mut order: Vec<(u64, char)> = vec![
            (c_inc, 'C'),
            (
                if g.size_z > 1 {
                    node.inc(Axis::Z)
                } else {
                    u64::MAX - 1
                },
                'Z',
            ),
            (
                if g.size_t > 1 {
                    g.folded.iter().find(|f| f.2 > 1).map_or(u64::MAX, |f| f.1)
                } else {
                    u64::MAX
                },
                'T',
            ),
        ];
        order.sort_unstable();
        info.dimension_order = format!("XY{}", order.iter().map(|(_, c)| *c).collect::<String>());
        info.physical_size = PhysicalSize::micrometres(
            um_step(node.dim(Axis::X)),
            um_step(node.dim(Axis::Y)),
            um_step(node.dim(Axis::Z)),
        );
        info.time_increment_s = node.dim(Axis::T).and_then(DimensionDesc::step);
        let lambda_dim = node.dim(Axis::Lambda).filter(|d| d.count > 1);
        info.channels = (0..g.size_c)
            .map(|k| {
                let ci = (k / g.lambda.max(1)) as usize;
                let l = k % g.lambda.max(1);
                let c = &node.channels[ci];
                let base = (!c.lut_name.is_empty()).then(|| c.lut_name.clone());
                let (name, emission_nm) = match lambda_dim {
                    Some(d) => {
                        let v = d.coordinate(l);
                        let nm = if d.unit.eq_ignore_ascii_case("m") {
                            v * 1e9
                        } else {
                            v
                        };
                        let label = format!(
                            "{} {:.0} nm",
                            base.clone().unwrap_or_else(|| format!("Ch{ci}")),
                            nm
                        );
                        (Some(label), Some((nm * 1000.0).round() / 1000.0))
                    }
                    None => (base, None),
                };
                let mut ch = ChannelInfo {
                    index: k,
                    name,
                    color: lut_color(&c.lut_name).map(str::to_string),
                    // λ scans: the λ coordinate, which is the start of the detection window
                    emission_nm,
                    emission_range_nm: if lambda_dim.is_some() {
                        None
                    } else {
                        c.band_nm
                    },
                    acquisition_mode: node.hardware.acquisition_mode.clone(),
                    ..ChannelInfo::default()
                };
                if let (Some(d), Some(start)) = (lambda_dim, emission_nm)
                    && let Some([a, b]) =
                        lambda_band(node.hardware.lambda_windows, d.count, l, start)
                {
                    ch.set_band(a, b);
                }
                ch
            })
            .collect();
        let hw = &node.hardware;
        if hw.objective_name.is_some()
            || hw.magnification.is_some()
            || hw.numerical_aperture.is_some()
        {
            info.objective = Some(ObjectiveInfo {
                model: hw.objective_name.clone(),
                nominal_magnification: hw.magnification,
                lens_na: hw.numerical_aperture,
                immersion: hw.immersion.clone(),
            });
        }
        if hw.software.is_some() || hw.system_type_name.is_some() || hw.microscope_model.is_some() {
            let (software, version) = split_software(hw.software.as_deref());
            info.instrument = Some(InstrumentInfo {
                manufacturer: Some("Leica Microsystems".into()),
                model: hw
                    .microscope_model
                    .clone()
                    .or_else(|| hw.system_type_name.clone()),
                software,
                software_version: version,
                detector: None,
            });
        }
        info.acquired_at = node.timestamps.first().map(|&ft| filetime_to_iso8601(ft));
        if let Some(l) = &mosaic {
            info.mosaic = Some(MosaicInfo {
                tile_count: g.mosaic,
                tile_width: Some(g.sx),
                tile_height: Some(g.sy),
                stitched_on_read: true,
            });
            info.extra
                .insert("mosaic_placement".into(), json!(l.method.name()));
            let tiles: Vec<serde_json::Value> = l
                .placements
                .iter()
                .map(|p| {
                    let t = e
                        .tile_scan
                        .as_ref()
                        .and_then(|s| s.tiles.get(p.index as usize));
                    json!({
                        "index": p.index,
                        "x_px": p.x_px,
                        "y_px": p.y_px,
                        "field_x": t.map(|t| t.field_x),
                        "field_y": t.map(|t| t.field_y),
                        "position_um": t.map(|t| json!({"x": t.pos_x * 1e6, "y": t.pos_y * 1e6, "z": t.pos_z * 1e6})),
                    })
                })
                .collect();
            info.extra.insert("tiles".into(), json!(tiles));
            if let Some(s) = &e.tile_scan {
                info.extra.insert(
                    "tile_scan_flags".into(),
                    json!({"flip_x": s.flip_x, "flip_y": s.flip_y, "swap_xy": s.swap_xy}),
                );
            }
        }
        if let Some(d) = lambda_dim {
            let scale = if d.unit.eq_ignore_ascii_case("m") {
                1e9
            } else {
                1.0
            };
            info.extra.insert(
                "lambda".into(),
                json!({"count": d.count, "first_nm": d.coordinate(0) * scale,
                       "last_nm": d.coordinate(d.count - 1) * scale,
                       "channel_order": "channel-major: c = detector_channel * lambda_count + lambda_index"}),
            );
        }
        let folded: Vec<serde_json::Value> = g
            .folded
            .iter()
            .filter(|f| f.0 != Axis::T)
            .map(|f| json!({"axis": f.0.name(), "count": f.2}))
            .collect();
        if !folded.is_empty() {
            info.extra.insert(
                "t_folded".into(),
                json!({"axes_outer_to_inner": folded.iter().rev().cloned().chain(std::iter::once(json!({"axis": "t", "count": node.count(Axis::T)}))).collect::<Vec<_>>()}),
            );
        }
        if !g.split.is_empty() {
            let mut rest = ex.split;
            let parts: Vec<serde_json::Value> = g
                .split
                .iter()
                .map(|(a, _, n)| {
                    let i = rest % n;
                    rest /= n;
                    let coord = node
                        .dimensions
                        .iter()
                        .find(|d| d.axis == *a)
                        .map(|d| d.coordinate(i));
                    json!({"axis": a.name(), "index": i, "count": n, "coordinate": coord})
                })
                .collect();
            info.extra.insert("split".into(), json!(parts));
        }
        if let Some(p) = &e.parent_image {
            info.extra.insert("parent_image".into(), json!(p));
        }
        if g.fmt.half {
            info.extra
                .insert("stored_pixel_type".into(), json!("float16"));
        }
        if self.kind == "lif" && !self.sidecar_nodes.is_empty() {
            let kids: Vec<serde_json::Value> = self
                .sidecar_nodes
                .iter()
                .filter(|s| s.parent_block_id.as_deref() == Some(node.memory_block_id.as_str()))
                .map(|s| {
                    json!({"path": s.path, "size_x": s.count(Axis::X), "size_y": s.count(Axis::Y),
                                "memory_block_id": s.memory_block_id})
                })
                .collect();
            if !kids.is_empty() {
                info.extra.insert("lifext_images".into(), json!(kids));
            }
        }
        info.extra.insert(
            "memory_block_id".into(),
            serde_json::Value::String(node.memory_block_id.clone()),
        );
        if let Some(id) = &node.unique_id {
            info.extra
                .insert("unique_id".into(), serde_json::Value::String(id.clone()));
        }
        // `Resolution` of integer channels, when all agree: the detector's bit depth (12-bit
        // data is stored as uint16), used for saturation.
        if let Some(first) = node.channels.first()
            && first.data_type == 0
            && node
                .channels
                .iter()
                .all(|c| c.data_type == 0 && c.resolution_bits == first.resolution_bits)
        {
            info.extra
                .insert("bits_significant".into(), json!(first.resolution_bits));
        }
        Ok(info.finish())
    }

    fn split_label(&self, g: &Geometry, split: u32) -> Option<String> {
        let _ = self;
        if g.split.is_empty() {
            return None;
        }
        let mut rest = split;
        let parts: Vec<String> = g
            .split
            .iter()
            .map(|(a, _, n)| {
                let i = rest % n;
                rest /= n;
                format!("{} {i}", a.name())
            })
            .collect();
        Some(parts.join(", "))
    }

    /// Read one tile plane (no stitching), converted to the output sample format.
    fn read_tile(
        &mut self,
        ei: usize,
        g: &Geometry,
        idx: PlaneIndex,
        split: u32,
        tile: u32,
    ) -> Result<Vec<u8>> {
        let e = &self.entries[ei];
        let storage = match &e.storage {
            Ok(s) => s.clone(),
            Err(StorageProblem::MissingBlock(id)) => {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("memory block {id} referenced by XML not found"),
                ));
            }
            Err(StorageProblem::MissingFile(f)) => {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("frame file '{f}' referenced by the XLIF is missing"),
                ));
            }
            Err(StorageProblem::UnsupportedFrames(ext)) => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("XLIF frames stored as {ext} files"),
                    "Only XLIF frames stored in .lof files are read; convert the experiment to .lif in LAS X, or read the frame images with a TIFF/PNG reader.",
                ));
            }
        };
        let node = &e.node;
        let bps = g.fmt.stored_bytes;
        let bad = || Error::corrupt(FORMAT_ID, "plane address overflows");
        let x_inc = node.inc(Axis::X).max(bps);
        let y_inc = node
            .inc(Axis::Y)
            .max(u64::from(g.sx).checked_mul(x_inc).ok_or_else(bad)?);
        let base = plane_offset(node, g, idx.c, idx.z, idx.t, split, tile).ok_or_else(bad)?;
        let row_span = (u64::from(g.sx) - 1)
            .checked_mul(x_inc)
            .and_then(|v| v.checked_add(bps))
            .ok_or_else(bad)?;
        let last = u64::from(g.sy - 1)
            .checked_mul(y_inc)
            .and_then(|v| v.checked_add(base))
            .and_then(|v| v.checked_add(row_span))
            .ok_or_else(bad)?;
        if last > storage.contiguous_len() && storage.segments.len() == 1 {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                storage.locate(0).map_or(0, |(_, o)| o) + base,
                format!(
                    "plane c={} z={} t={} extends past the end of its memory block (file truncated?)",
                    idx.c, idx.z, idx.t
                ),
            ));
        }
        // Samples are stored uncompressed: a plane cannot be (much) larger than its block.
        let stored: u64 = storage
            .segments
            .iter()
            .fold(0u64, |n, s| n.saturating_add(s.len));
        let out_len = plane_len(FORMAT_ID, u64::from(g.sx), u64::from(g.sy), 1, bps, stored)?;
        let mut data = vec![0u8; out_len];
        let row_out = (u64::from(g.sx) * bps) as usize;
        if x_inc == bps && y_inc == u64::from(g.sx) * bps {
            self.files.read(&storage, base, &mut data)?;
        } else {
            if row_span > stored {
                return Err(bad());
            }
            let mut row = vec![0u8; usize::try_from(row_span).map_err(|_| bad())?];
            let b = bps as usize;
            for y in 0..g.sy as usize {
                self.files
                    .read(&storage, base + y as u64 * y_inc, &mut row)?;
                let dst = &mut data[y * row_out..(y + 1) * row_out];
                if x_inc == bps {
                    dst.copy_from_slice(&row[..row_out]);
                } else {
                    for x in 0..g.sx as usize {
                        let s = x * x_inc as usize;
                        dst[x * b..(x + 1) * b].copy_from_slice(&row[s..s + b]);
                    }
                }
            }
        }
        Ok(if g.fmt.half { widen_half(&data) } else { data })
    }

    fn check_container(r: &mut CheckReport, c: &Loaded) {
        let name = c
            .lif
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().to_string());
        if let Some(off) = c.lif.truncated_at {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{name}: block chain runs past end of file (file is {} bytes); data after this point is missing",
                        c.lif.file_len
                    ),
                )
                .at(off),
            );
        }
        if let Some((off, why)) = &c.lif.bad_block_at {
            let code = if c.lif.kind == ContainerKind::Lof {
                "trailing_bytes"
            } else {
                "bad_block_header"
            };
            let f = if code == "trailing_bytes" {
                Finding::warning(code, format!("{name}: {why}"))
            } else {
                Finding::error(code, format!("{name}: {why}"))
            };
            r.push(f.at(*off));
        }
        if c.lif.kind != ContainerKind::Lof
            && c.lif.truncated_at.is_none()
            && c.lif.bad_block_at.is_none()
            && let Some(last) = c.lif.blocks.last()
        {
            let end = last.data_offset + last.data_len;
            if end != c.lif.file_len {
                r.push(
                    Finding::warning(
                        "trailing_bytes",
                        format!(
                            "{name}: {} bytes after the last block",
                            c.lif.file_len - end
                        ),
                    )
                    .at(end),
                );
            }
        }
    }
}

impl Dataset for LifDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for c in &self.containers {
            let p = &c.lif.path;
            if *p != self.path && self.files.fs.is_file(p) && !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    }
    fn info(&self) -> Result<FileInfo> {
        let mut images = Vec::with_capacity(self.exposed.len());
        for (i, ex) in self.exposed.iter().enumerate() {
            images.push(self.image_info(i as u32, *ex)?);
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let mut notes = Vec::new();
        if self
            .containers
            .iter()
            .any(|c| c.role != "parent" && c.lif.truncated_at.is_some())
        {
            notes.push("file appears truncated; run `check`".into());
        }
        if images.iter().any(|i| i.mosaic.is_some()) {
            notes.push("tile scans are stitched on read from TileScanInfo stage positions (later tiles overwrite earlier ones where they overlap); per-tile offsets are in images[].extra.tiles and per-tile byte ranges in `info --view structure`".into());
        }
        let frame_kinds: std::collections::BTreeSet<&str> = (0..self.files.paths.len())
            .filter_map(|i| self.files.frame_kind(i))
            .map(crate::frames::FrameKind::name)
            .collect();
        if !frame_kinds.is_empty() {
            notes.push(format!(
                "pixels come from the XLIF's frame image files ({}), decoded: an RGB frame fills the red, green and blue channels by channel tag{}",
                frame_kinds.iter().copied().collect::<Vec<_>>().join(", "),
                if frame_kinds.iter().any(|k| *k != "tiff") {
                    "; JPEG and PNG frames follow the rule validated on TIFF frames only"
                } else {
                    ""
                }
            ));
        }
        if self.entries.iter().any(|e| e.node.count(Axis::Lambda) > 1) {
            notes.push("spectral (lambda) series are exposed as channels: size_c = detector channels x lambda steps; channel names carry the wavelength".into());
        }
        let flim = self
            .entries
            .iter()
            .filter(|e| e.node.flim.is_some())
            .count();
        if flim > 0 {
            notes.push(format!("{flim} FALCON FLIM/TCSPC image(s): geometry is reported but raw photon data is not decoded (reading their planes exits 6); LAS X's derived intensity/lifetime child images are readable"));
        }
        if let Some(sc) = self.containers.iter().find(|c| c.role == "sidecar") {
            notes.push(format!(
                "LIFEXT sidecar {} holds {} additional image(s) (8-bit pyramid levels and histograms); open the .lifext directly to read them",
                sc.lif.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().to_string()),
                self.sidecar_nodes.len()
            ));
        }
        if !self.open_findings.is_empty() {
            notes.push(format!(
                "{} problem(s) while following container references; run `check`",
                self.open_findings.len()
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.files.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: LifReader.descriptor(),
            format_version: self.format_version.map(|v| v.to_string()),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        xml_to_json(&self.root_xml)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "XML header does not parse"))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_c", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].dimension_order", Source::Inferred),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].physical_size", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].channels[].name", Source::Inferred),
            ("images[].channels[].color", Source::Inferred),
            ("images[].channels[].emission_nm", Source::Inferred),
            ("images[].channels[].emission_range_nm", Source::Inferred),
            ("images[].channels[].acquisition_mode", Source::Inferred),
            ("images[].objective", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].mosaic", Source::Inferred),
            ("images[].extra.tiles", Source::Inferred),
            ("images[].extra.bits_significant", Source::PriorArt),
            (
                "images[].channels[].emission_band_start_nm",
                Source::Inferred,
            ),
            ("images[].channels[].emission_band_end_nm", Source::Inferred),
            (
                "images[].channels[].emission_band_center_nm",
                Source::Inferred,
            ),
            ("images[].extra.lambda", Source::Inferred),
            ("images[].extra.flim", Source::PriorArt),
            ("images[].extra.lifext_images", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for c in &self.xml_containers {
            out.push(LsEntry {
                kind: "container".into(),
                name: c.path.display().to_string(),
                offset: None,
                size: self.files.fs.metadata(&c.path).ok().map(|m| m.len()),
                image: None,
                details: json!({"type": c.kind.name(), "element": c.name, "references": c.references}),
            });
        }
        for c in &self.containers {
            out.push(LsEntry {
                kind: if c.role == "main" {
                    "metadata"
                } else {
                    "container"
                }
                .into(),
                name: if c.role == "main" {
                    "xml-header".into()
                } else {
                    c.lif.path.display().to_string()
                },
                offset: Some(c.lif.header.xml_offset),
                size: Some(2 * u64::from(c.lif.header.xml_len)),
                image: None,
                details: json!({"type": c.lif.kind.name(), "role": c.role, "encoding": "utf-16le",
                                "container_version": c.lif.header.container_version,
                                "lof_versions": c.lif.header.lof_versions}),
            });
        }
        for (i, ex) in self.exposed.iter().enumerate() {
            let e = &self.entries[ex.entry];
            let node = &e.node;
            let loc = e.storage.as_ref().ok().and_then(|s| s.locate(0));
            let (storage_desc, file) = match &e.storage {
                Ok(s) => (
                    json!({"segments": s.segments.len()}),
                    loc.map(|(f, _)| self.files.paths[f].display().to_string()),
                ),
                Err(p) => (json!({"problem": format!("{p:?}")}), None),
            };
            out.push(LsEntry {
                kind: if node.flim.is_some() { "flim" } else { "image" }.into(),
                name: e.display.clone(),
                offset: loc.map(|(_, o)| o),
                size: Some(node.memory_size),
                image: Some(i as u32),
                details: json!({
                    "memory_block_id": node.memory_block_id,
                    "file": file,
                    "storage": storage_desc,
                    "split": if ex.split > 0 || geometry(node).is_ok_and(|g| g.split_count > 1) { Some(ex.split) } else { None },
                    "dimensions": node.dimensions.iter().map(|d| json!({"axis": d.axis.name(), "dim_id": d.dim_id, "count": d.count, "bytes_inc": d.bytes_inc})).collect::<Vec<_>>(),
                    "channels": node.channels.iter().map(|c| json!({"bytes_inc": c.bytes_inc, "bits": c.resolution_bits, "lut": c.lut_name})).collect::<Vec<_>>(),
                }),
            });
            // Per-tile access: each tile's byte range (c=0, z=0, t=0) and its place in the mosaic.
            if ex.split == 0
                && node.flim.is_none()
                && let Ok(g) = geometry(node)
                && let Some(l) = Self::mosaic_layout(e, &g)
            {
                for (k, p) in l.placements.iter().enumerate() {
                    let voff = plane_offset(node, &g, 0, 0, 0, 0, p.index);
                    let at = voff.and_then(|v| e.storage.as_ref().ok().and_then(|s| s.locate(v)));
                    let t = e
                        .tile_scan
                        .as_ref()
                        .and_then(|s| s.tiles.get(p.index as usize));
                    out.push(LsEntry {
                        kind: "tile".into(),
                        name: format!("{} [tile {}]", e.display, p.index),
                        offset: at.map(|(_, o)| o),
                        size: Some(node.inc(Axis::Mosaic)),
                        image: Some(i as u32),
                        details: json!({
                            "tile": p.index,
                            "x_px": p.x_px, "y_px": p.y_px,
                            "width": g.sx, "height": g.sy,
                            "field_x": t.map(|t| t.field_x), "field_y": t.map(|t| t.field_y),
                            "fully_visible": !l.overlapped_by_later(k),
                            "placement": l.method.name(),
                        }),
                    });
                }
            }
        }
        for s in &self.sidecar_nodes {
            let parent = self
                .entries
                .iter()
                .find(|e| Some(e.node.memory_block_id.as_str()) == s.parent_block_id.as_deref());
            out.push(LsEntry {
                kind: "sidecar_image".into(),
                name: s.path.clone(),
                offset: None,
                size: Some(s.memory_size),
                image: None,
                details: json!({"memory_block_id": s.memory_block_id, "parent_block_id": s.parent_block_id,
                                "parent_image": parent.map(|p| p.display.clone()),
                                "size_x": s.count(Axis::X), "size_y": s.count(Axis::Y)}),
            });
        }
        for c in &self.containers {
            if c.role == "parent" {
                continue;
            }
            for b in &c.lif.blocks {
                let owner = self.entries.iter().position(|e| {
                    e.storage.as_ref().is_ok_and(|s| {
                        s.segments
                            .iter()
                            .any(|sg| sg.file == c.file && sg.file_offset == b.data_offset)
                    })
                });
                let role = if owner.is_some() {
                    "pixels"
                } else if c.role == "sidecar" {
                    "sidecar"
                } else if b.data_len == 0 {
                    "empty"
                } else {
                    "attachment"
                };
                out.push(LsEntry {
                    kind: "block".into(),
                    name: b.block_id.clone(),
                    offset: Some(b.data_offset),
                    size: Some(b.data_len),
                    image: owner
                        .and_then(|n| self.exposed.iter().position(|e| e.entry == n))
                        .map(|i| i as u32),
                    details: json!({"header_offset": b.header_offset, "role": role, "file": c.lif.path.display().to_string()}),
                });
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        let ex = *self.exposed.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.exposed.len()
            ))
        })?;
        let e = &self.entries[ex.entry];
        if e.node.flim.is_some() {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "FALCON FLIM/TCSPC raw photon data (image {image}, '{}')",
                    e.display
                ),
                "Raw TCSPC histograms are not decoded. Read the LAS X result images stored as its children instead (e.g. '<image>/Intensity', '<image>/Fast Flim'; see `openreadout info`), or export them from LAS X.",
            ));
        }
        let g = geometry(&e.node)?;
        if idx.c >= g.size_c || idx.z >= g.size_z || idx.t >= g.size_t {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{}, z<{}, t<{})",
                idx.c, idx.z, idx.t, g.size_c, g.size_z, g.size_t
            )));
        }
        let pixel_type = g.fmt.pixel_type;
        let out_bps = pixel_type.bytes_per_sample();
        let Some(l) = Self::mosaic_layout(e, &g) else {
            let data = self.read_tile(ex.entry, &g, idx, ex.split, 0)?;
            return Ok(Plane {
                width: g.sx,
                height: g.sy,
                pixel_type,
                samples_per_pixel: 1,
                data,
            });
        };
        let canvas_len = plane_len(
            FORMAT_ID,
            u64::from(l.width),
            u64::from(l.height),
            1,
            out_bps as u64,
            // The stitched canvas is bounded by the tiles it is made of.
            u64::from(g.sx)
                .saturating_mul(u64::from(g.sy))
                .saturating_mul(out_bps as u64)
                .saturating_mul(u64::from(g.mosaic)),
        )?;
        let mut canvas = vec![0u8; canvas_len];
        let row_out = l.width as usize * out_bps;
        let tile_row = g.sx as usize * out_bps;
        for p in &l.placements {
            let tile = self.read_tile(ex.entry, &g, idx, ex.split, p.index)?;
            let (x0, y0) = (p.x_px as usize, p.y_px as usize);
            let w = (g.sx as usize).min(l.width as usize - x0);
            for y in 0..(g.sy as usize).min(l.height as usize - y0) {
                let dst = (y0 + y) * row_out + x0 * out_bps;
                canvas[dst..dst + w * out_bps]
                    .copy_from_slice(&tile[y * tile_row..y * tile_row + w * out_bps]);
            }
        }
        Ok(Plane {
            width: l.width,
            height: l.height,
            pixel_type,
            samples_per_pixel: 1,
            data: canvas,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("file header markers and block_len == 2*xml_len+5");
        r.performed("XML header is well-formed");
        r.performed(
            "memory block chain: every block header valid, chain ends exactly at end of file",
        );
        r.performed(
            "every image's memory block exists and is at least as large as its declared geometry",
        );
        r.performed("every image's XML Memory/@Size equals its block payload length");
        if !self.xml_containers.is_empty() {
            r.performed(
                "every XLEF/XLCF/XLLF reference and XLIF frame file exists and is readable",
            );
        }
        if self.containers.iter().any(|c| c.role == "sidecar") {
            r.performed("LIFEXT sidecar: block chain valid and every ChildrenOf/@MemoryBlockID names an image in the .lif");
        }
        for f in &self.open_findings {
            r.push(f.clone());
        }
        for c in &self.containers {
            if c.role != "parent" {
                Self::check_container(&mut r, c);
            }
        }
        for e in &self.entries {
            let node = &e.node;
            if let Some(f) = &node.flim {
                r.push(Finding::info(
                    "flim_not_decoded",
                    format!(
                        "'{}': FALCON FLIM/TCSPC data ({} format, {} bytes) is present but not decoded",
                        e.display, f.raw_format, node.memory_size
                    ),
                ));
            }
            let storage = match &e.storage {
                Ok(s) => s,
                Err(StorageProblem::MissingBlock(id)) => {
                    if node.flim.is_none() || !id.is_empty() {
                        r.push(Finding::error(
                            "missing_block",
                            format!(
                                "image '{}' references memory block {id} which is not in the file",
                                e.display
                            ),
                        ));
                    }
                    continue;
                }
                Err(StorageProblem::MissingFile(f)) => {
                    r.push(Finding::error(
                        "missing_frame_file",
                        format!("image '{}': frame file '{f}' is missing", e.display),
                    ));
                    continue;
                }
                Err(StorageProblem::UnsupportedFrames(ext)) => {
                    r.push(Finding::warning(
                        "unsupported_frames",
                        format!(
                            "image '{}': frames are stored as {ext} files, which are not read",
                            e.display
                        ),
                    ));
                    continue;
                }
            };
            if node.flim.is_some() {
                continue;
            }
            let bps = geometry(node).map_or(1, |g| g.fmt.stored_bytes);
            let need = bytes_needed(node, bps);
            let have = storage.contiguous_len();
            let at = storage.locate(0).map(|(_, o)| o);
            if have < need {
                let missing = need - have;
                let mut f = Finding::error(
                    "missing_planes",
                    format!(
                        "image '{}': geometry needs {need} bytes but only {have} are stored ({missing} bytes missing)",
                        e.display
                    ),
                );
                if let Some(o) = at {
                    f = f.at(o);
                }
                r.push(f);
            }
            let stored: u64 = storage.segments.iter().map(|s| s.len).sum();
            if storage.segments.len() == 1 && node.memory_size != stored {
                let mut f = Finding::warning(
                    "size_mismatch",
                    format!(
                        "image '{}': XML declares {} bytes, block {} holds {stored}",
                        e.display, node.memory_size, node.memory_block_id
                    ),
                );
                if let Some(o) = at {
                    f = f.at(o);
                }
                r.push(f);
            }
        }
        if self.containers.iter().any(|c| c.role == "sidecar") {
            let mut orphans: Vec<&str> = self
                .sidecar_nodes
                .iter()
                .filter_map(|s| s.parent_block_id.as_deref())
                .filter(|id| !self.entries.iter().any(|e| e.node.memory_block_id == *id))
                .collect();
            orphans.dedup();
            for id in orphans {
                r.push(Finding::warning(
                    "sidecar_orphan",
                    format!("LIFEXT sidecar has images for memory block {id}, which no image in the .lif uses"),
                ));
            }
        }
        if self.entries.is_empty() {
            r.push(Finding::warning(
                "no_images",
                "the XML header describes no images",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::pixel::half_to_f32;

    #[test]
    fn half_floats_widen_exactly() {
        assert_eq!(half_to_f32(0x3c00).to_bits(), 1.0f32.to_bits());
        assert_eq!(half_to_f32(0xc000).to_bits(), (-2.0f32).to_bits());
        assert_eq!(half_to_f32(0x7bff).to_bits(), 65504.0f32.to_bits());
        assert_eq!(half_to_f32(0x0001).to_bits(), 5.960_464_5e-8f32.to_bits());
        assert_eq!(half_to_f32(0x0400).to_bits(), 6.103_515_6e-5f32.to_bits());
        assert!(half_to_f32(0x7c00).is_infinite());
        assert!(half_to_f32(0x7e00).is_nan());
        assert_eq!(
            half_to_f32(0x7c01).to_bits(),
            0x7fc0_2000,
            "signalling NaN is quieted"
        );
        assert_eq!(half_to_f32(0x8000).to_bits(), 0x8000_0000);
        assert_eq!(widen_half(&[0x00, 0x3c]), 1.0f32.to_le_bytes());
    }

    fn dim(axis_id: u32, count: u32, inc: u64) -> DimensionDesc {
        DimensionDesc {
            dim_id: axis_id,
            axis: Axis::from_dim_id(axis_id),
            count,
            origin: 4.0e-7,
            length: 1.0e-7,
            unit: "m".into(),
            bytes_inc: inc,
        }
    }

    fn node(dims: Vec<DimensionDesc>, channels: usize) -> ImageNode {
        ImageNode {
            name: "n".into(),
            path: "n".into(),
            unique_id: None,
            memory_block_id: "MemBlock_1".into(),
            memory_size: 0,
            channels: (0..channels)
                .map(|i| ChannelDesc {
                    data_type: 0,
                    channel_tag: 0,
                    resolution_bits: 8,
                    bytes_inc: i as u64 * 1000,
                    lut_name: String::new(),
                    min: 0.0,
                    max: 0.0,
                    band_nm: None,
                })
                .collect(),
            dimensions: dims,
            timestamps: Vec::new(),
            hardware: crate::xml::HardwareInfo::default(),
            tile_scan: None,
            flim: None,
            parent_block_id: None,
            frames: Vec::new(),
        }
    }

    #[test]
    fn lambda_folds_into_channels_and_slices_into_t() {
        // X 4, Y 2, lambda 3 (inc 8), T 2 (inc 24), XT slices 2 (inc 48), rotation 2 (inc 96)
        let n = node(
            vec![
                dim(1, 4, 1),
                dim(2, 2, 4),
                dim(5, 3, 8),
                dim(4, 2, 24),
                dim(7, 2, 48),
                dim(6, 2, 96),
            ],
            2,
        );
        let g = geometry(&n).unwrap();
        assert_eq!((g.size_c, g.size_t, g.split_count), (6, 4, 2));
        // c = 4 → channel 1, lambda 1; t = 3 → t 1, xt slice 1; split 1 → rotation 1
        assert_eq!(
            plane_offset(&n, &g, 4, 0, 3, 1, 0),
            Some(1000 + 8 + 24 + 48 + 96)
        );
        assert_eq!(plane_offset(&n, &g, 2, 0, 0, 0, 0), Some(16));
        assert_eq!(bytes_needed(&n, 1), 4 + 3 + 8 * 2 + 24 + 48 + 96 + 1000 + 1);
    }
}

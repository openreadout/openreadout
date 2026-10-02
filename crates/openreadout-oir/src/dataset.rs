//! `Dataset` implementation: plane map from chunk tags, normalized metadata from the XML
//! documents, plane reads across continuation files, integrity checks.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Value, json};

use crate::container::{Block, BlockKind, ChunkName, OirFile, XmlDoc, documents};
use crate::meta::{
    self, ChannelSettings, FrameGeometry, FrameRecord, ImageProperties, frame_record,
};
use crate::{FORMAT_ID, OirReader};

/// Continuation files are looked for up to this number.
const MAX_CONTINUATIONS: u32 = 9999;
/// Text values longer than this are shortened in `vendor` (LUT tables are 512 KiB of hex).
const MAX_VENDOR_TEXT: usize = 4096;

/// One stored chunk of a plane.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    /// Index into `OirDataset::files`.
    file: usize,
    /// File offset of the samples.
    offset: u64,
    len: u32,
    /// Byte offset inside the plane.
    plane_offset: u32,
}

/// `(t, lambda, z)` index as written in chunk names (1-based; 0 when absent) and channel slot.
type PlaneKey = (u32, u32, u32, usize);

/// The main image: geometry, axis values and where each plane's chunks are.
#[derive(Debug, Default)]
struct Layout {
    geometry: FrameGeometry,
    pixel_type: Option<PixelType>,
    t_values: Vec<u32>,
    l_values: Vec<u32>,
    z_values: Vec<u32>,
    /// Channel uuids in exposed order.
    channels: Vec<String>,
    planes: HashMap<PlaneKey, Vec<Chunk>>,
    /// Chunks whose names carry axis letters we do not know.
    unknown_axis_chunks: usize,
}

impl Layout {
    fn plane_len(&self) -> u64 {
        u64::from(self.geometry.width)
            * u64::from(self.geometry.height)
            * u64::from(self.geometry.depth)
    }
}

/// The reference image (`REF_` chunks): one plane per channel.
#[derive(Debug, Default)]
struct Reference {
    source: String,
    width: u32,
    height: u32,
    depth: u32,
    pixel_type: Option<PixelType>,
    channels: Vec<String>,
    planes: BTreeMap<usize, Vec<Chunk>>,
}

/// Which exposed image an index is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exposed {
    Main,
    Reference,
}

/// An opened OIR file (plus its continuation files).
#[derive(Debug)]
pub struct OirDataset {
    path: PathBuf,
    /// Where the file and its continuation files are read from.
    fs: Fs,
    /// `[0]` is the file that was opened; the rest are `<stem>_00001`, `_00002`, ...
    files: Vec<OirFile>,
    /// The first continuation name that was looked for and not found.
    next_continuation: PathBuf,
    /// Documents of the last documents block of the main file.
    docs: Vec<XmlDoc>,
    documents_blocks: usize,
    props: Option<ImageProperties>,
    settings: Vec<ChannelSettings>,
    /// `(channel uuid, LUT name)`.
    luts: Vec<(String, String)>,
    /// `(file, block index)` of every frame-properties block, in file order.
    frame_blocks: Vec<(usize, usize)>,
    first_frame: Option<FrameRecord>,
    layout: Option<Layout>,
    reference: Option<Reference>,
    exposed: Vec<Exposed>,
    open_findings: Vec<Finding>,
}

fn pixel_type_for(depth: u32) -> Option<PixelType> {
    match depth {
        1 => Some(PixelType::Uint8),
        2 => Some(PixelType::Uint16),
        4 => Some(PixelType::Float),
        _ => None,
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().to_string())
}

/// `<dir>/<stem>_<n:05>` for `<dir>/<stem>.oir`.
fn continuation_path(path: &Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().to_string());
    path.with_file_name(format!("{stem}_{n:05}"))
}

/// Replace very long strings (LUT tables) by a note; keep everything else.
fn shorten(v: &mut Value) {
    match v {
        Value::String(s) if s.len() > MAX_VENDOR_TEXT => {
            *s = format!("<{} characters omitted>", s.len());
        }
        Value::Array(a) => a.iter_mut().for_each(shorten),
        Value::Object(o) => o.values_mut().for_each(shorten),
        _ => {}
    }
}

fn doc_json(d: &XmlDoc) -> Value {
    let mut v = xml_to_json(&d.text).unwrap_or_else(|| json!({"#unparsed": d.text.len()}));
    shorten(&mut v);
    v
}

/// 1-based `(t, lambda, z)` from a frame name such as `t002l001z003_0_1`.
fn frame_axes(name: &str) -> Option<(u32, u32, u32)> {
    let n = ChunkName::parse(&format!("{name}_0_0"))
        .or_else(|| ChunkName::parse(&format!("{name}_0_1_0_0")))?;
    Some((n.t, n.lambda, n.z))
}

impl OirDataset {
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let main = OirFile::open_in(fs, path)?;
        let mut ds = OirDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            files: vec![main],
            next_continuation: continuation_path(path, 1),
            docs: Vec::new(),
            documents_blocks: 0,
            props: None,
            settings: Vec::new(),
            luts: Vec::new(),
            frame_blocks: Vec::new(),
            first_frame: None,
            layout: None,
            reference: None,
            exposed: Vec::new(),
            open_findings: Vec::new(),
        };
        ds.open_continuations();
        ds.load_documents()?;
        ds.load_frames()?;
        ds.build_layout();
        Ok(ds)
    }

    fn open_continuations(&mut self) {
        let is_oir = self
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("oir"));
        if !is_oir {
            return;
        }
        for n in 1..=MAX_CONTINUATIONS {
            let p = continuation_path(&self.path, n);
            self.next_continuation = p.clone();
            if !self.fs.is_file(&p) {
                break;
            }
            match OirFile::open_in(&self.fs, &p) {
                Ok(f) => self.files.push(f),
                Err(e) => {
                    self.open_findings.push(Finding::error(
                        "bad_continuation",
                        format!("continuation file {} cannot be read: {e}", file_name(&p)),
                    ));
                    break;
                }
            }
        }
    }

    fn load_documents(&mut self) -> Result<()> {
        let main = &mut self.files[0];
        let doc_blocks: Vec<usize> = main
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == BlockKind::Documents && b.end() <= main.file_len)
            .map(|(i, _)| i)
            .collect();
        self.documents_blocks = doc_blocks.len();
        // The last documents block is written when the acquisition ends; earlier ones are snapshots.
        for &i in doc_blocks.iter().rev() {
            let payload = main.payload(i)?;
            let docs = documents(&payload, main.blocks[i].payload_offset());
            if docs.iter().any(|d| d.root.ends_with(":imageProperties")) || i == doc_blocks[0] {
                self.docs = docs;
                break;
            }
        }
        if let Some(d) = self
            .docs
            .iter()
            .find(|d| d.root.ends_with(":imageProperties"))
        {
            self.props = meta::image_properties(&d.text);
        }
        self.settings = self
            .docs
            .iter()
            .filter(|d| d.root.ends_with(":lsmChannel"))
            .filter_map(|d| meta::channel_settings(&d.text))
            .collect();
        self.luts = self
            .docs
            .iter()
            .filter(|d| d.root.ends_with(":LUT"))
            .filter_map(|d| Some((d.channel.clone()?, meta::lut_name(&d.text)?)))
            .collect();
        Ok(())
    }

    fn load_frames(&mut self) -> Result<()> {
        for (fi, f) in self.files.iter().enumerate() {
            for (bi, b) in f.blocks.iter().enumerate() {
                if b.kind == BlockKind::FrameProperties && b.end() <= f.file_len {
                    self.frame_blocks.push((fi, bi));
                }
            }
        }
        if let Some(&(fi, bi)) = self.frame_blocks.first() {
            self.first_frame = self.frame_at(fi, bi)?;
        }
        Ok(())
    }

    fn frame_at(&mut self, fi: usize, bi: usize) -> Result<Option<FrameRecord>> {
        let f = &mut self.files[fi];
        let payload = f.payload(bi)?;
        let docs = documents(&payload, f.blocks[bi].payload_offset());
        Ok(docs.first().and_then(|d| frame_record(&d.text)))
    }

    fn build_layout(&mut self) {
        let mut main: BTreeMap<(u32, u32, u32, String), Vec<Chunk>> = BTreeMap::new();
        let mut refs: BTreeMap<String, Vec<Chunk>> = BTreeMap::new();
        let mut ref_source = String::new();
        let mut unknown_axis_chunks = 0usize;
        for (fi, f) in self.files.iter().enumerate() {
            for (bi, tag) in &f.chunk_tags {
                let Some(pix) = f.pixels_after(*bi) else {
                    self.open_findings.push(
                        Finding::error(
                            "orphan_chunk_tag",
                            format!(
                                "{}: chunk tag '{}' is not followed by a pixel block",
                                file_name(&f.path),
                                tag.name
                            ),
                        )
                        .at(f.blocks[*bi].offset),
                    );
                    continue;
                };
                let Some(name) = ChunkName::parse(&tag.name) else {
                    self.open_findings.push(
                        Finding::warning(
                            "unparsed_chunk_name",
                            format!(
                                "{}: chunk name '{}' has an unknown form; its pixels are not used",
                                file_name(&f.path),
                                tag.name
                            ),
                        )
                        .at(f.blocks[*bi].offset),
                    );
                    continue;
                };
                let chunk = Chunk {
                    file: fi,
                    offset: pix.payload_offset(),
                    len: pix.payload_len,
                    plane_offset: tag.plane_offset,
                };
                if let Some(src) = name.reference {
                    ref_source = src;
                    refs.entry(name.channel).or_default().push(chunk);
                } else if !name.unknown_axes.is_empty() {
                    unknown_axis_chunks += 1;
                } else {
                    main.entry((name.t, name.lambda, name.z, name.channel))
                        .or_default()
                        .push(chunk);
                }
            }
        }
        let order: Vec<String> = self
            .props
            .as_ref()
            .map(|p| p.channels.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_default();
        let ordered = |present: BTreeSet<String>| -> Vec<String> {
            let mut out: Vec<String> = order
                .iter()
                .filter(|id| present.contains(*id))
                .cloned()
                .collect();
            out.extend(present.into_iter().filter(|id| !order.contains(id)));
            out
        };
        if !main.is_empty() {
            let geometry = self
                .first_frame
                .as_ref()
                .map(|f| f.geometry.clone())
                .unwrap_or_default();
            let mut l = Layout {
                pixel_type: pixel_type_for(geometry.depth),
                geometry,
                unknown_axis_chunks,
                ..Layout::default()
            };
            let chans: BTreeSet<String> = main.keys().map(|k| k.3.clone()).collect();
            l.channels = ordered(chans);
            l.t_values = main
                .keys()
                .map(|k| k.0)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            l.l_values = main
                .keys()
                .map(|k| k.1)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            l.z_values = main
                .keys()
                .map(|k| k.2)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            // A plane may hold more rows than the frame height (line scans): take the stored bytes.
            let row = u64::from(l.geometry.width) * u64::from(l.geometry.depth);
            if row > 0 {
                let stored = main
                    .values()
                    .map(|cs| {
                        cs.iter()
                            .map(|c| u64::from(c.plane_offset) + u64::from(c.len))
                            .max()
                            .unwrap_or(0)
                    })
                    .max()
                    .unwrap_or(0);
                let rows = stored
                    .checked_div(row)
                    .map_or(0, |r| u32::try_from(r).unwrap_or(u32::MAX));
                l.geometry.height = l.geometry.height.max(rows);
            }
            for ((t, lam, z, ch), mut chunks) in main {
                chunks.sort_by_key(|c| c.plane_offset);
                let slot = l.channels.iter().position(|c| *c == ch).unwrap_or(0);
                l.planes.insert((t, lam, z, slot), chunks);
            }
            self.layout = Some(l);
            self.exposed.push(Exposed::Main);
        }
        if !refs.is_empty() {
            self.reference = Some(self.reference_layout(ref_source, refs, &ordered));
            self.exposed.push(Exposed::Reference);
        }
    }

    fn reference_layout(
        &self,
        source: String,
        refs: BTreeMap<String, Vec<Chunk>>,
        ordered: &dyn Fn(BTreeSet<String>) -> Vec<String>,
    ) -> Reference {
        let main_geo = self
            .first_frame
            .as_ref()
            .map(|f| f.geometry.clone())
            .unwrap_or_default();
        // Camera reference images carry their own sample width.
        let camera_depth = self
            .docs
            .iter()
            .filter(|d| d.root.starts_with("cameraimage:"))
            .find_map(|d| {
                let doc = meta::parse(&d.text)?;
                doc.descendants()
                    .filter(|n| n.is_element() && n.tag_name().name() == "elementChannel")
                    .find_map(|n| {
                        n.children()
                            .find(|c| c.is_element() && c.tag_name().name() == "depth")
                            .and_then(|c| c.text())
                            .and_then(|t| t.trim().parse::<u32>().ok())
                            .filter(|d| *d > 0)
                    })
            });
        let depth = camera_depth.unwrap_or(main_geo.depth).max(1);
        let channels = ordered(refs.keys().cloned().collect());
        let first_bytes: u64 = channels
            .first()
            .and_then(|c| refs.get(c))
            .map_or(0, |cs| cs.iter().map(|c| u64::from(c.len)).sum());
        let pixels = first_bytes / u64::from(depth);
        let def = self
            .docs
            .iter()
            .find(|d| d.root.ends_with(":imageDefinition"))
            .and_then(|d| meta::parse(&d.text).map(|doc| meta::geometry(doc.root_element())));
        let sq = pixels.isqrt();
        let (w, h) = match def {
            Some(g)
                if g.width > 0
                    && g.height > 0
                    && u64::from(g.width) * u64::from(g.height) == pixels =>
            {
                (u64::from(g.width), u64::from(g.height))
            }
            _ if sq * sq == pixels => (sq, sq),
            _ if main_geo.height > 0 && pixels.is_multiple_of(u64::from(main_geo.height)) => (
                pixels / u64::from(main_geo.height),
                u64::from(main_geo.height),
            ),
            _ if main_geo.width > 0 && pixels.is_multiple_of(u64::from(main_geo.width)) => (
                u64::from(main_geo.width),
                pixels / u64::from(main_geo.width),
            ),
            _ => (pixels, 1),
        };
        let mut planes = BTreeMap::new();
        for (slot, ch) in channels.iter().enumerate() {
            if let Some(cs) = refs.get(ch) {
                let mut cs = cs.clone();
                cs.sort_by_key(|c| c.plane_offset);
                planes.insert(slot, cs);
            }
        }
        Reference {
            source,
            width: u32::try_from(w).unwrap_or(0),
            height: u32::try_from(h).unwrap_or(0),
            depth,
            pixel_type: pixel_type_for(depth),
            channels,
            planes,
        }
    }

    /// Planes the acquisition axes declare (an axis without a declaration counts what is
    /// stored), planes stored, and how the expectation was formed.
    fn expected_planes(&self, l: &Layout) -> (usize, usize, String) {
        let declared = |name: &str, have: usize| -> (usize, &'static str) {
            match self.axis(name).and_then(|a| a.max_size) {
                Some(m) => (m.max(1) as usize, "declared"),
                None => (have.max(1), "found"),
            }
        };
        let (nt, st) = declared("TIMELAPSE", l.t_values.len());
        let (nl, sl) = declared("LAMBDA", l.l_values.len());
        let (nz, sz) = declared("ZSTACK", l.z_values.len());
        let expected = nt * nl * nz * l.channels.len().max(1);
        let desc = format!(
            "T {nt} {st} x lambda {nl} {sl} x Z {nz} {sz} x {} channel(s) with pixels",
            l.channels.len()
        );
        (expected, l.planes.len(), desc)
    }

    fn channel_desc(&self, id: &str) -> Option<&meta::ChannelDesc> {
        self.props.as_ref()?.channels.iter().find(|c| c.id == id)
    }

    fn channel_settings(&self, id: &str) -> Option<&ChannelSettings> {
        self.settings.iter().find(|s| s.id == id)
    }

    fn axis(&self, name: &str) -> Option<&meta::AxisDesc> {
        self.props.as_ref()?.axes.iter().find(|a| a.axis == name)
    }

    /// Wavelength (nm) of each lambda step: first frames' `LAMBDA` positions, else the axis.
    fn lambda_nm(&self, frames: &[FrameRecord], n: usize) -> Vec<Option<f64>> {
        let mut vals: Vec<f64> = frames
            .iter()
            .flat_map(|f| {
                f.positions
                    .iter()
                    .filter(|(a, _)| a == "LAMBDA")
                    .map(|(_, v)| *v)
            })
            .collect();
        vals.sort_by(f64::total_cmp);
        vals.dedup();
        if vals.len() >= n {
            return vals.into_iter().take(n).map(Some).collect();
        }
        let ax = self.axis("LAMBDA");
        (0..n)
            .map(|i| {
                let a = ax?;
                Some(a.start? + a.step? * i as f64)
            })
            .collect()
    }

    #[allow(clippy::many_single_char_names)]
    fn main_info(&self, index: u32, l: &Layout, frames: &[FrameRecord]) -> Result<ImageInfo> {
        let g = &l.geometry;
        let pt = l.pixel_type.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("{} bytes per sample", g.depth),
                "Only 1-, 2- and 4-byte samples (uint8, uint16, float32) are known in OIR files.",
            )
        })?;
        let nl = l.l_values.len().max(1);
        let mut info = ImageInfo::new(index, g.width, g.height, pt);
        info.name = Some(file_name(&self.path));
        info.size_z = l.z_values.len().max(1) as u32;
        info.size_t = l.t_values.len().max(1) as u32;
        info.size_c = (l.channels.len().max(1) * nl) as u32;
        info.dimension_order = "XYCZT".into();
        let p = self.props.as_ref();
        let um = meta::unit_to_um(p.and_then(|p| p.pixel_unit.as_deref()));
        let scale = |v: Option<f64>| {
            v.zip(um)
                .map(|(v, u)| v * u)
                .filter(|v| *v > 0.0 && v.is_finite())
        };
        let z_step = if info.size_z > 1 {
            self.axis("ZSTACK")
                .and_then(|a| a.step)
                .map(f64::abs)
                .filter(|s| *s > 0.0)
                .or_else(|| {
                    let mut zs: Vec<f64> = frames
                        .iter()
                        .flat_map(|f| {
                            f.positions
                                .iter()
                                .filter(|(a, _)| a == "ZSTACK")
                                .map(|(_, v)| *v)
                        })
                        .collect();
                    zs.sort_by(f64::total_cmp);
                    zs.dedup();
                    (zs.len() > 1).then(|| (zs[1] - zs[0]).abs())
                })
        } else {
            None
        };
        info.physical_size = PhysicalSize::micrometres(
            scale(p.and_then(|p| p.pixel_length_x)),
            scale(p.and_then(|p| p.pixel_length_y)),
            z_step,
        );
        if info.size_t > 1 {
            let per_t = (l.l_values.len().max(1) * l.z_values.len().max(1)).max(1);
            let t_pos = |f: &FrameRecord| {
                f.positions
                    .iter()
                    .find(|(a, _)| a == "TIMELAPSE")
                    .map(|(_, v)| *v)
            };
            info.time_increment_s = match (
                frames.first().and_then(t_pos),
                frames.get(per_t).and_then(t_pos),
            ) {
                (Some(a), Some(b)) if b > a => Some((b - a) / 1000.0),
                _ => self
                    .axis("TIMELAPSE")
                    .and_then(|a| a.step)
                    .filter(|s| *s > 0.0),
            };
        }
        let lambda_nm = if l.l_values.len() > 1 {
            self.lambda_nm(frames, nl)
        } else {
            Vec::new()
        };
        let mut k = 0u32;
        for id in &l.channels {
            let desc = self.channel_desc(id);
            let set = self.channel_settings(id);
            let lasers: BTreeSet<u64> = desc
                .map(|d| {
                    d.laser_ids
                        .iter()
                        .filter_map(|lid| {
                            p?.lasers
                                .iter()
                                .find(|x| x.id == *lid && x.enabled)
                                .and_then(|x| x.wavelength_nm)
                        })
                        .map(f64::to_bits)
                        .collect()
                })
                .unwrap_or_default();
            let excitation = (lasers.len() == 1)
                .then(|| lasers.iter().next().map(|b| f64::from_bits(*b)))
                .flatten();
            let band = desc
                .and_then(|d| d.band_start_nm.zip(d.band_end_nm))
                .filter(|(a, b)| b > a)
                .map(|(a, b)| [a, b]);
            let color = self
                .luts
                .iter()
                .find(|(c, _)| c == id)
                .and_then(|(_, n)| meta::lut_color(n))
                .map(str::to_string);
            let base = desc.and_then(|d| d.name.clone());
            for li in 0..nl {
                let lam = lambda_nm.get(li).copied().flatten();
                let name = match (&base, lam) {
                    (b, Some(nm)) => Some(format!(
                        "{} {nm} nm",
                        b.clone().unwrap_or_else(|| format!("CH{}", k + 1))
                    )),
                    (b, None) => b.clone(),
                };
                info.channels.push(ChannelInfo {
                    index: k,
                    name,
                    fluorophore: set.and_then(|s| s.dye_name.clone()),
                    excitation_nm: excitation,
                    emission_nm: if l.l_values.len() > 1 {
                        lam
                    } else {
                        set.and_then(|s| s.dye_emission_nm)
                    },
                    emission_range_nm: if l.l_values.len() > 1 { None } else { band },
                    color: color.clone(),
                    ..ChannelInfo::default()
                });
                k += 1;
            }
        }
        if let Some(o) = p.and_then(|p| p.objective.as_ref()) {
            info.objective = Some(ObjectiveInfo {
                model: o.name.clone(),
                nominal_magnification: o.magnification,
                lens_na: o.numerical_aperture,
                immersion: o.immersion.clone(),
            });
        }
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("Olympus/Evident".into()),
            model: p.and_then(|p| p.system_name.clone()),
            software: Some(self.files[0].header.producer.clone()).filter(|s| !s.is_empty()),
            software_version: p.and_then(|p| p.system_version.clone()),
            detector: None,
        });
        info.acquired_at = p
            .and_then(|p| p.created.clone())
            .or_else(|| self.first_frame.as_ref().and_then(|f| f.created.clone()));
        info.extra
            .insert("significant_bits".into(), json!(g.bit_count));
        info.extra.insert("color_type".into(), json!(g.color_type));
        if let Some(m) = p.and_then(|p| p.microscope.clone()) {
            info.extra.insert("microscope".into(), json!(m));
        }
        if l.l_values.len() > 1 {
            info.extra.insert(
                "lambda".into(),
                json!({"count": l.l_values.len(), "wavelengths_nm": lambda_nm,
                       "channel_order": "channel-major: c = channel * lambda_count + lambda_index"}),
            );
        }
        let detectors: Vec<Value> = l
            .channels
            .iter()
            .map(|id| json!(self.channel_settings(id).and_then(|s| s.detector.clone())))
            .collect();
        if detectors.iter().any(|d| !d.is_null()) {
            info.extra
                .insert("channel_detectors".into(), Value::Array(detectors));
        }
        info.extra.insert("channel_ids".into(), json!(l.channels));
        if self.files.len() > 1 {
            info.extra.insert(
                "continuation_files".into(),
                json!(
                    self.files[1..]
                        .iter()
                        .map(|f| file_name(&f.path))
                        .collect::<Vec<_>>()
                ),
            );
        }
        Ok(info.finish())
    }

    fn reference_info(&self, index: u32, r: &Reference) -> Result<ImageInfo> {
        let pt = r.pixel_type.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("reference image with {} bytes per sample", r.depth),
                "Only 1-, 2- and 4-byte samples are known in OIR files.",
            )
        })?;
        let mut info = ImageInfo::new(index, r.width, r.height, pt);
        info.name = Some(format!(
            "{} [reference {}]",
            file_name(&self.path),
            r.source
        ));
        info.size_c = r.channels.len().max(1) as u32;
        let p = self.props.as_ref();
        let um = meta::unit_to_um(p.and_then(|p| p.pixel_unit.as_deref()));
        let scale = |v: Option<f64>| v.zip(um).map(|(v, u)| v * u).filter(|v| *v > 0.0);
        info.physical_size = PhysicalSize::micrometres(
            scale(p.and_then(|p| p.pixel_length_x)),
            scale(p.and_then(|p| p.pixel_length_y)),
            None,
        );
        info.channels = r
            .channels
            .iter()
            .enumerate()
            .map(|(k, id)| ChannelInfo {
                index: k as u32,
                name: self.channel_desc(id).and_then(|d| d.name.clone()),
                ..ChannelInfo::default()
            })
            .collect();
        info.acquired_at = p.and_then(|p| p.created.clone());
        info.extra.insert("kind".into(), json!("reference"));
        info.extra
            .insert("reference_source".into(), json!(r.source));
        info.extra.insert("channel_ids".into(), json!(r.channels));
        Ok(info.finish())
    }

    /// Assemble a plane from its chunks.
    fn assemble(&mut self, chunks: &[Chunk], plane_len: u64, what: &str) -> Result<Vec<u8>> {
        let len = usize::try_from(plane_len)
            .map_err(|_| Error::corrupt(FORMAT_ID, "plane size overflows"))?;
        let mut data = vec![0u8; len];
        let mut covered = 0u64;
        for c in chunks {
            let start = u64::from(c.plane_offset);
            if start >= plane_len {
                continue; // beyond the declared height (never for a consistent file)
            }
            let n = u64::from(c.len).min(plane_len - start);
            let s = start as usize;
            let f = self
                .files
                .get_mut(c.file)
                .ok_or_else(|| Error::corrupt(FORMAT_ID, "chunk refers to an unknown file"))?;
            f.reader.read_into(c.offset, &mut data[s..s + n as usize])?;
            covered += n;
        }
        if covered < plane_len {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "{what}: only {covered} of {plane_len} bytes are stored (acquisition interrupted, or a continuation file is missing)"
                ),
            ));
        }
        Ok(data)
    }

    fn plane_findings(r: &mut CheckReport, chunks: &[Chunk], plane_len: u64, what: &str) {
        let mut end = 0u64;
        for c in chunks {
            let s = u64::from(c.plane_offset);
            if s < end {
                r.push(
                    Finding::error(
                        "overlapping_chunks",
                        format!("{what}: chunks overlap at plane byte {s}"),
                    )
                    .at(c.offset),
                );
                return;
            }
            if s > end {
                r.push(
                    Finding::error(
                        "incomplete_plane",
                        format!("{what}: bytes {end}..{s} of the plane are not stored"),
                    )
                    .at(c.offset),
                );
                return;
            }
            end = s + u64::from(c.len);
        }
        if end < plane_len {
            r.push(Finding::error(
                "incomplete_plane",
                format!("{what}: {end} of {plane_len} bytes stored"),
            ));
        } else if end > plane_len {
            r.push(Finding::warning(
                "oversized_plane",
                format!("{what}: {end} bytes stored for a {plane_len}-byte plane"),
            ));
        }
    }

    fn check_file(r: &mut CheckReport, f: &OirFile) {
        let name = file_name(&f.path);
        let h = &f.header;
        if h.header_words != [12, 0, 1, 2] {
            r.push(
                Finding::info(
                    "unusual_header",
                    format!(
                        "{name}: header words {:?} differ from the usual [12, 0, 1, 2]",
                        h.header_words
                    ),
                )
                .at(16),
            );
        }
        if h.declared_size > f.file_len {
            r.push(Finding::error("truncated", format!("{name}: the header records {} bytes but the file has {} (truncated copy or interrupted write)", h.declared_size, f.file_len)).at(f.file_len));
        } else if h.declared_size < f.file_len {
            r.push(
                Finding::warning(
                    "trailing_bytes",
                    format!(
                        "{name}: {} bytes after the recorded end of the file",
                        f.file_len - h.declared_size
                    ),
                )
                .at(h.declared_size),
            );
        }
        if let Some(why) = &f.index_problem {
            r.push(Finding::error("bad_index", format!("{name}: block index unusable ({why}); blocks were recovered by walking the chain")).at(h.index_offset));
        } else if f.blocks.len() != h.block_count as usize {
            r.push(
                Finding::warning(
                    "block_count_mismatch",
                    format!(
                        "{name}: header lists {} blocks, the index has {}",
                        h.block_count,
                        f.blocks.len()
                    ),
                )
                .at(0x30),
            );
        }
        if let Some(o) = f.truncated_at {
            r.push(
                Finding::error(
                    "truncated",
                    format!("{name}: a block runs past the end of the file"),
                )
                .at(o),
            );
        }
        if let Some((o, why)) = &f.bad_block_at {
            r.push(Finding::error("bad_block", format!("{name}: {why}")).at(*o));
        }
        // the chain: contiguous from 0x60 to the index
        let mut expect = crate::container::FIRST_BLOCK_OFFSET;
        let mut reported = 0;
        for b in &f.blocks {
            if b.offset != expect && reported < 5 {
                let (code, what) = if b.offset > expect {
                    ("gap", "unused bytes before")
                } else {
                    ("overlapping_blocks", "overlap before")
                };
                r.push(
                    Finding::error(
                        code,
                        format!("{name}: {what} the {} block at {}", b.kind.name(), b.offset),
                    )
                    .at(expect),
                );
                reported += 1;
            }
            if let BlockKind::Unknown(k) = b.kind {
                r.push(
                    Finding::warning(
                        "unknown_block",
                        format!("{name}: block of unknown kind {k}"),
                    )
                    .at(b.offset),
                );
            }
            expect = b.end();
        }
        if f.index_problem.is_none() && expect != h.index_offset && reported < 5 {
            r.push(
                Finding::error(
                    "gap",
                    format!(
                        "{name}: the last block ends at {expect}, the index starts at {}",
                        h.index_offset
                    ),
                )
                .at(expect),
            );
        }
        if let Some(t) = h.thumbnail_offset
            && !f
                .blocks
                .iter()
                .any(|b: &Block| b.offset == t && b.kind == BlockKind::Thumbnail)
        {
            r.push(
                Finding::warning(
                    "bad_thumbnail_offset",
                    format!("{name}: header thumbnail offset {t} is not a thumbnail block"),
                )
                .at(0x40),
            );
        }
        for (bi, tag) in &f.chunk_tags {
            match f.pixels_after(*bi) {
                Some(p) if p.payload_len != tag.chunk_len => {
                    r.push(
                        Finding::error(
                            "chunk_length_mismatch",
                            format!(
                                "{name}: chunk '{}' declares {} bytes, its pixel block holds {}",
                                tag.name, tag.chunk_len, p.payload_len
                            ),
                        )
                        .at(p.offset),
                    );
                }
                Some(p) if p.end() > f.file_len => {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!(
                                "{name}: pixel block of chunk '{}' runs past the end of the file",
                                tag.name
                            ),
                        )
                        .at(p.offset),
                    );
                }
                _ => {}
            }
        }
    }
}

impl Dataset for OirDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .skip(1)
            .map(|f| f.path.clone())
            .filter(|p| self.fs.is_file(p))
            .collect()
    }
    fn info(&self) -> Result<FileInfo> {
        // `info` needs only the first frames (lambda values, time step): parse them on a copy of
        // the block list without touching `self`.
        let mut me = OirFrames { ds: self };
        let per_t = self
            .layout
            .as_ref()
            .map_or(1, |l| l.l_values.len().max(1) * l.z_values.len().max(1));
        let frames = me.leading(per_t + 1);
        let mut images = Vec::new();
        for (i, e) in self.exposed.iter().enumerate() {
            let info = match e {
                Exposed::Main => {
                    self.main_info(i as u32, self.layout.as_ref().expect("main"), &frames)?
                }
                Exposed::Reference => {
                    self.reference_info(i as u32, self.reference.as_ref().expect("reference"))?
                }
            };
            images.push(info);
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let mut notes = Vec::new();
        if self.layout.is_none() && self.reference.is_none() {
            let g = self.first_frame.as_ref().map(|f| &f.geometry);
            notes.push(format!(
                "no pixel data is stored in this file (metadata only{}); `info --view full` shows its XML documents",
                g.map(|g| format!(", frame properties declare a {}x{} canvas", g.width, g.height)).unwrap_or_default()
            ));
        }
        if self
            .files
            .iter()
            .any(|f| f.truncated_at.is_some() || f.header.declared_size > f.file_len)
        {
            notes.push("file appears truncated; run `check`".into());
        }
        if let Some(l) = &self.layout {
            let (expected, stored, _) = self.expected_planes(l);
            if stored < expected {
                notes.push(format!("only {stored} of {expected} planes declared by the acquisition are stored (interrupted acquisition or missing continuation file); the sizes above count stored planes; run `check`"));
            }
        }
        if self.files.len() > 1 {
            notes.push(format!(
                "{} continuation file(s) read: {}",
                self.files.len() - 1,
                self.files[1..]
                    .iter()
                    .map(|f| file_name(&f.path))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.layout.as_ref().is_some_and(|l| l.l_values.len() > 1) {
            notes.push("lambda (spectral) series are exposed as channels: size_c = channels x lambda steps; channel names carry the wavelength".into());
        }
        if self.reference.is_some() {
            notes.push("the reference image (REF_ chunks) is exposed as a separate image".into());
        }
        if let Some(l) = &self.layout
            && l.unknown_axis_chunks > 0
        {
            notes.push(format!(
                "{} chunk(s) carry axis letters other than t/l/z and are not exposed; run `check`",
                l.unknown_axis_chunks
            ));
        }
        if !self.open_findings.is_empty() {
            notes.push(format!(
                "{} problem(s) found while indexing; run `check`",
                self.open_findings.len()
            ));
        }
        let version = self
            .docs
            .iter()
            .find(|d| d.root.ends_with(":fileInfomation"))
            .and_then(|d| {
                let doc = meta::parse(&d.text)?;
                doc.root_element()
                    .children()
                    .find(|c| c.is_element() && c.tag_name().name() == "version")
                    .and_then(|c| c.text())
                    .map(|t| t.trim().to_string())
            });
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.files[0].file_len,
            format: OirReader.descriptor(),
            format_version: version,
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut by_kind: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for d in &self.docs {
            let local = d.root.rsplit(':').next().unwrap_or(&d.root);
            let key = match local {
                "fileInfomation" => "file_info",
                "imageProperties" => "image_properties",
                "annotationStore" => "annotations",
                "contents" => "overlays",
                "LUT" => "luts",
                "lsmChannel" => "channel_settings",
                "imageDefinition" => "image_definitions",
                "eventList" => "event_list",
                _ if d.root.starts_with("cameraimage:") => "camera_images",
                _ => "other_documents",
            };
            let mut v = doc_json(d);
            if let (Some(ch), Value::Object(o)) = (&d.channel, &mut v) {
                o.insert("@channel".into(), json!(ch));
            }
            by_kind.entry(key).or_default().push(v);
        }
        let mut out = serde_json::Map::new();
        for (k, mut v) in by_kind {
            let single =
                matches!(k, "file_info" | "image_properties" | "event_list") && v.len() == 1;
            out.insert(k.into(), if single { v.remove(0) } else { Value::Array(v) });
        }
        let mut me = OirFrames { ds: self };
        let first = me.first_doc();
        out.insert(
            "frame_properties".into(),
            json!({"count": self.frame_blocks.len(), "first": first,
                   "note": "one document per frame; all of them are summarized by `info --view full` under images[0].extra.frames"}),
        );
        Ok(Value::Object(out))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_c", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].dimension_order", Source::Inferred),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].physical_size", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].channels[].name", Source::PriorArt),
            ("images[].channels[].fluorophore", Source::Inferred),
            ("images[].channels[].excitation_nm", Source::Inferred),
            ("images[].channels[].emission_nm", Source::Inferred),
            ("images[].channels[].emission_range_nm", Source::PriorArt),
            ("images[].channels[].color", Source::Inferred),
            ("images[].objective", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].extra.lambda", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (fi, f) in self.files.iter().enumerate() {
            let fname = file_name(&f.path);
            out.push(LsEntry {
                kind: if fi == 0 { "header" } else { "continuation" }.into(),
                name: fname.clone(),
                offset: Some(0),
                size: Some(crate::container::FIRST_BLOCK_OFFSET),
                image: None,
                details: json!({"declared_size": f.header.declared_size, "index_offset": f.header.index_offset,
                                "block_count": f.header.block_count, "thumbnail_offset": f.header.thumbnail_offset,
                                "producer": f.header.producer, "file_size": f.file_len}),
            });
            out.push(LsEntry {
                kind: "index".into(),
                name: format!("{fname}: block index"),
                offset: Some(f.header.index_offset),
                size: Some(f.file_len.saturating_sub(f.header.index_offset)),
                image: None,
                details: json!({"entries": f.blocks.len(), "recovered_by_chain_walk": f.index_problem.is_some()}),
            });
            let mut counts: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
            for b in &f.blocks {
                let e = counts.entry(b.kind.name()).or_default();
                e.0 += 1;
                e.1 += u64::from(b.payload_len);
                if matches!(b.kind, BlockKind::Documents | BlockKind::Thumbnail) {
                    out.push(LsEntry {
                        kind: "block".into(),
                        name: format!("{fname}: {} block", b.kind.name()),
                        offset: Some(b.offset),
                        size: Some(8 + u64::from(b.payload_len)),
                        image: None,
                        details: json!({"kind": b.kind.code()}),
                    });
                }
            }
            out.push(LsEntry {
                kind: "block-summary".into(),
                name: format!("{fname}: blocks by kind"),
                offset: None,
                size: None,
                image: None,
                details: json!(
                    counts
                        .iter()
                        .map(|(k, (n, b))| json!({"kind": k, "count": n, "payload_bytes": b}))
                        .collect::<Vec<_>>()
                ),
            });
        }
        for d in &self.docs {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: d.root.clone(),
                offset: Some(d.offset),
                size: Some(d.text.len() as u64),
                image: None,
                details: json!({"encoding": "ascii-xml", "channel": d.channel}),
            });
        }
        for (i, e) in self.exposed.iter().enumerate() {
            match e {
                Exposed::Main => {
                    let l = self.layout.as_ref().expect("main");
                    let nl = l.l_values.len().max(1);
                    let mut keys: Vec<(&PlaneKey, &Vec<Chunk>)> = l.planes.iter().collect();
                    keys.sort_by_key(|(k, _)| **k);
                    for (k, chunks) in keys {
                        let (t, lam, z, slot) = *k;
                        let ti = l.t_values.iter().position(|v| *v == t).unwrap_or(0);
                        let li = l.l_values.iter().position(|v| *v == lam).unwrap_or(0);
                        let zi = l.z_values.iter().position(|v| *v == z).unwrap_or(0);
                        let c = slot * nl + li;
                        out.push(LsEntry {
                            kind: "plane".into(),
                            name: format!("c{c} z{zi} t{ti}"),
                            offset: chunks.first().map(|c| c.offset),
                            size: Some(chunks.iter().map(|c| u64::from(c.len)).sum()),
                            image: Some(i as u32),
                            details: json!({"channel_id": l.channels.get(slot), "chunks": chunks.len(),
                                            "files": chunks.iter().map(|c| c.file).collect::<BTreeSet<_>>(),
                                            "name_indices": {"t": t, "lambda": lam, "z": z}}),
                        });
                    }
                }
                Exposed::Reference => {
                    let r = self.reference.as_ref().expect("reference");
                    for (slot, chunks) in &r.planes {
                        out.push(LsEntry {
                            kind: "plane".into(),
                            name: format!("reference c{slot}"),
                            offset: chunks.first().map(|c| c.offset),
                            size: Some(chunks.iter().map(|c| u64::from(c.len)).sum()),
                            image: Some(i as u32),
                            details: json!({"channel_id": r.channels.get(*slot), "chunks": chunks.len()}),
                        });
                    }
                }
            }
        }
        for a in self.attachments()? {
            out.push(LsEntry {
                kind: "attachment".into(),
                name: a.name.clone(),
                offset: a.offset,
                size: Some(a.size),
                image: None,
                details: json!({"content_type": a.content_type}),
            });
        }
        Ok(out)
    }

    #[allow(clippy::many_single_char_names)]
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let e = *self.exposed.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image {image} does not exist (file has {} image(s))",
                self.exposed.len()
            ))
        })?;
        match e {
            Exposed::Main => {
                let l = self.layout.as_ref().expect("main");
                let pt = l.pixel_type.ok_or_else(|| {
                    Error::unsupported(
                        FORMAT_ID,
                        format!("{} bytes per sample", l.geometry.depth),
                        "Only 1-, 2- and 4-byte samples are known in OIR files.",
                    )
                })?;
                let nl = l.l_values.len().max(1);
                let size_c = l.channels.len().max(1) * nl;
                let (c, z, t) = (index.c as usize, index.z as usize, index.t as usize);
                if c >= size_c || z >= l.z_values.len().max(1) || t >= l.t_values.len().max(1) {
                    return Err(Error::Usage(format!(
                        "plane c={c} z={z} t={t} is out of range (C={size_c}, Z={}, T={})",
                        l.z_values.len().max(1),
                        l.t_values.len().max(1)
                    )));
                }
                let key = (
                    l.t_values.get(t).copied().unwrap_or(0),
                    l.l_values.get(c % nl).copied().unwrap_or(0),
                    l.z_values.get(z).copied().unwrap_or(0),
                    c / nl,
                );
                let what = format!("plane c={c} z={z} t={t}");
                let chunks = l.planes.get(&key).cloned().ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, format!("{what} is not stored (acquisition interrupted, or a continuation file is missing)"))
                })?;
                let (w, h, len) = (l.geometry.width, l.geometry.height, l.plane_len());
                let data = self.assemble(&chunks, len, &what)?;
                Ok(Plane {
                    width: w,
                    height: h,
                    pixel_type: pt,
                    samples_per_pixel: 1,
                    data,
                })
            }
            Exposed::Reference => {
                let r = self.reference.as_ref().expect("reference");
                let pt = r.pixel_type.ok_or_else(|| {
                    Error::unsupported(
                        FORMAT_ID,
                        format!("reference image with {} bytes per sample", r.depth),
                        "Only 1-, 2- and 4-byte samples are known in OIR files.",
                    )
                })?;
                if index.z != 0 || index.t != 0 || index.c as usize >= r.channels.len().max(1) {
                    return Err(Error::Usage(format!(
                        "plane c={} z={} t={} is out of range (C={}, Z=1, T=1)",
                        index.c,
                        index.z,
                        index.t,
                        r.channels.len()
                    )));
                }
                let chunks = r
                    .planes
                    .get(&(index.c as usize))
                    .cloned()
                    .unwrap_or_default();
                let len = u64::from(r.width) * u64::from(r.height) * u64::from(r.depth);
                let (w, h) = (r.width, r.height);
                let data =
                    self.assemble(&chunks, len, &format!("reference plane c={}", index.c))?;
                Ok(Plane {
                    width: w,
                    height: h,
                    pixel_type: pt,
                    samples_per_pixel: 1,
                    data,
                })
            }
        }
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "header: signature, recorded file size, index offset, block count, thumbnail offset",
        );
        r.performed("block chain: blocks contiguous from byte 96 to the index, kinds known, nothing past the end of the file");
        r.performed("chunk tags: each followed by a pixel block of the declared length");
        r.performed("planes: every plane's chunks cover it exactly (no gap, no overlap)");
        r.performed("planes present for every channel x lambda x z x t declared by the acquisition axes (continuation files included)");
        r.performed(
            "XML documents parse; channels with pixels are described in the image properties",
        );
        for f in &self.files {
            Self::check_file(&mut r, f);
        }
        for f in &self.open_findings {
            r.push(f.clone());
        }
        let mut xml_bad = 0;
        for d in &self.docs {
            if meta::parse(&d.text).is_none() {
                xml_bad += 1;
                if xml_bad <= 5 {
                    r.push(
                        Finding::error("bad_xml", format!("{} document does not parse", d.root))
                            .at(d.offset),
                    );
                }
            }
        }
        if self.docs.is_empty() {
            r.push(Finding::error(
                "missing_metadata",
                "no XML metadata documents found",
            ));
        } else if self.props.is_none() {
            r.push(Finding::warning(
                "missing_metadata",
                "no image-properties document: channel names, pixel size and objective are unknown",
            ));
        }
        if let Some(l) = &self.layout {
            let plane_len = l.plane_len();
            let mut keys: Vec<&PlaneKey> = l.planes.keys().collect();
            keys.sort();
            for k in keys {
                let what = format!("plane t{} l{} z{} channel {}", k.0, k.1, k.2, k.3);
                Self::plane_findings(&mut r, &l.planes[k], plane_len, &what);
            }
            for id in &l.channels {
                if self.props.is_some() && self.channel_desc(id).is_none() {
                    r.push(Finding::warning(
                        "unknown_channel",
                        format!("pixels for channel {id} but no channel description"),
                    ));
                }
            }
            if l.unknown_axis_chunks > 0 {
                r.push(Finding::warning(
                    "unknown_axis",
                    format!(
                        "{} chunk(s) name axes other than t/l/z; they are not exposed",
                        l.unknown_axis_chunks
                    ),
                ));
            }
            // declared vs stored planes
            let (expected, stored, desc) = self.expected_planes(l);
            if stored < expected {
                let cont = if self.fs.is_file(&self.next_continuation) {
                    String::new()
                } else {
                    format!(
                        "; if the acquisition was split, continuation file {} is missing",
                        file_name(&self.next_continuation)
                    )
                };
                r.push(Finding::error(
                    "missing_planes",
                    format!("{expected} planes expected ({desc}); {stored} are stored{cont}"),
                ));
            }
            for (name, vals) in [
                ("t", &l.t_values),
                ("lambda", &l.l_values),
                ("z", &l.z_values),
            ] {
                let contiguous =
                    vals.iter().enumerate().all(|(i, v)| *v as usize == i + 1) || vals == &[0];
                if !contiguous {
                    r.push(Finding::warning(
                        "sparse_axis",
                        format!(
                            "{name} indices in chunk names are not 1..{}: {:?}",
                            vals.len(),
                            vals.iter().take(8).collect::<Vec<_>>()
                        ),
                    ));
                }
            }
            let frames = self.frame_blocks.len();
            let per = l.t_values.len().max(1) * l.l_values.len().max(1) * l.z_values.len().max(1);
            if frames != 0 && frames != per {
                r.push(Finding::warning(
                    "frame_count_mismatch",
                    format!("{frames} frame-properties blocks for {per} frames"),
                ));
            }
        }
        if let Some(rf) = &self.reference {
            let len = u64::from(rf.width) * u64::from(rf.height) * u64::from(rf.depth);
            for (slot, chunks) in &rf.planes {
                Self::plane_findings(&mut r, chunks, len, &format!("reference plane c{slot}"));
            }
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        let f = &self.files[0];
        Ok(f.blocks
            .iter()
            .filter(|b| {
                b.kind == BlockKind::Thumbnail && b.payload_len > 4 && b.end() <= f.file_len
            })
            .enumerate()
            .map(|(i, b)| AttachmentInfo {
                index: i as u32,
                name: format!(
                    "thumbnail{}",
                    if i == 0 {
                        String::new()
                    } else {
                        format!(" {i}")
                    }
                ),
                content_type: "BMP".into(),
                extension: "bmp".into(),
                offset: Some(b.payload_offset() + 4),
                size: u64::from(b.payload_len) - 4,
                extra: BTreeMap::new(),
            })
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let a = self
            .attachments()?
            .into_iter()
            .nth(index as usize)
            .ok_or_else(|| Error::Usage(format!("attachment {index} does not exist")))?;
        let off = a.offset.unwrap_or(0);
        let n = usize::try_from(a.size)
            .map_err(|_| Error::corrupt(FORMAT_ID, "attachment too large"))?;
        let mut buf = vec![0u8; n];
        self.files[0].reader.read_into(off, &mut buf)?;
        Ok(buf)
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        if self.exposed.get(image as usize) != Some(&Exposed::Main) {
            return Ok((0, Vec::new()));
        }
        let l = self.layout.as_ref().expect("main");
        let total = self.frame_blocks.len();
        let mut me = OirFrames { ds: self };
        let recs = me.leading(limit.unwrap_or(total).min(total));
        let pos =
            |f: &FrameRecord, a: &str| f.positions.iter().find(|(x, _)| x == a).map(|(_, v)| *v);
        let out = recs
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let (t, lam, z) = frame_axes(&f.name).unwrap_or((0, 0, 0));
                let idx = |vals: &[u32], v: u32| vals.iter().position(|x| *x == v);
                json!({
                    "index": i,
                    "name": f.name,
                    "t": idx(&l.t_values, t),
                    "lambda": idx(&l.l_values, lam),
                    "z": idx(&l.z_values, z),
                    "time_s": pos(f, "TIMELAPSE").map(|ms| ms / 1000.0),
                    "z_um": pos(f, "ZSTACK"),
                    "lambda_nm": pos(f, "LAMBDA"),
                    "created": f.created,
                })
            })
            .collect();
        Ok((total as u64, out))
    }
}

/// Frame-property reads for `&self` methods: opens its own file handles (the dataset's are
/// `&mut`), so `info` and `frames` stay `&self`.
struct OirFrames<'a> {
    ds: &'a OirDataset,
}

impl OirFrames<'_> {
    fn read_docs(&mut self, fi: usize, bi: usize) -> Option<Vec<XmlDoc>> {
        let f = self.ds.files.get(fi)?;
        let b = f.blocks.get(bi)?;
        let n = usize::try_from(b.payload_len).ok()?;
        let mut buf = vec![0u8; n];
        let mut file = self.ds.fs.open(&f.path).ok()?;
        file.seek(SeekFrom::Start(b.payload_offset())).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(documents(&buf, b.payload_offset()))
    }

    fn leading(&mut self, n: usize) -> Vec<FrameRecord> {
        let blocks: Vec<(usize, usize)> = self.ds.frame_blocks.iter().take(n).copied().collect();
        blocks
            .into_iter()
            .filter_map(|(fi, bi)| {
                let docs = self.read_docs(fi, bi)?;
                frame_record(&docs.first()?.text)
            })
            .collect()
    }

    fn first_doc(&mut self) -> Value {
        let Some(&(fi, bi)) = self.ds.frame_blocks.first() else {
            return Value::Null;
        };
        self.read_docs(fi, bi)
            .and_then(|d| d.first().map(doc_json))
            .unwrap_or(Value::Null)
    }
}

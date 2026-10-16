//! `Dataset` for ParaVision experiments and studies: one image (or spectrum) per
//! reconstruction.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry, PhysicalSize,
    SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use super::params::{Atom, Params, parse};
use super::{FORMAT_ID, ParavisionReader, Reco, recos};
use crate::util::{num, plane_of_normal};

/// Parameters whose values identify a person (the subject or a user); withheld.
const PERSONAL: &[(&str, &str)] = &[
    ("VisuSubjectName", "person_name"),
    ("VisuSubjectId", "patient_id"),
    ("VisuSubjectBirthDate", "date_of_birth"),
    ("VisuStudyReferringPhysician", "person_name"),
    ("VisuSubjectComment", "free_text"),
    ("SUBJECT_name_string", "person_name"),
    ("SUBJECT_name", "person_name"),
    ("SUBJECT_id", "patient_id"),
    ("SUBJECT_dbirth", "date_of_birth"),
    ("SUBJECT_referral", "person_name"),
    ("OWNER", "person_name"),
];

/// Frame groups that are steps in time; the slice group is Z; every other group is a channel.
fn axis_of(group: &str) -> char {
    match group {
        "FG_SLICE" => 'z',
        "FG_CYCLE" | "FG_MOVIE" => 't',
        _ => 'c',
    }
}

#[derive(Debug, Clone)]
struct Group {
    count: u32,
    name: String,
}

/// One reconstruction, parsed.
#[derive(Debug)]
struct Recon {
    reco: Reco,
    visu: Params,
    method: Option<Params>,
    acqp: Option<Params>,
    file: SourceFile,
    file_len: u64,
    core: Vec<u32>,
    frames: u32,
    groups: Vec<Group>,
    /// Groups laid out as (z, t, c), each a list of group indices, first fastest.
    axes: [Vec<usize>; 3],
    sizes: (u32, u32, u32),
    bytes: usize,
    stored: PixelType,
    little: bool,
    slopes: Vec<f64>,
    offsets: Vec<f64>,
    spectrum: bool,
    notes: Vec<String>,
}

fn word_type(w: &str) -> Option<(usize, PixelType)> {
    Some(match w {
        "_8BIT_UNSGN_INT" => (1, PixelType::Uint8),
        "_16BIT_SGN_INT" => (2, PixelType::Int16),
        "_32BIT_SGN_INT" => (4, PixelType::Int32),
        "_32BIT_FLOAT" => (4, PixelType::Float),
        _ => return None,
    })
}

fn read_params(input: &Input, p: &std::path::Path) -> Option<Params> {
    let fs = input.fs();
    if !fs.is_file(p) {
        return None;
    }
    let b = openreadout_core::limits::read_metadata_file(FORMAT_ID, fs, p).ok()?;
    Some(parse(&b))
}

impl Recon {
    fn open(input: &Input, reco: Reco) -> Result<Recon> {
        let fs = input.fs();
        let visu_path = reco.proc_dir.join("visu_pars");
        let visu = read_params(input, &visu_path).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("{} has no visu_pars", reco.proc_dir.display()),
                "ParaVision 5 reconstructions without visu_pars are not read; export them to DICOM or NIfTI in ParaVision.",
            )
        })?;
        let method = read_params(input, &reco.exp_dir.join("method"));
        let acqp = read_params(input, &reco.exp_dir.join("acqp"));
        let seq = reco.proc_dir.join("2dseq");
        let file = fs.open(&seq).map_err(|e| Error::io(&seq, e))?;
        let file_len = file.size().map_err(|e| Error::io(&seq, e))?;
        let core: Vec<u32> = visu
            .numbers("VisuCoreSize")
            .iter()
            .map(|v| {
                if *v >= 1.0 && *v <= f64::from(u32::MAX) {
                    *v as u32
                } else {
                    0
                }
            })
            .collect();
        if core.is_empty() || core.contains(&0) || core.len() > 3 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "{}: VisuCoreSize {:?} is not 1 to 3 positive sizes",
                    visu_path.display(),
                    core
                ),
            ));
        }
        let frames = visu
            .number("VisuCoreFrameCount")
            .map_or(1, |v| v.max(1.0) as u32);
        let wt = visu.text("VisuCoreWordType").unwrap_or_default();
        let (bytes, stored) = word_type(&wt).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("VisuCoreWordType {wt}"),
                "Only 8-bit unsigned, 16- and 32-bit signed and 32-bit float 2dseq data are read.",
            )
        })?;
        let little = visu
            .text("VisuCoreByteOrder")
            .is_none_or(|b| b != "bigEndian");
        let groups: Vec<Group> = visu
            .get("VisuFGOrderDesc")
            .map(|p| {
                p.structs()
                    .iter()
                    .filter_map(|s| {
                        let count = match s.first()? {
                            Atom::Number(n) if *n >= 1.0 => *n as u32,
                            _ => return None,
                        };
                        let name = match s.get(1)? {
                            Atom::Text(t) | Atom::Word(t) => t.clone(),
                            _ => String::new(),
                        };
                        Some(Group { count, name })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut notes = Vec::new();
        let product: u64 = groups.iter().map(|g| u64::from(g.count)).product();
        let mut axes: [Vec<usize>; 3] = Default::default();
        let groups = if !groups.is_empty() && product == u64::from(frames) {
            groups
        } else {
            if !groups.is_empty() {
                notes.push(format!(
                    "the frame groups describe {product} frames but VisuCoreFrameCount is {frames}: frames are listed as T"
                ));
            }
            vec![Group {
                count: frames,
                name: if core.len() == 3 {
                    "FG_CYCLE".into()
                } else {
                    "frames".into()
                },
            }]
        };
        for (i, g) in groups.iter().enumerate() {
            let a = match axis_of(&g.name) {
                'z' if core.len() < 3 => 0,
                'z' => 2, // a 3-D core already fills Z
                't' => 1,
                _ if g.name == "frames" => 1,
                _ => 2,
            };
            axes[a].push(i);
        }
        let prod = |ix: &[usize]| ix.iter().map(|&i| groups[i].count).product::<u32>().max(1);
        let z = if core.len() == 3 {
            core[2]
        } else {
            prod(&axes[0])
        };
        let sizes = (z, prod(&axes[1]), prod(&axes[2]));
        let spectrum = core.len() == 1
            && visu
                .get("VisuCoreDimDesc")
                .is_some_and(|d| d.texts().iter().any(|t| t == "spectroscopic"));
        let slopes = visu.numbers("VisuCoreDataSlope");
        let offsets = visu.numbers("VisuCoreDataOffs");
        Ok(Recon {
            reco,
            visu,
            method,
            acqp,
            file,
            file_len,
            core,
            frames,
            groups,
            axes,
            sizes,
            bytes,
            stored,
            little,
            slopes,
            offsets,
            spectrum,
            notes,
        })
    }

    fn frame_px(&self) -> u64 {
        self.core.iter().map(|&v| u64::from(v)).product()
    }

    #[allow(clippy::float_cmp)]
    fn scaled(&self) -> bool {
        self.slopes.iter().any(|s| *s != 1.0) || self.offsets.iter().any(|o| *o != 0.0)
    }

    fn out_type(&self) -> PixelType {
        if self.scaled() {
            PixelType::Float
        } else {
            self.stored
        }
    }

    /// The frame index of (z, t, c) (z only for 2-D cores), first group fastest.
    #[allow(clippy::many_single_char_names)]
    fn frame_of(&self, z: u32, t: u32, c: u32) -> u64 {
        let mut stride = vec![1u64; self.groups.len()];
        for i in 1..self.groups.len() {
            stride[i] = stride[i - 1] * u64::from(self.groups[i - 1].count);
        }
        let mut f = 0u64;
        let coords = [if self.core.len() == 3 { 0 } else { z }, t, c];
        for (a, ix) in self.axes.iter().enumerate() {
            if a == 0 && self.core.len() == 3 {
                continue;
            }
            let mut v = coords[a];
            for &g in ix {
                let n = self.groups[g].count.max(1);
                f += u64::from(v % n) * stride[g];
                v /= n;
            }
        }
        f
    }

    fn slope_offset(&self, frame: u64) -> (f64, f64) {
        let pick = |v: &[f64], d: f64| v.get(frame as usize).or(v.first()).copied().unwrap_or(d);
        (pick(&self.slopes, 1.0), pick(&self.offsets, 0.0))
    }

    fn read_samples(&self, offset: u64, n: u64) -> Result<Vec<u8>> {
        let len = n
            .checked_mul(self.bytes as u64)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "read size overflows"))?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "offset overflows"))?;
        if end > self.file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                offset,
                format!(
                    "2dseq of {}/pdata/{} holds {} bytes; this read needs {end} (truncated?)",
                    self.reco.expno, self.reco.procno, self.file_len
                ),
            ));
        }
        let mut buf = vec![
            0u8;
            usize::try_from(len)
                .map_err(|_| Error::corrupt(FORMAT_ID, "read too large"))?
        ];
        self.file
            .read_exact_at(offset, &mut buf)
            .map_err(|e| Error::Other(format!("2dseq read failed: {e}")))?;
        if !self.little && self.bytes > 1 {
            for c in buf.chunks_exact_mut(self.bytes) {
                c.reverse();
            }
        }
        Ok(buf)
    }

    fn scale(&self, raw: &[u8], frame: u64) -> Vec<u8> {
        if !self.scaled() {
            return raw.to_vec();
        }
        let (s, o) = self.slope_offset(frame);
        let mut out = Vec::with_capacity(raw.len() / self.bytes * 4);
        for x in self.stored.samples_f64(raw) {
            out.extend(((x * s + o) as f32).to_le_bytes());
        }
        out
    }

    fn name(&self) -> String {
        let proto = self.visu.text("VisuAcquisitionProtocol");
        let seq = self.visu.text("VisuAcqSequenceName");
        let label = proto.or(seq).unwrap_or_else(|| "scan".into());
        format!("E{} P{}: {label}", self.reco.expno, self.reco.procno)
    }

    fn extra(&self, ex: &mut BTreeMap<String, Value>) {
        let v = &self.visu;
        ex.insert("expno".into(), Value::from(self.reco.expno.clone()));
        ex.insert("procno".into(), Value::from(self.reco.procno.clone()));
        if let Some(m) = self
            .method
            .as_ref()
            .and_then(|m| m.text("Method"))
            .or_else(|| v.text("VisuAcqSequenceName"))
        {
            ex.insert("method".into(), Value::from(m));
        }
        if let Some(p) = v.text("VisuAcquisitionProtocol") {
            ex.insert("protocol".into(), Value::from(p));
        }
        ex.insert(
            "word_type".into(),
            Value::from(v.text("VisuCoreWordType").unwrap_or_default()),
        );
        ex.insert(
            "frame_groups".into(),
            Value::Array(
                self.groups
                    .iter()
                    .map(|g| Value::from(g.name.clone()))
                    .collect(),
            ),
        );
        ex.insert("frame_count".into(), Value::from(self.frames));
        if self.scaled() {
            let distinct = {
                let mut s = self.slopes.clone();
                s.dedup();
                s.len()
            };
            ex.insert(
                "rescale".into(),
                if distinct <= 1 {
                    json!({"slope": num(self.slope_offset(0).0), "intercept": num(self.slope_offset(0).1), "applied": true})
                } else {
                    json!({"varies": true, "frames": self.slopes.len(), "applied": true})
                },
            );
            ex.insert(
                "stored_pixel_type".into(),
                Value::from(self.stored.ome_name()),
            );
        }
        let mut acq = Map::new();
        for (k, p, scale) in [
            ("repetition_time_ms", "VisuAcqRepetitionTime", 1.0),
            ("echo_time_ms", "VisuAcqEchoTime", 1.0),
            ("inversion_time_ms", "VisuAcqInversionTime", 1.0),
            ("flip_angle_deg", "VisuAcqFlipAngle", 1.0),
            ("averages", "VisuAcqNumberOfAverages", 1.0),
            ("echo_train_length", "VisuAcqEchoTrainLength", 1.0),
            ("imaging_frequency_mhz", "VisuAcqImagingFrequency", 1.0),
            ("scan_time_s", "VisuAcqScanTime", 1e-3),
        ] {
            if let Some(x) = v.number(p) {
                acq.insert(k.into(), num(x * scale));
            }
        }
        if let Some(n) = v.text("VisuAcqImagedNucleus") {
            acq.insert("nucleus".into(), Value::from(n));
        }
        if let Some(c) = v.text("VisuCoilReceiveName") {
            acq.insert("receive_coil".into(), Value::from(c));
        }
        if let Some(f) = v.number("VisuAcqImagingFrequency").filter(|f| *f > 0.0) {
            // field strength from the 1H Larmor frequency (42.577478 MHz/T)
            if v.text("VisuAcqImagedNucleus")
                .as_deref()
                .is_none_or(|n| n == "1H")
            {
                acq.insert(
                    "field_strength_t".into(),
                    num((f / 42.577_478_5 * 100.0).round() / 100.0),
                );
            }
        }
        if !acq.is_empty() {
            ex.insert("acquisition".into(), Value::Object(acq));
        }
        if let Some(units) = v
            .get("VisuCoreDataUnits")
            .map(super::params::Param::texts)
            .filter(|u| !u.is_empty() && u.iter().any(|s| !s.is_empty()))
        {
            ex.insert("value_unit".into(), Value::from(units[0].clone()));
        }
        let mut subject = Map::new();
        if let Some(s) = v.text("VisuSubjectSex") {
            subject.insert("sex".into(), Value::from(s));
        }
        if let Some(t) = v.text("VisuSubjectType") {
            subject.insert("type".into(), Value::from(t));
        }
        if let Some(w) = v.number("VisuSubjectWeight").filter(|w| *w > 0.0) {
            subject.insert("weight_kg".into(), num(w));
        }
        if let Some(p) = v.text("VisuSubjectPosition") {
            subject.insert("position".into(), Value::from(p));
        }
        if !subject.is_empty() {
            ex.insert("subject".into(), Value::Object(subject));
        }
        let o = v.numbers("VisuCoreOrientation");
        if o.len() >= 9 {
            let n = [o[6], o[7], o[8]];
            ex.insert(
                "orientation".into(),
                json!({"plane": plane_of_normal(n), "matrix": o[..9].iter().map(|x| num(*x)).collect::<Vec<_>>()}),
            );
        }
        let pd: Vec<Value> = PERSONAL
            .iter()
            .filter(|(k, _)| {
                [Some(&self.visu), self.method.as_ref(), self.acqp.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|p| {
                        p.get(k)
                            .is_some_and(|x| x.texts().iter().any(|s| !s.trim().is_empty()))
                    })
            })
            .map(|(k, kind)| json!({"field": k, "kind": kind}))
            .collect();
        if !pd.is_empty() {
            ex.insert("personal_data".into(), Value::Array(pd));
        }
    }

    fn channel_names(&self) -> Vec<String> {
        // element comments of the channel groups (VisuFGElemComment), when one group is a channel
        let c = &self.axes[2];
        if c.len() == 1 {
            let g = c[0];
            let start: usize = self.groups[..g].iter().map(|g| g.count as usize).sum();
            let comments = self
                .visu
                .get("VisuFGElemComment")
                .map(super::params::Param::texts)
                .unwrap_or_default();
            let n = self.groups[g].count as usize;
            if comments.len() >= start + n {
                return comments[start..start + n].to_vec();
            }
            let base = self.groups[g]
                .name
                .trim_start_matches("FG_")
                .to_ascii_lowercase();
            return (0..n).map(|i| format!("{base} {i}")).collect();
        }
        Vec::new()
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self, index: u32) -> ImageInfo {
        let x = self.core[0];
        let y = self.core.get(1).copied().unwrap_or(1);
        let (nz, nt, nc) = self.sizes;
        let mut im = ImageInfo::new(index, x, y, self.out_type());
        im.size_z = nz;
        im.size_t = nt;
        im.size_c = nc;
        im.name = Some(self.name());
        let v = &self.visu;
        let extent = v.numbers("VisuCoreExtent");
        let units = v
            .get("VisuCoreUnits")
            .map(super::params::Param::texts)
            .unwrap_or_default();
        let to_um = |i: usize| -> Option<f64> {
            let e = *extent.get(i)?;
            let n = f64::from(*self.core.get(i)?);
            let u = units.get(i).map_or("mm", String::as_str);
            let f = match u {
                "mm" => 1e3,
                "um" | "µm" => 1.0,
                "cm" => 1e4,
                _ => return None,
            };
            (e > 0.0 && n > 0.0).then(|| e / n * f)
        };
        let z = if self.core.len() == 3 {
            to_um(2)
        } else if nz > 1 {
            v.number("VisuCoreSlicePacksSliceDist")
                .or_else(|| {
                    let p = v.numbers("VisuCorePosition");
                    (p.len() >= 6).then(|| {
                        ((p[3] - p[0]).powi(2) + (p[4] - p[1]).powi(2) + (p[5] - p[2]).powi(2))
                            .sqrt()
                    })
                })
                .map(|mm| mm * 1e3)
        } else {
            None
        };
        im.physical_size = PhysicalSize::micrometres(to_um(0), to_um(1), z);
        let names = self.channel_names();
        im.channels = (0..nc)
            .map(|c| ChannelInfo {
                index: c,
                name: names.get(c as usize).cloned().filter(|s| !s.is_empty()),
                ..ChannelInfo::default()
            })
            .collect();
        im.instrument = Some(InstrumentInfo {
            manufacturer: v.text("VisuManufacturer"),
            model: v.text("VisuStation"),
            software: Some("ParaVision".into()),
            software_version: v
                .text("VisuCreatorVersion")
                .or_else(|| v.text("VisuAcqSoftwareVersion")),
            detector: None,
        });
        im.acquired_at = v.text("VisuAcqDate").map(|d| d.replace(',', "."));
        self.extra(&mut im.extra);
        im.finish()
    }
}

/// An opened ParaVision experiment or study.
#[derive(Debug)]
pub(crate) struct ParavisionDataset {
    path: PathBuf,
    recons: Vec<Recon>,
    /// Image index → reconstruction; trace index → reconstruction.
    images: Vec<usize>,
    traces: Vec<usize>,
    skipped: Vec<String>,
}

impl ParavisionDataset {
    pub(crate) fn open(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let list = recos(input.fs(), &path).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "{} holds no pdata/<procno>/2dseq reconstructions",
                    path.display()
                ),
            )
        })?;
        let mut recons = Vec::new();
        let mut skipped = Vec::new();
        let single = list.len() == 1;
        for r in list {
            let name = format!("{}/pdata/{}", r.expno, r.procno);
            match Recon::open(input, r) {
                Ok(x) => recons.push(x),
                Err(e) if single => return Err(e),
                Err(e) => skipped.push(format!("{name}: {e}")),
            }
        }
        if recons.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("no reconstruction could be read: {}", skipped.join("; ")),
            ));
        }
        let images = (0..recons.len()).filter(|&i| !recons[i].spectrum).collect();
        let traces = (0..recons.len()).filter(|&i| recons[i].spectrum).collect();
        Ok(ParavisionDataset {
            path,
            recons,
            images,
            traces,
            skipped,
        })
    }

    fn recon(&self, image: u32) -> Result<&Recon> {
        self.images
            .get(image as usize)
            .map(|&i| &self.recons[i])
            .ok_or_else(|| {
                Error::Usage(format!(
                    "image index {image} out of range ({} images)",
                    self.images.len()
                ))
            })
    }
}

impl Dataset for ParavisionDataset {
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self
            .images
            .iter()
            .enumerate()
            .map(|(i, &r)| self.recons[r].image_info(i as u32))
            .collect();
        let traces: Vec<TraceInfo> = self
            .traces
            .iter()
            .enumerate()
            .map(|(i, &r)| {
                let rc = &self.recons[r];
                let n = u64::from(rc.core[0]);
                let mut extra = BTreeMap::new();
                rc.extra(&mut extra);
                extra.insert("kind".into(), json!("spectrum"));
                let ext = rc.visu.number("VisuCoreExtent").unwrap_or(0.0);
                let centre = rc
                    .method
                    .as_ref()
                    .and_then(|m| m.number("PVM_FrqWorkPpm"));
                if let Some(c) = centre.filter(|_| ext > 0.0 && n > 1) {
                    let step = -ext / n as f64;
                    let first = c + ext / 2.0;
                    extra.insert(
                        "axis".into(),
                        json!({"quantity": "chemical_shift", "unit": "ppm", "first": num(first), "step": num(step), "last": num(first + step * (n as f64 - 1.0)), "size": n}),
                    );
                }
                TraceInfo {
                    index: i as u32,
                    name: Some(rc.name()),
                    sample_rate_hz: 0.0,
                    sample_count: n,
                    sweep_count: rc.frames,
                    channels: vec![SignalChannelInfo {
                        index: 0,
                        name: rc.visu.text("VisuCoreFrameType").unwrap_or_else(|| "magnitude".into()).to_ascii_lowercase(),
                        unit: None,
                        dtype: rc.stored.numpy_dtype().into(),
                        scale: rc.slope_offset(0).0,
                        offset: rc.slope_offset(0).1,
                        extra: BTreeMap::new(),
                    }],
                    start_s: None,
                    extra,
                }
            })
            .collect();
        let mut notes: Vec<String> = self
            .recons
            .iter()
            .flat_map(|r| r.notes.iter().cloned())
            .collect();
        notes.extend(self.skipped.iter().map(|s| format!("not read: {s}")));
        if self.recons.iter().any(Recon::scaled) {
            notes.push(
                "2dseq values have VisuCoreDataSlope and VisuCoreDataOffs applied (float32)".into(),
            );
        }
        if !traces.is_empty() {
            notes.push("spectroscopy reconstructions are traces; the ppm axis is centred on the method's working frequency (PVM_FrqWorkPpm)".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.recons.iter().map(|r| r.file_len).sum(),
            format: ParavisionReader.descriptor(),
            format_version: self.recons[0]
                .visu
                .text("VisuCreatorVersion")
                .map(|v| format!("ParaVision {v}")),
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let withheld: Vec<&str> = PERSONAL.iter().map(|(k, _)| *k).collect();
        let list: Vec<Value> = self
            .recons
            .iter()
            .map(|r| {
                json!({
                    "expno": r.reco.expno,
                    "procno": r.reco.procno,
                    "visu_pars": r.visu.to_json(&withheld),
                    "method": r.method.as_ref().map(|m| m.to_json(&withheld)),
                    "acqp": r.acqp.as_ref().map(|m| m.to_json(&withheld)),
                })
            })
            .collect();
        Ok(json!({ "reconstructions": list }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].size_z",
            "images[].size_t",
            "images[].size_c",
            "images[].pixel_type",
            "images[].physical_size",
            "images[].extra.rescale",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p.insert("traces[].extra.axis".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .recons
            .iter()
            .enumerate()
            .map(|(i, r)| LsEntry {
                kind: if r.spectrum { "trace".into() } else { "image".into() },
                name: format!("{}/pdata/{}/2dseq", r.reco.expno, r.reco.procno),
                offset: Some(0),
                size: Some(r.file_len),
                image: self.images.iter().position(|&x| x == i).map(|v| v as u32),
                details: json!({"frames": r.frames, "core": r.core, "word_type": r.visu.text("VisuCoreWordType")}),
            })
            .collect())
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        let r = self.recon(image)?;
        let (nz, nt, nc) = r.sizes;
        if idx.z >= nz || idx.t >= nt || idx.c >= nc {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<{nc}, z<{nz}, t<{nt})",
                idx.c, idx.z, idx.t
            )));
        }
        let x = r.core[0];
        let y = r.core.get(1).copied().unwrap_or(1);
        let plane_px = u64::from(x) * u64::from(y);
        openreadout_core::limits::plane_len(
            FORMAT_ID,
            u64::from(x),
            u64::from(y),
            1,
            r.bytes as u64,
            r.file_len,
        )?;
        let frame = r.frame_of(idx.z, idx.t, idx.c);
        let within = if r.core.len() == 3 {
            u64::from(idx.z)
        } else {
            0
        };
        let off = (frame * r.frame_px() + within * plane_px)
            .checked_mul(r.bytes as u64)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "offset overflows"))?;
        let raw = r.read_samples(off, plane_px)?;
        Ok(Plane {
            width: x,
            height: y,
            pixel_type: r.out_type(),
            samples_per_pixel: 1,
            data: r.scale(&raw, frame),
        })
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let &ri = self.traces.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace index {index} out of range ({} traces)",
                self.traces.len()
            ))
        })?;
        let r = &self.recons[ri];
        if sweep >= r.frames {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} frames)",
                r.frames
            )));
        }
        let n = u64::from(r.core[0]);
        let first = first_sample.min(n);
        let take = max_samples.min(n - first);
        let off = (u64::from(sweep) * n + first) * r.bytes as u64;
        let raw = r.read_samples(off, take)?;
        let (s, o) = r.slope_offset(u64::from(sweep));
        let vals = r.stored.samples_f64(&raw).map(|v| v * s + o).collect();
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels: vec![vals],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        rep.performed("visu_pars parses; VisuCoreSize, VisuCoreWordType and the frame groups agree with VisuCoreFrameCount");
        rep.performed("each 2dseq holds VisuCoreFrameCount frames of VisuCoreSize samples");
        for s in &self.skipped {
            rep.push(Finding::error("unreadable_reconstruction", s.clone()));
        }
        for r in &self.recons {
            let need = r.frame_px() * u64::from(r.frames) * r.bytes as u64;
            let name = format!("{}/pdata/{}/2dseq", r.reco.expno, r.reco.procno);
            if need > r.file_len {
                rep.push(Finding::error(
                    "truncated",
                    format!(
                        "{name} holds {} bytes; {} frames need {need}",
                        r.file_len, r.frames
                    ),
                ));
            } else if need < r.file_len {
                rep.push(Finding::warning(
                    "trailing_bytes",
                    format!(
                        "{name} holds {} bytes more than {} frames need",
                        r.file_len - need,
                        r.frames
                    ),
                ));
            }
            for n in &r.notes {
                rep.push(Finding::warning("frame_groups", format!("{name}: {n}")));
            }
            if r.visu
                .get("VisuCoreTransposition")
                .is_some_and(|p| p.numbers().iter().any(|v| *v != 0.0))
            {
                rep.push(Finding::info(
                    "transposed_frames",
                    format!("{name}: some frames are stored transposed (VisuCoreTransposition); they are returned as stored"),
                ));
            }
        }
        Ok(rep)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }
}

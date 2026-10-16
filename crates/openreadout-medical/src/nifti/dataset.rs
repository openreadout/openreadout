//! `Dataset` for NIfTI and Analyze files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{ByteSource, Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use super::header::{self, Header, Kind};
use super::{FORMAT_ID, NiftiReader, pair_of};
use crate::util::{lps_letter, num, plane_of_normal};

/// Largest header plus extensions read (extensions can hold whole XML documents).
const MAX_HEADER: u64 = 64 << 20;

/// A file opened as bytes: plain, or a gzip file decompressed through restart points.
struct Opened {
    file: SourceFile,
    len: u64,
    gzip: Option<(u64, Option<String>)>,
}

fn open_bytes(input: &Input, path: &Path) -> Result<Opened> {
    let src = input.fs().source(path).map_err(|e| Error::io(path, e))?;
    let mut head = [0u8; 3];
    let raw = SourceFile::new(src.clone());
    let n = raw.read_at(0, &mut head).map_err(|e| Error::io(path, e))?;
    if n == 3 && openreadout_core::gzip::is_gzip(&head) {
        let name = path.file_name().map_or_else(
            || "data.nii".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let inner = openreadout_core::gzip::strip_gz(&name).to_string();
        let gz = openreadout_core::gzip::GzipSource::open(src, inner, FORMAT_ID)?;
        let s = gz.summary().clone();
        let arc: Arc<dyn ByteSource> = Arc::new(gz);
        return Ok(Opened {
            file: SourceFile::new(arc),
            len: s.decompressed_len,
            gzip: Some((s.compressed_len, s.problem)),
        });
    }
    let len = raw.size().map_err(|e| Error::io(path, e))?;
    Ok(Opened {
        file: raw,
        len,
        gzip: None,
    })
}

fn read(o: &Opened, path: &Path, offset: u64, n: u64) -> Result<Vec<u8>> {
    let end = offset
        .checked_add(n)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "offset overflows"))?;
    if end > o.len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            offset,
            format!(
                "{} needs bytes {offset}..{end} but holds {} (truncated?)",
                path.display(),
                o.len
            ),
        ));
    }
    let mut buf =
        vec![0u8; usize::try_from(n).map_err(|_| Error::corrupt(FORMAT_ID, "read too large"))?];
    o.file
        .read_exact_at(offset, &mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// One header extension (NIfTI-1 §extensions): its code and size.
#[derive(Debug, Clone)]
struct Extension {
    code: i32,
    offset: u64,
    size: u64,
}

fn extension_name(code: i32) -> &'static str {
    match code {
        0 => "ignore",
        2 => "DICOM",
        4 => "AFNI",
        6 => "comment",
        8 => "XCEDE",
        10 => "jimdiminfo",
        12 => "workflow_fwds",
        14 => "FreeSurfer",
        16 => "pypickle",
        18 => "MIND_ident",
        20 => "b_value",
        22 => "spherical_direction",
        24 => "DT_component",
        26 => "SHC_degreeorder",
        28 => "voxbo",
        30 => "Caret",
        32 => "CIfTI",
        34 => "variable_frame_timing",
        38 => "eval",
        40 => "MATLAB",
        42 => "Quantiphyse",
        44 => "MRS",
        _ => "unknown",
    }
}

/// An opened NIfTI or Analyze file (or pair).
pub(crate) struct NiftiDataset {
    path: PathBuf,
    header_path: PathBuf,
    data_path: PathBuf,
    hdr: Header,
    head_file: Opened,
    data_file: Option<Opened>,
    extensions: Vec<Extension>,
    /// (sample bytes, stored pixel type, samples per pixel)
    stored: (usize, PixelType, u32),
    dims: (u32, u32, u32, u32, u32),
    folded: bool,
}

impl std::fmt::Debug for NiftiDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NiftiDataset")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn dim_u32(v: i64, name: &str) -> Result<u32> {
    u32::try_from(v.max(1))
        .map_err(|_| Error::corrupt(FORMAT_ID, format!("{name} = {v} is too large")))
}

impl NiftiDataset {
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    pub(crate) fn open(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let lower = path.to_string_lossy().to_ascii_lowercase();
        let is_pair_member = [".hdr", ".img", ".hdr.gz", ".img.gz"]
            .iter()
            .any(|s| lower.ends_with(s));
        let (header_path, pair_data) = if is_pair_member {
            match pair_of(input.fs(), &path) {
                Some((h, i)) => (h, Some(i)),
                None if lower.ends_with(".img") || lower.ends_with(".img.gz") => {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("{} has no .hdr header beside it", path.display()),
                    ));
                }
                None => (path.clone(), None),
            }
        } else {
            (path.clone(), None)
        };
        let head_file = open_bytes(input, &header_path)?;
        let first = read(&head_file, &header_path, 0, head_file.len.min(540))?;
        let hdr = header::parse(&first).ok_or_else(|| {
            Error::corrupt_at(
                FORMAT_ID,
                0,
                "the header is neither NIfTI-1, NIfTI-2 nor Analyze 7.5 (sizeof_hdr is not 348 or 540)",
            )
        })?;
        let (_, bytes, ty, spp) = header::datatype(hdr.datatype).ok_or_else(|| {
            match header::unsupported_datatype(hdr.datatype) {
                Some(name) => Error::unsupported(
                    FORMAT_ID,
                    format!("datatype {} ({name})", hdr.datatype),
                    "This sample type is not decoded; convert the image to a standard type (for example with nibabel or fslmaths).",
                ),
                None => Error::corrupt_at(
                    FORMAT_ID,
                    70,
                    format!("datatype {} is not a NIfTI datatype code", hdr.datatype),
                ),
            }
        })?;
        let n = hdr.dim[0];
        if !(1..=7).contains(&n) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                40,
                format!("dim[0] = {n}: the number of dimensions must be 1 to 7"),
            ));
        }
        let d = |i: usize| -> i64 {
            if (i as i64) <= n {
                hdr.dim[i].max(1)
            } else {
                1
            }
        };
        for i in 1..=n as usize {
            if hdr.dim[i] < 0 {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    40,
                    format!("dim[{i}] = {} is negative", hdr.dim[i]),
                ));
            }
        }
        let extra_c = d(5)
            .checked_mul(d(6))
            .and_then(|v| v.checked_mul(d(7)))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "dimension product overflows"))?;
        let dims = (
            dim_u32(d(1), "dim[1]")?,
            dim_u32(d(2), "dim[2]")?,
            dim_u32(d(3), "dim[3]")?,
            dim_u32(d(4), "dim[4]")?,
            dim_u32(extra_c, "dim[5..7]")?,
        );
        let single = hdr.single;
        let data_file = match (&pair_data, single) {
            (Some(p), false) => Some(open_bytes(input, p)?),
            _ => None,
        };
        // extensions: after the header when the 4-byte extension flag is set
        let mut extensions = Vec::new();
        let hl = hdr.header_len as u64;
        let ext_end = if single {
            (hdr.vox_offset.max(0.0) as u64).min(head_file.len)
        } else {
            head_file.len
        };
        if hdr.kind != Kind::Analyze && head_file.len >= hl + 4 {
            let flag = read(&head_file, &header_path, hl, 4)?;
            if flag[0] != 0 {
                let mut pos = hl + 4;
                while pos + 8 <= ext_end.min(MAX_HEADER) {
                    let h = read(&head_file, &header_path, pos, 8)?;
                    let g = |b: [u8; 4]| {
                        if hdr.little {
                            i32::from_le_bytes(b)
                        } else {
                            i32::from_be_bytes(b)
                        }
                    };
                    let esize = g([h[0], h[1], h[2], h[3]]);
                    let ecode = g([h[4], h[5], h[6], h[7]]);
                    if esize < 16 || esize % 16 != 0 || pos + esize as u64 > ext_end {
                        break;
                    }
                    extensions.push(Extension {
                        code: ecode,
                        offset: pos + 8,
                        size: esize as u64 - 8,
                    });
                    pos += esize as u64;
                }
            }
        }
        Ok(NiftiDataset {
            path,
            header_path,
            data_path: pair_data.unwrap_or_default(),
            folded: d(6) > 1 || d(7) > 1,
            hdr,
            head_file,
            data_file,
            extensions,
            stored: (bytes, ty, spp),
            dims,
        })
    }

    fn data(&self) -> (&Opened, &Path) {
        match &self.data_file {
            Some(f) => (f, self.data_path.as_path()),
            None => (&self.head_file, self.header_path.as_path()),
        }
    }

    /// stored × slope + intercept: the slope when it applies.
    #[allow(clippy::float_cmp)]
    fn scaling(&self) -> Option<(f64, f64)> {
        let (_, ty, spp) = self.stored;
        if spp > 1 || ty.is_complex() {
            return None;
        }
        let h = &self.hdr;
        if h.kind == Kind::Analyze {
            let s = h.spm_scale;
            return (s.is_finite() && s > 0.0 && s != 1.0).then_some((s, 0.0));
        }
        let (s, i) = (
            h.scl_slope,
            if h.scl_inter.is_finite() {
                h.scl_inter
            } else {
                0.0
            },
        );
        (s.is_finite() && s != 0.0 && (s != 1.0 || i != 0.0)).then_some((s, i))
    }

    fn out_type(&self) -> PixelType {
        if self.scaling().is_some() {
            if self.stored.1 == PixelType::Double {
                PixelType::Double
            } else {
                PixelType::Float
            }
        } else {
            self.stored.1
        }
    }

    fn plane_bytes(&self) -> u64 {
        u64::from(self.dims.0) * u64::from(self.dims.1) * self.stored.0 as u64
    }

    fn data_offset(&self) -> u64 {
        let v = self.hdr.vox_offset;
        if v.is_finite() && v > 0.0 {
            v as u64
        } else {
            0
        }
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self) -> ImageInfo {
        let h = &self.hdr;
        let (nx, ny, nz, nt, nc) = self.dims;
        let (_, _, spp) = self.stored;
        let mut im = ImageInfo::new(0, nx, ny, self.out_type());
        im.size_z = nz;
        im.size_t = nt;
        im.size_c = nc;
        im.samples_per_pixel = spp;
        let (um, sec, _) = h.units();
        let px = |i: usize| (h.pixdim[i].abs() > 0.0).then(|| h.pixdim[i].abs() * um);
        im.physical_size =
            PhysicalSize::micrometres(px(1), px(2), (nz > 1).then(|| px(3)).flatten());
        if nt > 1 {
            im.time_increment_s = sec
                .or(if h.kind == Kind::Analyze {
                    None
                } else {
                    Some(1.0)
                })
                .and_then(|s| (h.pixdim[4].abs() > 0.0).then(|| h.pixdim[4].abs() * s));
        }
        im.channels = (0..nc)
            .map(|c| ChannelInfo {
                index: c,
                ..ChannelInfo::default()
            })
            .collect();
        if !h.descrip.is_empty() {
            im.name = Some(h.descrip.clone());
        }
        let ex = &mut im.extra;
        let (name, _, _, _) = header::datatype(h.datatype).unwrap_or(("?", 0, PixelType::Uint8, 1));
        ex.insert("datatype".into(), Value::from(name));
        ex.insert(
            "byte_order".into(),
            Value::from(if h.little {
                "little-endian"
            } else {
                "big-endian"
            }),
        );
        let dims: Vec<Value> = (1..=h.dim[0].clamp(1, 7) as usize)
            .map(|i| Value::from(h.dim[i]))
            .collect();
        ex.insert("dims".into(), Value::Array(dims));
        if let Some((s, i)) = self.scaling() {
            ex.insert(
                "rescale".into(),
                json!({"slope": num(s), "intercept": num(i), "applied": true}),
            );
            ex.insert(
                "stored_pixel_type".into(),
                Value::from(self.stored.1.ome_name()),
            );
        }
        if h.cal_max > h.cal_min && h.cal_max.is_finite() && h.cal_min.is_finite() {
            ex.insert(
                "windows".into(),
                json!([{"center": num(f64::midpoint(h.cal_min, h.cal_max)), "width": num(h.cal_max - h.cal_min), "name": "cal_min/cal_max"}]),
            );
        }
        if h.kind == Kind::Analyze {
            ex.insert("analyze_orient".into(), Value::from(h.orient));
            if h.analyze_patient_id {
                ex.insert(
                    "personal_data".into(),
                    json!([{"field": "patient_id (Analyze header byte 283)", "kind": "patient_id"}]),
                );
            }
        } else {
            let (m, from) = h.affine();
            ex.insert(
                "affine".into(),
                Value::Array(
                    m.iter()
                        .map(|r| Value::Array(r.iter().map(|v| num(*v)).collect()))
                        .chain(std::iter::once(json!([0, 0, 0, 1])))
                        .collect(),
                ),
            );
            ex.insert("affine_from".into(), Value::from(from));
            ex.insert("qform_code".into(), Value::from(xform_name(h.qform_code)));
            ex.insert("sform_code".into(), Value::from(xform_name(h.sform_code)));
            // directions of i, j, k in RAS; our orientation vocabulary uses LPS letters
            let col = |j: usize| {
                let v = [m[0][j], m[1][j], m[2][j]];
                let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if n > 0.0 {
                    [-v[0] / n, -v[1] / n, v[2] / n]
                } else {
                    [0.0, 0.0, 0.0]
                }
            };
            let (i, j, k) = (col(0), col(1), col(2));
            let mut o = json!({
                "plane": plane_of_normal(k),
                "columns_toward": lps_letter(i).to_string(),
                "rows_toward": lps_letter(j).to_string(),
            });
            if nz > 1 {
                o["slices_toward"] = Value::from(lps_letter(k).to_string());
            }
            ex.insert("orientation".into(), o);
            if h.intent_code != 0 {
                ex.insert(
                    "intent".into(),
                    json!({"code": h.intent_code, "name": intent_name(h.intent_code), "label": h.intent_name, "parameters": h.intent_p.map(num)}),
                );
            }
            if h.slice_code != 0 || h.slice_duration > 0.0 {
                ex.insert(
                    "slice_timing".into(),
                    json!({"code": h.slice_code, "order": slice_order(h.slice_code), "duration_s": num(h.slice_duration * sec.unwrap_or(1.0)), "start": h.slice_start, "end": h.slice_end}),
                );
            }
            if h.dim_info != 0 {
                ex.insert(
                    "dim_info".into(),
                    json!({"frequency": h.dim_info & 3, "phase": (h.dim_info >> 2) & 3, "slice": (h.dim_info >> 4) & 3}),
                );
            }
            if h.toffset != 0.0 {
                ex.insert("toffset".into(), num(h.toffset));
            }
        }
        if !h.aux_file.is_empty() {
            ex.insert("aux_file".into(), Value::from(h.aux_file.clone()));
        }
        if !self.extensions.is_empty() {
            ex.insert(
                "extensions".into(),
                Value::Array(
                    self.extensions
                        .iter()
                        .map(|e| json!({"code": e.code, "name": extension_name(e.code), "bytes": e.size}))
                        .collect(),
                ),
            );
        }
        im.finish()
    }
}

fn xform_name(code: i32) -> &'static str {
    match code {
        0 => "unknown",
        1 => "scanner_anat",
        2 => "aligned_anat",
        3 => "talairach",
        4 => "mni_152",
        5 => "template_other",
        _ => "other",
    }
}

fn slice_order(code: i32) -> &'static str {
    match code {
        1 => "sequential increasing",
        2 => "sequential decreasing",
        3 => "alternating increasing",
        4 => "alternating decreasing",
        5 => "alternating increasing 2",
        6 => "alternating decreasing 2",
        _ => "unknown",
    }
}

fn intent_name(code: i32) -> &'static str {
    match code {
        2 => "correlation",
        3 => "t-test",
        4 => "F-test",
        5 => "z-score",
        6 => "chi-squared",
        1001 => "estimate",
        1002 => "label",
        1003 => "neuroname",
        1004 => "general matrix",
        1005 => "symmetric matrix",
        1006 => "displacement vector",
        1007 => "vector",
        1008 => "pointset",
        1009 => "triangle",
        1010 => "quaternion",
        1011 => "dimensionless",
        2001 => "time series",
        2002 => "node index",
        2003 => "RGB vector",
        2004 => "RGBA vector",
        2005 => "shape",
        _ => "other",
    }
}

impl Dataset for NiftiDataset {
    fn info(&self) -> Result<FileInfo> {
        let image = self.image_info();
        let h = &self.hdr;
        let mut notes = Vec::new();
        for (what, o) in [
            ("header", Some(&self.head_file)),
            ("image", self.data_file.as_ref()),
        ] {
            if let Some((c, problem)) = o.and_then(|o| o.gzip.as_ref()) {
                notes.push(format!(
                    "gzip-compressed {what}: {c} bytes decompress to {}; decompressed once at open",
                    o.map_or(0, |o| o.len)
                ));
                if let Some(p) = problem {
                    notes.push(format!("gzip: {p}"));
                }
            }
        }
        if self.scaling().is_some() {
            notes.push(if h.kind == Kind::Analyze {
                "values are scaled by the SPM scale factor (funused1) and returned as float32".into()
            } else {
                "values have scl_slope and scl_inter applied and are returned as float32 (float64 data stays float64)".into()
            });
        }
        if self.folded {
            notes.push("dimensions 5 to 7 are folded into C (channels)".into());
        }
        if h.kind != Kind::Analyze && h.units().2 {
            notes.push("xyzt_units gives no spatial unit; millimetres assumed".into());
        }
        if h.kind == Kind::Analyze {
            notes.push("Analyze 7.5 header: voxel sizes are taken as millimetres and no orientation is recorded".into());
        }
        let (f, _) = self.data();
        let need = self
            .plane_bytes()
            .saturating_mul(
                u64::from(image.size_z) * u64::from(image.size_t) * u64::from(image.size_c),
            )
            .saturating_add(self.data_offset());
        if need > f.len {
            notes.push(format!(
                "the image data is {} bytes but the header describes {need}; it appears truncated (run `check`)",
                f.len
            ));
        }
        let size = self
            .head_file
            .gzip
            .as_ref()
            .map_or(self.head_file.len, |g| g.0)
            + self
                .data_file
                .as_ref()
                .map_or(0, |d| d.gzip.as_ref().map_or(d.len, |g| g.0));
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: size,
            format: NiftiReader.descriptor(),
            format_version: Some(
                match h.kind {
                    Kind::Nifti1 => "NIfTI-1",
                    Kind::Nifti2 => "NIfTI-2",
                    Kind::Analyze => "Analyze 7.5",
                }
                .into(),
            ),
            plane_count: image.plane_count,
            images: vec![image],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.hdr;
        let mut m = Map::new();
        m.insert("sizeof_hdr".into(), Value::from(h.header_len));
        m.insert("magic".into(), Value::from(h.magic.clone()));
        m.insert("dim".into(), json!(h.dim));
        m.insert("datatype".into(), Value::from(h.datatype));
        m.insert("bitpix".into(), Value::from(h.bitpix));
        m.insert(
            "pixdim".into(),
            Value::Array(h.pixdim.iter().map(|v| num(*v)).collect()),
        );
        m.insert("vox_offset".into(), num(h.vox_offset));
        m.insert("cal_max".into(), num(h.cal_max));
        m.insert("cal_min".into(), num(h.cal_min));
        m.insert("descrip".into(), Value::from(h.descrip.clone()));
        m.insert("aux_file".into(), Value::from(h.aux_file.clone()));
        if h.kind == Kind::Analyze {
            m.insert("funused1".into(), num(h.spm_scale));
            m.insert("orient".into(), Value::from(h.orient));
            m.insert(
                "patient_id".into(),
                Value::from(if h.analyze_patient_id { "withheld" } else { "" }),
            );
        } else {
            m.insert("scl_slope".into(), num(h.scl_slope));
            m.insert("scl_inter".into(), num(h.scl_inter));
            m.insert("xyzt_units".into(), Value::from(h.xyzt_units));
            m.insert("intent_code".into(), Value::from(h.intent_code));
            m.insert(
                "intent_p".into(),
                Value::Array(h.intent_p.iter().map(|v| num(*v)).collect()),
            );
            m.insert("intent_name".into(), Value::from(h.intent_name.clone()));
            m.insert("slice_start".into(), Value::from(h.slice_start));
            m.insert("slice_end".into(), Value::from(h.slice_end));
            m.insert("slice_code".into(), Value::from(h.slice_code));
            m.insert("slice_duration".into(), num(h.slice_duration));
            m.insert("toffset".into(), num(h.toffset));
            m.insert("dim_info".into(), Value::from(h.dim_info));
            m.insert("qform_code".into(), Value::from(h.qform_code));
            m.insert("sform_code".into(), Value::from(h.sform_code));
            m.insert(
                "quatern".into(),
                Value::Array(h.quatern.iter().map(|v| num(*v)).collect()),
            );
            m.insert(
                "srow".into(),
                Value::Array(
                    h.srow
                        .iter()
                        .map(|r| Value::Array(r.iter().map(|v| num(*v)).collect()))
                        .collect(),
                ),
            );
        }
        let mut exts = Vec::new();
        for e in &self.extensions {
            let mut o = json!({"code": e.code, "name": extension_name(e.code), "bytes": e.size});
            // comments and AFNI/XCEDE XML are text; DICOM extensions may hold personal data
            if matches!(e.code, 4 | 6 | 8)
                && e.size <= 1 << 20
                && let Ok(b) = read(&self.head_file, &self.header_path, e.offset, e.size)
            {
                let t = String::from_utf8_lossy(&b)
                    .trim_end_matches('\0')
                    .to_string();
                o["text"] = Value::from(t);
            }
            exts.push(o);
        }
        if !exts.is_empty() {
            m.insert("extensions".into(), Value::Array(exts));
        }
        Ok(Value::Object(m))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "images[].size_x",
            "images[].size_y",
            "images[].size_z",
            "images[].size_t",
            "images[].size_c",
            "images[].pixel_type",
            "images[].physical_size",
            "images[].time_increment_s",
            "images[].extra.affine",
            "images[].extra.rescale",
            "images[].extra.orientation",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: "header".into(),
            offset: Some(0),
            size: Some(self.hdr.header_len as u64),
            image: None,
            details: json!({"version": format!("{:?}", self.hdr.kind), "byte_order": if self.hdr.little {"little-endian"} else {"big-endian"}}),
        }];
        for e in &self.extensions {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: format!("extension {} ({})", e.code, extension_name(e.code)),
                offset: Some(e.offset),
                size: Some(e.size),
                image: None,
                details: Value::Null,
            });
        }
        let (nx, ny, nz, nt, nc) = self.dims;
        let total = u64::from(nz) * u64::from(nt) * u64::from(nc);
        out.push(LsEntry {
            kind: "image".into(),
            name: if self.data_file.is_some() {
                self.data_path
                    .file_name()
                    .map_or("data".into(), |n| n.to_string_lossy().to_string())
            } else {
                "data".into()
            },
            offset: Some(self.data_offset()),
            size: Some(self.plane_bytes().saturating_mul(total)),
            image: Some(0),
            details: json!({"x": nx, "y": ny, "planes": total, "plane_bytes": self.plane_bytes()}),
        });
        Ok(out)
    }

    #[allow(clippy::many_single_char_names)]
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image index {image} out of range (a NIfTI file holds one image)"
            )));
        }
        let (nx, ny, nz, nt, nc) = self.dims;
        if index.c >= nc || index.z >= nz || index.t >= nt {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<{nc}, z<{nz}, t<{nt})",
                index.c, index.z, index.t
            )));
        }
        let pb = self.plane_bytes();
        openreadout_core::limits::plane_len(
            FORMAT_ID,
            u64::from(nx),
            u64::from(ny),
            1,
            self.stored.0 as u64,
            self.data().0.len,
        )?;
        let k = u64::from(index.z)
            + u64::from(nz) * (u64::from(index.t) + u64::from(nt) * u64::from(index.c));
        let off = k
            .checked_mul(pb)
            .and_then(|o| o.checked_add(self.data_offset()))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "plane offset overflows"))?;
        let (f, p) = self.data();
        let mut raw = read(f, p, off, pb)?;
        let (bytes, ty, spp) = self.stored;
        let w = if ty.is_complex() {
            bytes / 2
        } else if spp > 1 {
            1
        } else {
            bytes
        };
        if !self.hdr.little && w > 1 {
            for c in raw.chunks_exact_mut(w) {
                c.reverse();
            }
        }
        let out = self.out_type();
        let data = match self.scaling() {
            Some((s, i)) => {
                let mut v = Vec::with_capacity(raw.len() / bytes * out.bytes_per_sample());
                for x in ty.samples_f64(&raw) {
                    let y = x * s + i;
                    if out == PixelType::Double {
                        v.extend(y.to_le_bytes());
                    } else {
                        v.extend((y as f32).to_le_bytes());
                    }
                }
                v
            }
            None => raw,
        };
        Ok(Plane {
            width: nx,
            height: ny,
            pixel_type: out,
            samples_per_pixel: spp,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        let h = self.hdr.clone();
        r.performed("sizeof_hdr, magic and byte order");
        r.performed("dim, datatype and bitpix agree");
        r.performed("vox_offset and file size hold every voxel");
        r.performed("extensions are whole 16-byte blocks inside the header");
        if let Some((name, bytes, _, spp)) = header::datatype(h.datatype)
            && i64::from(h.bitpix) != (bytes as i64) * 8
        {
            let _ = spp;
            r.push(
                Finding::warning(
                    "bitpix_mismatch",
                    format!(
                        "bitpix is {} but datatype {name} has {} bits",
                        h.bitpix,
                        bytes * 8
                    ),
                )
                .at(72),
            );
        }
        if h.kind != Kind::Analyze && h.single && h.vox_offset < h.header_len as f64 + 4.0 {
            r.push(
                Finding::error(
                    "bad_vox_offset",
                    format!("vox_offset {} is inside the header", h.vox_offset),
                )
                .at(108),
            );
        }
        if h.kind == Kind::Nifti1 && h.single && !(h.vox_offset as u64).is_multiple_of(16) {
            r.push(Finding::info(
                "vox_offset_alignment",
                format!(
                    "vox_offset {} is not a multiple of 16 (NIfTI-1 asks for it)",
                    h.vox_offset
                ),
            ));
        }
        if h.qform_code == 0 && h.sform_code == 0 && h.kind != Kind::Analyze {
            r.push(Finding::info(
                "no_orientation",
                "qform_code and sform_code are 0: the image has no world orientation",
            ));
        }
        let (_, _, nz, nt, nc) = self.dims;
        let need = self
            .plane_bytes()
            .saturating_mul(u64::from(nz) * u64::from(nt) * u64::from(nc))
            .saturating_add(self.data_offset());
        let (f, _) = self.data();
        if need > f.len {
            r.push(Finding::error(
                "truncated",
                format!("the data needs {need} bytes but the file holds {}", f.len),
            ));
        } else if need < f.len {
            r.push(Finding::info(
                "trailing_bytes",
                format!("{} bytes follow the last voxel", f.len - need),
            ));
        }
        if let Some((_, Some(p))) = &self.head_file.gzip {
            r.push(Finding::error("gzip", p.clone()));
        }
        Ok(r)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        for p in [&self.header_path, &self.data_path] {
            if !p.as_os_str().is_empty() && *p != self.path {
                v.push(p.clone());
            }
        }
        v
    }
}

//! `Dataset` for MRC/CCP4 files: normalized metadata, listing, plane reads, integrity checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use super::ext::{
    FEI1_BLOCK_LEN, fei_block, fei_block_len, ints_reals_record, is_text, seri_bytes, seri_record,
    symmetry_lines,
};
use super::header::{HEADER_LEN, Layout, Mode, MrcHeader};
use super::{FORMAT_ID, MrcReader};
use crate::util::{
    Blob, Endian, byte_order_name, ext_lower, num, ole_date_to_iso, swap_samples,
    unix_micros_to_iso, widen_half,
};

/// Largest extended header read into memory.
const MAX_EXT_BYTES: u64 = 256 << 20;
/// Sections listed individually by `info --view structure`.
const MAX_LISTED_SECTIONS: i32 = 4096;

/// What the extended header holds, as far as we decode it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtKind {
    None,
    /// FEI1/FEI2 metadata blocks of `block` bytes, one per section.
    Fei {
        block: usize,
    },
    /// SerialEM section records of `bytes` bytes with item flags `flags`.
    Seri {
        bytes: usize,
        flags: i16,
    },
    /// Agard/FEI-style records of `nint` integers and `nreal` reals per section.
    IntsReals {
        nint: usize,
        nreal: usize,
    },
    /// CCP4 symmetry operators as 80-character text lines.
    Symmetry,
    /// Anything else: kept raw (extractable as an attachment).
    Raw,
}

/// Decodes one per-section extended-header record.
type RecordDecoder = Box<dyn Fn(&[u8]) -> Map<String, Value>>;

/// An opened MRC/CCP4 file.
#[derive(Debug)]
pub struct MrcDataset {
    path: PathBuf,
    /// A gzip container around the MRC data: (compressed bytes, a problem found while
    /// decompressing).
    gzip: Option<(u64, Option<String>)>,
    blob: Blob,
    header: MrcHeader,
    extension: String,
    ext: Vec<u8>,
    ext_kind: ExtKind,
    layout: Layout,
}

fn pixel_type(h: &MrcHeader) -> PixelType {
    match h.mode {
        Mode::Int8 if h.bytes_unsigned() => PixelType::Uint8,
        Mode::Int8 => PixelType::Int8,
        Mode::Int16 => PixelType::Int16,
        Mode::Uint16 => PixelType::Uint16,
        Mode::Rgb8 | Mode::Packed4Bit | Mode::Unknown(_) => PixelType::Uint8,
        Mode::Float32 | Mode::Float16 => PixelType::Float,
        // complex int16 pairs are widened exactly to complex float32
        Mode::ComplexInt16 | Mode::ComplexFloat32 => PixelType::ComplexFloat,
    }
}

fn axis_letter(a: usize) -> char {
    ['X', 'Y', 'Z'][a.min(2)]
}

fn classify_ext(h: &MrcHeader, ext: &[u8]) -> ExtKind {
    if ext.is_empty() {
        return ExtKind::None;
    }
    let e = h.endian();
    let nz = usize::try_from(h.nz.max(1)).unwrap_or(1);
    match h.exttyp.as_str() {
        "FEI1" | "FEI2" => {
            if let Some(block) = fei_block_len(ext, e) {
                return ExtKind::Fei { block };
            }
            ExtKind::Raw
        }
        "SERI" => match seri_bytes(h.nreal) {
            Some(n) if n > 0 && usize::try_from(h.nint).ok() == Some(n) => ExtKind::Seri {
                bytes: n,
                flags: h.nreal,
            },
            _ => ints_reals(h, ext, nz),
        },
        "AGAR" | "MRCO" => ints_reals(h, ext, nz),
        "CCP4" => ExtKind::Symmetry,
        _ if h.ispg > 0 && is_text(ext) => ExtKind::Symmetry,
        _ => ExtKind::Raw,
    }
}

fn ints_reals(h: &MrcHeader, ext: &[u8], nz: usize) -> ExtKind {
    match (usize::try_from(h.nint), usize::try_from(h.nreal)) {
        (Ok(nint), Ok(nreal))
            if nint + nreal > 0
                && (nint + nreal)
                    .checked_mul(4)
                    .and_then(|v| v.checked_mul(nz))
                    .is_some_and(|v| v <= ext.len()) =>
        {
            ExtKind::IntsReals { nint, nreal }
        }
        _ => ExtKind::Raw,
    }
}

fn nonempty(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn positive(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|x| x.is_finite() && *x > 0.0)
}

impl MrcDataset {
    /// Open an MRC/CCP4 file. Reads the main and extended headers only.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs): (&Path, &Fs) = (input.path(), input.fs());
        let mut blob = Blob::open(fs, path)?;
        if blob.len < HEADER_LEN as u64 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "file is {} bytes, shorter than the 1024-byte MRC header",
                    blob.len
                ),
            ));
        }
        let head = blob.read_at(FORMAT_ID, 0, HEADER_LEN as u64)?;
        let header = MrcHeader::parse(&head)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "header does not parse"))?;
        if header.nx <= 0 || header.ny <= 0 || header.nz <= 0 {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                0,
                format!(
                    "dimensions NX={} NY={} NZ={} are not all positive",
                    header.nx, header.ny, header.nz
                ),
            ));
        }
        if let Mode::Unknown(code) = header.mode {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("MODE {code}"),
                "MRC2014 defines modes 0, 1, 2, 3, 4, 6, 12 and 101 (IMOD also writes 16); this value is none of them. Run `openreadout check` to see whether the header is damaged.",
            ));
        }
        if header.nsymbt < 0 {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                92,
                format!("NSYMBT is negative ({})", header.nsymbt),
            ));
        }
        let want = u64::try_from(header.nsymbt).unwrap_or(0);
        let avail = blob.len.saturating_sub(HEADER_LEN as u64);
        let ext = blob.read_upto(HEADER_LEN as u64, want.min(avail).min(MAX_EXT_BYTES))?;
        let ext_kind = classify_ext(&header, &ext);
        let extension = ext_lower(path);
        let layout = header.layout(&extension);
        Ok(MrcDataset {
            path: path.to_path_buf(),
            gzip: None,
            blob,
            header,
            extension,
            ext,
            ext_kind,
            layout,
        })
    }

    /// Open a gzip-compressed MRC file (`emd_1234.map.gz`): decompressed once at open, then read
    /// through restart points (`openreadout_core::gzip`).
    pub(crate) fn open_gzip(input: &Input) -> Result<Self> {
        let path = input.path();
        let src = input.fs().source(path).map_err(|e| Error::io(path, e))?;
        let name = path.file_name().map_or_else(
            || "data.mrc".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let inner = openreadout_core::gzip::strip_gz(&name).to_string();
        let gz = openreadout_core::gzip::GzipSource::open(src, inner.clone(), FORMAT_ID)?;
        let summary = gz.summary().clone();
        let view = Input::from_source(&inner, std::sync::Arc::new(gz));
        let mut ds = Self::open_input(&view)?;
        ds.path = path.to_path_buf();
        ds.gzip = Some((summary.compressed_len, summary.problem.clone()));
        Ok(ds)
    }

    /// The decoded main header.
    pub fn header(&self) -> &MrcHeader {
        &self.header
    }

    fn sizes(&self) -> (u32, u32) {
        let h = &self.header;
        let nz = h.nz.max(1) as u32;
        match self.layout {
            Layout::ImageStack => (1, nz),
            Layout::Volume => (nz, 1),
            Layout::VolumeStack => {
                let mz = h.mz.max(1) as u32;
                (mz, nz / mz)
            }
        }
    }

    fn records(&self, limit: Option<usize>) -> (u64, Vec<Value>) {
        let e = self.header.endian();
        let nz = usize::try_from(self.header.nz.max(0)).unwrap_or(0);
        let (len, decode): (usize, RecordDecoder) = match self.ext_kind {
            ExtKind::Fei { block } => (block, Box::new(move |b| fei_block(b, e))),
            ExtKind::Seri { bytes, flags } => (bytes, Box::new(move |b| seri_record(b, flags, e))),
            ExtKind::IntsReals { nint, nreal } => (
                (nint + nreal) * 4,
                Box::new(move |b| ints_reals_record(b, nint, nreal, e)),
            ),
            _ => return (0, Vec::new()),
        };
        if len == 0 {
            return (0, Vec::new());
        }
        let total = nz.min(self.ext.len() / len);
        let take = limit.map_or(total, |l| l.min(total));
        let (size_z, _) = self.sizes();
        let size_z = size_z.max(1) as usize;
        let fei = matches!(self.ext_kind, ExtKind::Fei { .. });
        let recs = (0..take)
            .map(|s| {
                let mut m = Map::new();
                m.insert("section".into(), Value::from(s as u64));
                m.insert("t".into(), Value::from((s / size_z) as u64));
                m.insert("z".into(), Value::from((s % size_z) as u64));
                let fields = decode(&self.ext[s * len..(s + 1) * len]);
                if fei {
                    shared_fei_fields(&fields, &mut m);
                }
                m.extend(fields);
                Value::Object(m)
            })
            .collect();
        (total as u64, recs)
    }

    fn first_fei(&self) -> Option<Map<String, Value>> {
        match self.ext_kind {
            ExtKind::Fei { block } => Some(fei_block(&self.ext[..block], self.header.endian())),
            _ => None,
        }
    }

    fn data_len(&self) -> Option<u64> {
        self.header
            .section_bytes()?
            .checked_mul(u64::try_from(self.header.nz).ok()?)
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self) -> ImageInfo {
        let h = &self.header;
        let (size_z, size_t) = self.sizes();
        let mut im = ImageInfo::new(0, h.nx as u32, h.ny as u32, pixel_type(h));
        im.size_z = size_z;
        im.size_t = size_t;
        if h.mode == Mode::Rgb8 {
            im.samples_per_pixel = 3;
        }
        let steps = h.file_axis_steps_angstrom();
        let um = |s: Option<f64>| s.map(|a| a / 1e4);
        let z = matches!(self.layout, Layout::Volume | Layout::VolumeStack)
            .then_some(steps[2])
            .flatten();
        im.physical_size = PhysicalSize::micrometres(um(steps[0]), um(steps[1]), um(z));
        let fei = self.first_fei();
        let mut channel = ChannelInfo {
            index: 0,
            ..ChannelInfo::default()
        };
        if let Some(f) = &fei {
            // Integration time is in seconds (inferred from EPU corpus values, e.g. 10.0 and 12.2).
            channel.exposure_ms = positive(f.get("integration_time")).map(|s| s * 1000.0);
            let detector = nonempty(f.get("detector_commercial_name"))
                .or_else(|| nonempty(f.get("camera_name")))
                .or_else(|| nonempty(f.get("stem_detector_name")));
            let instrument = InstrumentInfo {
                manufacturer: Some("FEI / Thermo Fisher Scientific".into()),
                model: nonempty(f.get("microscope_type")),
                software: nonempty(f.get("application")),
                software_version: nonempty(f.get("application_version")),
                detector,
            };
            im.instrument = Some(instrument);
            im.acquired_at = f
                .get("acquisition_time_stamp")
                .and_then(Value::as_i64)
                .and_then(unix_micros_to_iso)
                .or_else(|| positive(f.get("timestamp")).and_then(ole_date_to_iso));
            if let Some(ht) = positive(f.get("high_tension")) {
                im.extra.insert("high_tension_kv".into(), num(ht / 1000.0));
            }
            if let Some(m) = positive(f.get("magnification")) {
                im.extra.insert("magnification".into(), num(m));
            }
            im.extra
                .insert("fei_metadata".into(), Value::Object(f.clone()));
        }
        im.channels = vec![channel];
        let map = h.axis_map();
        let axis_order: String = map
            .unwrap_or([0, 1, 2])
            .iter()
            .map(|a| axis_letter(*a))
            .collect();
        let ex = &mut im.extra;
        ex.insert("mode".into(), Value::from(h.mode.code()));
        ex.insert("mode_name".into(), Value::from(h.mode.label()));
        ex.insert("layout".into(), Value::from(self.layout.label()));
        ex.insert(
            "byte_order".into(),
            Value::from(byte_order_name(h.endian())),
        );
        ex.insert("axis_order".into(), Value::from(axis_order));
        ex.insert(
            "pixel_size_angstrom".into(),
            json!({"x": steps[0].map(num), "y": steps[1].map(num), "z": steps[2].map(num)}),
        );
        ex.insert(
            "cell_angstrom".into(),
            json!(h.cell_a.map(|v| num(f64::from(v)))),
        );
        ex.insert(
            "cell_angles_deg".into(),
            json!(h.cell_b.map(|v| num(f64::from(v)))),
        );
        ex.insert("grid_sampling".into(), json!([h.mx, h.my, h.mz]));
        ex.insert(
            "start_index".into(),
            json!([h.nxstart, h.nystart, h.nzstart]),
        );
        ex.insert(
            "origin_angstrom".into(),
            json!(h.origin.map(|v| num(f64::from(v)))),
        );
        ex.insert("space_group".into(), Value::from(h.ispg));
        let (r, m, s) = h.stats_undetermined();
        ex.insert(
            "density".into(),
            json!({
                "min": num(f64::from(h.dmin)), "max": num(f64::from(h.dmax)),
                "mean": num(f64::from(h.dmean)), "rms": num(f64::from(h.rms)),
                "range_determined": !r, "mean_determined": !m, "rms_determined": !s,
            }),
        );
        let labels: Vec<&String> = h.labels.iter().filter(|l| !l.is_empty()).collect();
        if !labels.is_empty() {
            ex.insert("labels".into(), json!(labels));
        }
        ex.insert("mrc_version".into(), Value::from(h.nversion));
        if h.nsymbt > 0 {
            let (records, _) = self.records(Some(0));
            ex.insert(
                "extended_header".into(),
                json!({"type": h.exttyp, "bytes": h.nsymbt, "decoded_as": self.ext_kind_name(), "records": records}),
            );
        }
        if h.imod_stamp {
            ex.insert("imod_flags".into(), Value::from(h.imod_flags));
        }
        if h.mode == Mode::Int8 {
            ex.insert("bytes_signed".into(), Value::Bool(!h.bytes_unsigned()));
        }
        if matches!(h.mode, Mode::ComplexInt16 | Mode::ComplexFloat32) {
            ex.insert("complex".into(), Value::Bool(true));
        }
        im.finish()
    }

    fn ext_kind_name(&self) -> &'static str {
        match self.ext_kind {
            ExtKind::None => "none",
            ExtKind::Fei { block } if block >= super::ext::FEI2_BLOCK_LEN => "fei2",
            ExtKind::Fei { .. } => "fei1",
            ExtKind::Seri { .. } => "serialem",
            ExtKind::IntsReals { .. } => "integers-and-reals",
            ExtKind::Symmetry => "symmetry",
            ExtKind::Raw => "raw",
        }
    }

    fn section_of(&self, idx: PlaneIndex) -> Result<u64> {
        let (size_z, size_t) = self.sizes();
        if idx.c > 0 || idx.z >= size_z || idx.t >= size_t {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<1, z<{size_z}, t<{size_t})",
                idx.c, idx.z, idx.t
            )));
        }
        Ok(u64::from(idx.t) * u64::from(size_z) + u64::from(idx.z))
    }

    fn decode(&self, raw: Vec<u8>) -> Vec<u8> {
        let h = &self.header;
        let big = h.endian() == Endian::Big;
        match h.mode {
            Mode::Int16 | Mode::Uint16 | Mode::Float32 => {
                let mut d = raw;
                if big {
                    swap_samples(&mut d, h.mode.stored_bytes().unwrap_or(1) as usize);
                }
                d
            }
            Mode::Float16 => {
                let mut d = raw;
                if big {
                    swap_samples(&mut d, 2);
                }
                widen_half(&d)
            }
            Mode::Packed4Bit => unpack_4bit(&raw, h.nx as usize, h.ny as usize),
            Mode::ComplexFloat32 => {
                let mut d = raw;
                if big {
                    swap_samples(&mut d, 4);
                }
                d
            }
            Mode::ComplexInt16 => {
                let mut d = raw;
                if big {
                    swap_samples(&mut d, 2);
                }
                d.as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|c| f32::from(i16::from_le_bytes(*c)).to_le_bytes())
                    .collect()
            }
            _ => raw,
        }
    }
}

/// The shared per-frame vocabulary (book/src/guides/metadata.md) from one FEI block: stage
/// position in µm (stored in metres), exposure in ms (stored in seconds), absolute time.
fn shared_fei_fields(f: &Map<String, Value>, m: &mut Map<String, Value>) {
    for (ours, key) in [
        ("stage_x_um", "stage_x"),
        ("stage_y_um", "stage_y"),
        ("stage_z_um", "stage_z"),
    ] {
        if let Some(v) = f.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()) {
            m.insert(ours.into(), num(v * 1e6));
        }
    }
    if let Some(s) = positive(f.get("integration_time")) {
        m.insert("exposure_ms".into(), num(s * 1000.0));
    }
    let at = f
        .get("acquisition_time_stamp")
        .and_then(Value::as_i64)
        .and_then(unix_micros_to_iso)
        .or_else(|| positive(f.get("timestamp")).and_then(ole_date_to_iso));
    if let Some(at) = at {
        m.insert("acquired_at".into(), Value::from(at));
    }
}

/// Two 4-bit values per byte, lower coordinate in the low nibble; rows padded to whole bytes.
pub(crate) fn unpack_4bit(raw: &[u8], nx: usize, ny: usize) -> Vec<u8> {
    let row = nx.div_ceil(2);
    let mut out = Vec::with_capacity(nx.saturating_mul(ny).min(raw.len().saturating_mul(2)));
    for y in 0..ny {
        let r = raw.get(y * row..(y + 1) * row).unwrap_or(&[]);
        for x in 0..nx {
            let b = r.get(x / 2).copied().unwrap_or(0);
            out.push(if x % 2 == 0 { b & 0x0f } else { b >> 4 });
        }
    }
    out
}

impl Dataset for MrcDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.header;
        let image = self.image_info();
        let mut notes = Vec::new();
        if let Some((compressed, problem)) = &self.gzip {
            notes.push(format!(
                "gzip-compressed: {compressed} bytes decompress to {}; decompressed once at open (offsets in this output refer to the decompressed file)",
                self.blob.len
            ));
            if let Some(p) = problem {
                notes.push(format!("gzip: {p}"));
            }
        }
        let need = self
            .data_len()
            .and_then(|d| d.checked_add(h.data_offset()))
            .unwrap_or(u64::MAX);
        if need > self.blob.len {
            notes.push(format!(
                "file is {} bytes but the header describes {need}; it appears truncated (run `check`)",
                self.blob.len
            ));
        }
        match h.mode {
            Mode::ComplexInt16 | Mode::ComplexFloat32 => notes.push(
                "complex (Fourier-transform) data, mode 3/4: samples are (real, imaginary) pairs, returned as complex float32 (mode 3's int16 parts widened exactly), NX complex values per row as stored".into(),
            ),
            Mode::Float16 => notes.push("mode 12 half floats are returned widened to float32".into()),
            Mode::Packed4Bit => notes.push(
                "mode 101 4-bit values are returned unpacked, one uint8 (0-15) per pixel".into(),
            ),
            _ => {}
        }
        if h.bytes_unsigned() {
            notes
                .push("mode 0 bytes are unsigned (IMOD file without the signed-bytes flag)".into());
        }
        if self.layout == Layout::Volume && h.ispg == 0 {
            notes.push(format!(
                "ISPG 0 declares an image stack, but .{} files are volumes by convention: sections are exposed as Z",
                self.extension
            ));
        }
        if self.layout == Layout::ImageStack && h.nz > 1 {
            notes.push("ISPG 0 image stack: sections are exposed as T".into());
        }
        if h.axis_map().is_some_and(|m| m != [0, 1, 2]) {
            notes.push(format!(
                "MAPC/MAPR/MAPS = {}/{}/{}: planes are returned in file order (columns x rows per section), not rotated into X/Y/Z; extra.axis_order names the cell axis of each file axis",
                h.mapc, h.mapr, h.maps
            ));
        }
        let steps = h.file_axis_steps_angstrom();
        if steps[..2]
            .iter()
            .all(|s| s.is_some_and(|v| (v - 1.0).abs() < 1e-9))
            && h.mx == h.nx
            && h.my == h.ny
        {
            notes.push("pixel size is exactly 1 Å with CELLA = grid size, which many programs write when no calibration is known".into());
        }
        if let Some(p) = &h.stamp_problem {
            notes.push(p.clone());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.blob.len,
            format: MrcReader.descriptor(),
            format_version: (h.nversion > 0).then(|| h.nversion.to_string()),
            plane_count: image.plane_count,
            images: vec![image],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        let f3 = |a: [f32; 3]| json!(a.map(|v| num(f64::from(v))));
        let mut header = json!({
            "NX": h.nx, "NY": h.ny, "NZ": h.nz, "MODE": h.mode.code(),
            "NXSTART": h.nxstart, "NYSTART": h.nystart, "NZSTART": h.nzstart,
            "MX": h.mx, "MY": h.my, "MZ": h.mz,
            "CELLA": f3(h.cell_a), "CELLB": f3(h.cell_b),
            "MAPC": h.mapc, "MAPR": h.mapr, "MAPS": h.maps,
            "DMIN": num(f64::from(h.dmin)), "DMAX": num(f64::from(h.dmax)), "DMEAN": num(f64::from(h.dmean)),
            "ISPG": h.ispg, "NSYMBT": h.nsymbt, "EXTTYP": h.exttyp, "NVERSION": h.nversion,
            "ORIGIN": f3(h.origin), "MAP": if h.has_map_id { "MAP" } else { "" },
            "MACHST": h.machine_stamp.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            "RMS": num(f64::from(h.rms)), "NLABL": h.nlabl, "LABEL": h.labels,
        });
        if h.imod_stamp {
            header["imod_stamp"] = Value::from(super::header::IMOD_STAMP);
            header["imod_flags"] = Value::from(h.imod_flags);
        }
        let mut out = json!({ "header": header });
        if h.nsymbt > 0 {
            let mut ext =
                json!({"type": h.exttyp, "bytes": h.nsymbt, "decoded_as": self.ext_kind_name()});
            match self.ext_kind {
                ExtKind::Symmetry => ext["symmetry"] = json!(symmetry_lines(&self.ext)),
                ExtKind::Fei { .. } | ExtKind::Seri { .. } | ExtKind::IntsReals { .. } => {
                    let (total, recs) = self.records(Some(1000));
                    ext["record_count"] = Value::from(total);
                    ext["records"] = Value::Array(recs);
                }
                _ => {}
            }
            out["extended_header"] = ext;
        }
        Ok(out)
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Spec),
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::Spec),
            ("images[].size_t", Source::Spec),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::Spec),
            ("images[].extra.layout", Source::Spec),
            ("images[].extra.pixel_size_angstrom", Source::Spec),
            ("images[].extra.density", Source::Spec),
            ("images[].extra.bytes_signed", Source::PriorArt),
            ("images[].extra.fei_metadata", Source::PriorArt),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
            ("images[].channels[].exposure_ms", Source::Inferred),
            ("images[].extra.high_tension_kv", Source::Inferred),
            ("images[].extra.magnification", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let h = &self.header;
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: "header".into(),
            offset: Some(0),
            size: Some(HEADER_LEN as u64),
            image: None,
            details: json!({"mode": h.mode.code(), "nx": h.nx, "ny": h.ny, "nz": h.nz, "byte_order": byte_order_name(h.endian()), "map_id": h.has_map_id}),
        }];
        if h.nsymbt > 0 {
            let (records, _) = self.records(Some(0));
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "extended-header".into(),
                offset: Some(HEADER_LEN as u64),
                size: Some(h.nsymbt as u64),
                image: None,
                details: json!({"type": h.exttyp, "decoded_as": self.ext_kind_name(), "records": records}),
            });
        }
        let sec = h.section_bytes().unwrap_or(0);
        out.push(LsEntry {
            kind: "image".into(),
            name: "data".into(),
            offset: Some(h.data_offset()),
            size: self.data_len(),
            image: Some(0),
            details: json!({"sections": h.nz, "section_bytes": sec, "layout": self.layout.label()}),
        });
        if h.nz <= MAX_LISTED_SECTIONS {
            let (size_z, _) = self.sizes();
            for s in 0..h.nz.max(0) as u64 {
                out.push(LsEntry {
                    kind: "plane".into(),
                    name: format!("section {s}"),
                    offset: Some(h.data_offset().saturating_add(s.saturating_mul(sec))),
                    size: Some(sec),
                    image: Some(0),
                    details: json!({"z": s % u64::from(size_z), "t": s / u64::from(size_z)}),
                });
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image index {image} out of range (an MRC file holds one image)"
            )));
        }
        let h = &self.header;
        let s = self.section_of(index)?;
        let sec = h
            .section_bytes()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "section size overflows"))?;
        let off = s
            .checked_mul(sec)
            .and_then(|o| o.checked_add(h.data_offset()))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "section offset overflows"))?;
        let raw = self.blob.read_at(FORMAT_ID, off, sec)?;
        let data = self.decode(raw);
        let h = &self.header;
        Ok(Plane {
            width: h.nx as u32,
            height: h.ny as u32,
            pixel_type: pixel_type(h),
            samples_per_pixel: if h.mode == Mode::Rgb8 { 3 } else { 1 },
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let h = self.header.clone();
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("MAP identifier and machine stamp (byte order)");
        r.performed("MODE is an MRC2014 mode; NX/NY/NZ, MX/MY/MZ and CELLA are non-negative");
        r.performed(
            "MAPC/MAPR/MAPS are a permutation of 1, 2, 3; NZ divisible by MZ for volume stacks",
        );
        r.performed("NLABL equals the number of labels in use; NVERSION is 20140 or 20141");
        r.performed(
            "extended header: EXTTYP set, NSYMBT within the file, FEI block size consistent",
        );
        r.performed(
            "file size equals 1024 + NSYMBT + NX*NY*NZ*sample size (truncation, trailing bytes)",
        );
        if !h.has_map_id {
            r.push(
                Finding::warning(
                    "missing_map_id",
                    "bytes 209-211 are not 'MAP' (pre-2000 MRC files lack it)",
                )
                .at(208),
            );
        }
        if let Some(p) = &h.stamp_problem {
            let code = if p.starts_with("unrecognised") {
                "machine_stamp"
            } else {
                "machine_stamp_mismatch"
            };
            r.push(Finding::warning(code, p.clone()).at(212));
        }
        if !h.mode.in_spec() {
            r.push(
                Finding::info(
                    "nonstandard_mode",
                    format!(
                        "MODE {} ({}) is an IMOD extension, not MRC2014",
                        h.mode.code(),
                        h.mode.label()
                    ),
                )
                .at(12),
            );
        }
        for (name, v, off) in [
            ("MX", h.mx, 28),
            ("MY", h.my, 32),
            ("MZ", h.mz, 36),
            ("ISPG", h.ispg, 88),
            ("NLABL", h.nlabl, 220),
        ] {
            if v < 0 {
                r.push(
                    Finding::warning("negative_field", format!("{name} is negative ({v})")).at(off),
                );
            }
        }
        for (k, v) in h.cell_a.iter().enumerate() {
            if *v < 0.0 || !v.is_finite() {
                r.push(
                    Finding::warning("bad_cell", format!("CELLA {} is {v}", axis_letter(k)))
                        .at(40 + 4 * k as u64),
                );
            }
        }
        if h.axis_map().is_none() {
            r.push(Finding::warning("bad_axis_map", format!("MAPC/MAPR/MAPS = {}/{}/{} is not a permutation of 1, 2, 3; axes are taken as 1/2/3", h.mapc, h.mapr, h.maps)).at(64));
        }
        if (401..=630).contains(&h.ispg) && (h.mz <= 0 || h.nz % h.mz != 0) {
            r.push(
                Finding::error(
                    "bad_volume_stack",
                    format!(
                        "ISPG {} declares a volume stack but NZ={} is not a multiple of MZ={}",
                        h.ispg, h.nz, h.mz
                    ),
                )
                .at(36),
            );
        }
        let used = h.labels.iter().filter(|l| !l.is_empty()).count();
        if i32::try_from(used).ok() != Some(h.nlabl) {
            r.push(
                Finding::warning(
                    "label_count",
                    format!("NLABL is {} but {used} labels contain text", h.nlabl),
                )
                .at(220),
            );
        }
        if let Some(gap) = h.labels.iter().position(String::is_empty)
            && h.labels[gap..].iter().any(|l| !l.is_empty())
        {
            r.push(Finding::warning("label_gap", "an empty label precedes a used one").at(224));
        }
        match h.nversion {
            20140 | 20141 => {}
            0 => r.push(
                Finding::info(
                    "no_version",
                    "NVERSION is 0: the file predates MRC2014 or does not claim conformance",
                )
                .at(108),
            ),
            v => r.push(
                Finding::warning(
                    "unknown_version",
                    format!("NVERSION {v} is not an MRC2014 version"),
                )
                .at(108),
            ),
        }
        let (dr, dm, ds) = h.stats_undetermined();
        if dr || dm || ds {
            r.push(
                Finding::info(
                    "stats_undetermined",
                    "DMIN/DMAX/DMEAN/RMS are flagged as not determined (spec note 5)",
                )
                .at(76),
            );
        }
        if h.nsymbt > 0 {
            if h.exttyp.is_empty() {
                r.push(
                    Finding::info(
                        "exttyp_unset",
                        "an extended header is present but EXTTYP is not set",
                    )
                    .at(104),
                );
            } else if !matches!(
                h.exttyp.as_str(),
                "CCP4" | "MRCO" | "SERI" | "AGAR" | "FEI1" | "FEI2" | "HDF5"
            ) {
                r.push(
                    Finding::warning(
                        "exttyp_unknown",
                        format!("EXTTYP '{}' is not a registered code", h.exttyp),
                    )
                    .at(104),
                );
            }
            if HEADER_LEN as u64 + h.nsymbt as u64 > self.blob.len {
                r.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "the extended header (NSYMBT {}) runs past the end of the file",
                            h.nsymbt
                        ),
                    )
                    .at(HEADER_LEN as u64),
                );
            }
            if matches!(h.exttyp.as_str(), "FEI1" | "FEI2") {
                match self.ext_kind {
                    ExtKind::Fei { block } => {
                        let blocks = self.ext.len() / block;
                        if blocks < h.nz as usize {
                            r.push(Finding::warning("fei_blocks", format!("extended header holds {blocks} FEI blocks of {block} bytes for {} sections", h.nz)).at(HEADER_LEN as u64));
                        }
                        if h.exttyp == "FEI1" && block != FEI1_BLOCK_LEN {
                            r.push(Finding::warning("fei_block_size", format!("FEI1 blocks should be 768 bytes; the first block says {block}")).at(HEADER_LEN as u64));
                        }
                        if let Some(f) = self.first_fei() {
                            let fx = positive(f.get("pixel_size_x")).map(|m| m * 1e10);
                            let cx = h.file_axis_steps_angstrom()[0];
                            if let (Some(a), Some(b)) = (fx, cx)
                                && (a - b).abs() > 1e-3 * a.max(b)
                            {
                                r.push(Finding::warning("pixel_size_mismatch", format!("FEI metadata pixel size {a:.5} Å differs from CELLA/MX {b:.5} Å")));
                            }
                        }
                    }
                    _ => r.push(Finding::warning("fei_unreadable", format!("EXTTYP {} but the first block's size field is not a valid block size", h.exttyp)).at(HEADER_LEN as u64)),
                }
            }
        }
        match self.data_len().and_then(|d| d.checked_add(h.data_offset())) {
            None => r.push(Finding::error("bad_dimensions", "data size overflows")),
            Some(need) if need > self.blob.len => {
                let sec = h.section_bytes().unwrap_or(1).max(1);
                let have = self.blob.len.saturating_sub(h.data_offset()) / sec;
                r.push(Finding::error("truncated", format!("file is {} bytes but the header describes {need}: {} bytes missing, {have} of {} sections complete", self.blob.len, need - self.blob.len, h.nz)).at(self.blob.len));
            }
            Some(need) if need < self.blob.len => {
                r.push(
                    Finding::warning(
                        "trailing_bytes",
                        format!("{} bytes follow the data block", self.blob.len - need),
                    )
                    .at(need),
                );
            }
            Some(_) => {}
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        let h = &self.header;
        if h.nsymbt <= 0 {
            return Ok(Vec::new());
        }
        let mut extra = BTreeMap::new();
        extra.insert("decoded_as".into(), Value::from(self.ext_kind_name()));
        Ok(vec![AttachmentInfo {
            index: 0,
            name: "extended-header".into(),
            content_type: if h.exttyp.is_empty() {
                "MRC extended header".into()
            } else {
                h.exttyp.clone()
            },
            extension: "bin".into(),
            offset: Some(HEADER_LEN as u64),
            size: h.nsymbt as u64,
            extra,
        }])
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        if index != 0 || self.header.nsymbt <= 0 {
            return Err(Error::Usage(format!(
                "attachment #{index} does not exist; `info --view structure` lists them"
            )));
        }
        self.blob
            .read_at(FORMAT_ID, HEADER_LEN as u64, self.header.nsymbt as u64)
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        if image != 0 {
            return Ok((0, Vec::new()));
        }
        Ok(self.records(limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bit_unpacking_pads_rows() {
        // nx = 3: two bytes per row; the high nibble of each row's last byte is padding
        let raw = [0x21, 0x03, 0x54, 0x06];
        assert_eq!(unpack_4bit(&raw, 3, 2), vec![1, 2, 3, 4, 5, 6]);
    }
}

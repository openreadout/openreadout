//! An open MassHunter `.d` directory: the scan index in memory, spectra read lazily.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::le_f64;
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, InstrumentInfo, LsEntry, SignalChannelInfo, SpectraInfo,
    Spectrum, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, SpectrumView};
use openreadout_core::source::{Fs, Input};
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::layout::{
    Calibration, DATA_START, DefaultCalibration, FALLBACK_XSD_V5, FALLBACK_XSD_V6, FORMAT_PEAK,
    FORMAT_PROFILE, FORMAT_QUAD_PEAK, FrameMethod, FrameRecord, ScanFileHeader, ScanLayout,
    ScanRecord, SignalDef, SignalDirectory, decode_ims_profile, decode_peaks, decode_profile,
    decode_quad_profile, default_calibrations, frame_layout_from_xsd, frame_methods,
    mass_cal_values, read_frames, read_record, scan_file_header, scan_layout_from_xsd,
    signal_block, signal_directory,
};
use crate::{AgilentMsReader, FORMAT_ID};

/// `ScanType` of a neutral-loss scan (`MzOfInterest` holds the loss).
const SCAN_TYPE_NEUTRAL_LOSS: i32 = 2048;

/// Largest metadata file read whole (XML, `.cd`, `MSScan.bin` index, `MSMassCal.bin`).
const MAX_INDEX_BYTES: u64 = 1 << 31;

/// The `.d` directory a path names: the directory itself, or the one around `AcqData` or a
/// file inside `AcqData`.
pub fn dataset_dir(path: &Path) -> Option<PathBuf> {
    dataset_dir_in(&Fs::local(), path)
}

/// [`dataset_dir`] in the namespace `fs`.
pub(crate) fn dataset_dir_in(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let p = if fs.is_dir(path) {
        path.to_path_buf()
    } else {
        let parent = path.parent()?;
        if parent.file_name()?.eq_ignore_ascii_case("AcqData") {
            parent.parent()?.to_path_buf()
        } else {
            return None;
        }
    };
    if p.file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("AcqData"))
    {
        return p.parent().map(Path::to_path_buf);
    }
    Some(p)
}

/// The `AcqData` directory of a `.d` (the name is matched case-insensitively).
pub fn acq_dir(dir: &Path) -> Option<PathBuf> {
    acq_dir_in(&Fs::local(), dir)
}

/// [`acq_dir`] in the namespace `fs`.
pub(crate) fn acq_dir_in(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("AcqData");
    if fs.is_dir(&direct) {
        return Some(direct);
    }
    fs.read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            fs.is_dir(p)
                && p.file_name()
                    .is_some_and(|n| n.eq_ignore_ascii_case("AcqData"))
        })
}

/// A file of `AcqData` by name, matched case-insensitively.
fn find_file(fs: &Fs, acq: &Path, name: &str) -> Option<PathBuf> {
    let direct = acq.join(name);
    if fs.is_file(&direct) {
        return Some(direct);
    }
    fs.read_dir(acq)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| fs.is_file(p) && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn read_all(fs: &Fs, path: &Path) -> Result<Vec<u8>> {
    let len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
    if len > MAX_INDEX_BYTES {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "{} is {len} bytes, larger than any index file",
                path.display()
            ),
        ));
    }
    fs.read(path).map_err(|e| Error::io(path, e))
}

fn read_text(fs: &Fs, acq: &Path, name: &str) -> Option<String> {
    let p = find_file(fs, acq, name)?;
    let b = read_all(fs, &p).ok()?;
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&b);
    Some(String::from_utf8_lossy(b).into_owned())
}

/// Text of the first `<tag>…</tag>` in `xml`, trimmed; `None` when absent or empty.
pub fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let i = xml.find(&open)? + open.len();
    let j = xml[i..].find(&format!("</{tag}>"))?;
    let t = xml[i..i + j].trim();
    (!t.is_empty()).then(|| unescape(t))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// One `<Device>` of `Devices.xml`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Device {
    /// `DeviceID` attribute.
    pub id: i32,
    /// Module name (`QTOF`, `TandemQuadrupole`, `DAD`, `BinPump`, …).
    pub name: String,
    /// Model number (`G6540B`, …).
    pub model: Option<String>,
    /// Serial number.
    pub serial: Option<String>,
    /// Device type code.
    pub device_type: Option<i32>,
    /// Driver version.
    pub driver_version: Option<String>,
    /// Firmware version.
    pub firmware_version: Option<String>,
}

/// Parse `Devices.xml`.
pub fn devices(xml: &str) -> Vec<Device> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Device ") {
        let tail = &rest[i..];
        let end = tail.find("</Device>").unwrap_or(tail.len());
        let body = &tail[..end];
        let id = body
            .split_once("DeviceID=\"")
            .and_then(|(_, r)| r.split_once('"'))
            .and_then(|(v, _)| v.trim().parse().ok())
            .unwrap_or(0);
        out.push(Device {
            id,
            name: tag_text(body, "Name").unwrap_or_default(),
            model: tag_text(body, "ModelNumber"),
            serial: tag_text(body, "SerialNumber"),
            device_type: tag_text(body, "Type").and_then(|v| v.parse().ok()),
            driver_version: tag_text(body, "DriverVersion"),
            firmware_version: tag_text(body, "FirmwareVersion"),
        });
        if end == tail.len() {
            break;
        }
        rest = &tail[end..];
    }
    out
}

/// `(Name, Value)` of each `<Field>` of `sample_info.xml`.
pub fn sample_fields(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<Field>") {
        let tail = &rest[i..];
        let end = tail.find("</Field>").unwrap_or(tail.len());
        let body = &tail[..end];
        if let Some(n) = tag_text(body, "Name") {
            out.push((n, tag_text(body, "Value").unwrap_or_default()));
        }
        if end == tail.len() {
            break;
        }
        rest = &tail[end..];
    }
    out
}

/// A device signal exposed as a trace.
#[derive(Debug, Clone)]
struct Signal {
    file_stem: String,
    device_id: i32,
    device_name: Option<String>,
    def: SignalDef,
    cg: PathBuf,
}

/// The ion-mobility part of a data directory: frames and their methods.
#[derive(Debug)]
struct Ims {
    frames: Vec<FrameRecord>,
    methods: Vec<FrameMethod>,
    /// Frame-summed records (`DriftBin` 0) left out of the spectrum list.
    frame_sums: usize,
}

/// An open MassHunter data directory.
#[derive(Debug)]
pub struct AgilentDataset {
    /// Where the `.d` directory is read from.
    fs: Fs,
    dir: PathBuf,
    header: ScanFileHeader,
    layout: ScanLayout,
    layout_source: &'static str,
    /// Decoded scan records, in file order.
    records: Vec<ScanRecord>,
    /// Bytes after the last whole record (a truncated index when non-zero).
    trailing_bytes: u64,
    scan_file_len: u64,
    peak_path: Option<PathBuf>,
    profile_path: Option<PathBuf>,
    mass_cal: Option<Vec<u8>>,
    defaults: Vec<DefaultCalibration>,
    contents: Option<String>,
    devices_xml: Option<String>,
    msts: Option<String>,
    devices: Vec<Device>,
    samples: Vec<(String, String)>,
    method_name: Option<String>,
    signals: Vec<Signal>,
    size_bytes: u64,
    notes: Vec<String>,
    ims: Option<Ims>,
}

fn walk(fs: &Fs, dir: &Path, depth: u32, out: &mut Vec<(PathBuf, u64)>) {
    if depth > 8 {
        return;
    }
    let Ok(rd) = fs.read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if fs.is_dir(&p) {
            walk(fs, &p, depth + 1, out);
        } else if let Ok(m) = fs.metadata(&p) {
            out.push((p, m.len()));
        }
    }
}

impl AgilentDataset {
    /// Open a `.d` directory (or its `AcqData` directory, or a file inside `AcqData`).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, or a `.d` directory held in memory).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let dir = dataset_dir_in(fs, path).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("{} is not a MassHunter .d directory", path.display()),
            )
        })?;
        let acq = acq_dir_in(fs, &dir).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("{} has no AcqData directory", dir.display()),
            )
        })?;
        let scan_path = find_file(fs, &acq, "MSScan.bin").ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "MassHunter data without MSScan.bin",
                "This .d holds no mass-spectrometer scans (only device signals are read from such directories by the ChemStation reader, if at all).",
            )
        })?;
        let scan = read_all(fs, &scan_path)?;
        let header = scan_file_header(&scan).map_err(|e| Error::corrupt(FORMAT_ID, e))?;
        let (layout, layout_source) = if let Some(x) = read_text(fs, &acq, "MSScan.xsd") {
            (
                scan_layout_from_xsd(&x).map_err(|e| Error::corrupt(FORMAT_ID, e))?,
                "MSScan.xsd",
            )
        } else {
            let (text, which) = if header.layout_version <= 5 {
                (FALLBACK_XSD_V5, "built-in layout 5 (no MSScan.xsd)")
            } else {
                (FALLBACK_XSD_V6, "built-in layout 6 (no MSScan.xsd)")
            };
            (
                scan_layout_from_xsd(text).map_err(|e| Error::corrupt(FORMAT_ID, e))?,
                which,
            )
        };
        let rec_len = layout.record_len(header.block_count as usize);
        let body = (scan.len() - header.first_record as usize) as u64;
        let n = body / rec_len as u64;
        let trailing_bytes = body % rec_len as u64;
        let mut records = Vec::with_capacity(n as usize);
        for i in 0..n as usize {
            let r = read_record(&scan, &header, &layout, i).ok_or_else(|| {
                Error::corrupt_at(
                    FORMAT_ID,
                    header.first_record as u64 + (i * rec_len) as u64,
                    format!("scan record {i} cannot be decoded"),
                )
            })?;
            records.push(r);
        }
        let mut notes = Vec::new();
        if trailing_bytes != 0 {
            notes.push(format!(
                "truncated: MSScan.bin ends {trailing_bytes} bytes into scan record {n}; {n} complete scans are exposed"
            ));
        }
        let ims = if layout.ims {
            Some(Self::load_ims(fs, &acq, &mut records, &mut notes)?)
        } else {
            None
        };
        let mass_cal = find_file(fs, &acq, "MSMassCal.bin").and_then(|p| read_all(fs, &p).ok());
        let defaults = read_text(fs, &acq, "DefaultMassCal.xml")
            .map(|x| default_calibrations(&x))
            .unwrap_or_default();
        let contents = read_text(fs, &acq, "Contents.xml");
        let devices_xml = read_text(fs, &acq, "Devices.xml");
        let msts = read_text(fs, &acq, "MSTS.xml");
        let sample_info = read_text(fs, &acq, "sample_info.xml");
        let devs = devices_xml.as_deref().map(devices).unwrap_or_default();
        let samples = sample_info
            .as_deref()
            .map(sample_fields)
            .unwrap_or_default();
        let method_name = read_text(fs, &acq, "AcqMethod.xml")
            .and_then(|x| tag_text(&x, "MethodName"))
            .or_else(|| {
                samples
                    .iter()
                    .find(|(k, _)| k == "Method")
                    .and_then(|(_, v)| {
                        v.rsplit(['\\', '/'])
                            .next()
                            .map(str::to_string)
                            .filter(|s| !s.is_empty())
                    })
            });
        // Device signals: every *.cd with a *.cg beside it.
        let mut signals = Vec::new();
        if let Ok(rd) = fs.read_dir(&acq) {
            let mut cds: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cd")))
                .collect();
            cds.sort();
            for cd in cds {
                let cg = cd.with_extension("cg");
                if !fs.is_file(&cg) {
                    continue;
                }
                let stem = cd
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match read_all(fs, &cd)
                    .map_err(|e| e.to_string())
                    .and_then(|b| signal_directory(&b))
                {
                    Ok(SignalDirectory {
                        device_id,
                        signals: defs,
                    }) => {
                        let name = devs
                            .iter()
                            .find(|d| d.id == device_id)
                            .map(|d| d.name.clone());
                        for def in defs {
                            signals.push(Signal {
                                file_stem: stem.clone(),
                                device_id,
                                device_name: name.clone(),
                                def,
                                cg: cg.clone(),
                            });
                        }
                    }
                    Err(e) => notes.push(format!("{stem}.cd: {e}; its signals are not exposed")),
                }
            }
        }
        let mut files = Vec::new();
        walk(fs, &dir, 0, &mut files);
        let size_bytes = files.iter().map(|(_, n)| n).sum();
        let mut ds = AgilentDataset {
            fs: fs.clone(),
            dir,
            header,
            layout,
            layout_source,
            records,
            trailing_bytes,
            scan_file_len: scan.len() as u64,
            peak_path: find_file(fs, &acq, "MSPeak.bin"),
            profile_path: find_file(fs, &acq, "MSProfile.bin"),
            mass_cal,
            defaults,
            contents,
            devices_xml,
            msts,
            devices: devs,
            samples,
            method_name,
            signals,
            size_bytes,
            notes,
            ims,
        };
        ds.finish_ims();
        Ok(ds)
    }

    /// Read `IMSFrame.bin` (through `IMSFrame.xsd`) and `IMSFrameMeth.xml`, give every
    /// drift-bin record its frame's time, level, polarity and calibration, and drop the
    /// frame-summed records (`DriftBin` 0) from the scan list.
    fn load_ims(
        fs: &Fs,
        acq: &Path,
        records: &mut Vec<ScanRecord>,
        notes: &mut Vec<String>,
    ) -> Result<Ims> {
        let missing = |what: &str| {
            Error::unsupported(
                FORMAT_ID,
                format!("ion-mobility data without {what}"),
                "The scan index is an ion-mobility index (records per frame and drift bin); its frames and methods are needed to place them. Copy the whole .d directory.",
            )
        };
        let xsd = read_text(fs, acq, "IMSFrame.xsd").ok_or_else(|| missing("IMSFrame.xsd"))?;
        let (fields, rec_len) =
            frame_layout_from_xsd(&xsd).map_err(|e| Error::corrupt(FORMAT_ID, e))?;
        let path = find_file(fs, acq, "IMSFrame.bin").ok_or_else(|| missing("IMSFrame.bin"))?;
        let (frames, trailing) = read_frames(&read_all(fs, &path)?, &fields, rec_len)
            .map_err(|e| Error::corrupt(FORMAT_ID, e))?;
        if trailing != 0 {
            notes.push(format!(
                "truncated: IMSFrame.bin ends {trailing} bytes into frame record {}",
                frames.len()
            ));
        }
        let methods = read_text(fs, acq, "IMSFrameMeth.xml")
            .map(|x| frame_methods(&x))
            .ok_or_else(|| missing("IMSFrameMeth.xml"))?;
        let by_id: BTreeMap<i32, usize> = frames
            .iter()
            .enumerate()
            .map(|(i, f)| (f.frame_id, i))
            .collect();
        let before = records.len();
        records.retain(|r| r.drift_bin != Some(0));
        let frame_sums = before - records.len();
        for r in records.iter_mut() {
            let f = r
                .frame_id
                .and_then(|id| by_id.get(&id))
                .map(|&i| &frames[i])
                .ok_or_else(|| {
                    Error::corrupt_at(
                        FORMAT_ID,
                        r.record_offset,
                        format!(
                            "scan {} belongs to frame {:?}, which IMSFrame.bin does not hold",
                            r.scan_id, r.frame_id
                        ),
                    )
                })?;
            let m = methods
                .iter()
                .find(|m| m.id == f.method_id)
                .ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "frame {} uses frame method {}, which IMSFrameMeth.xml does not define",
                            f.frame_id, f.method_id
                        ),
                    )
                })?;
            r.scan_time_min = f.scan_time_min;
            r.ms_level = if f.frag_class == Some(2) { 2 } else { 1 };
            r.scan_type = 1;
            r.ion_polarity = m.ion_polarity;
            r.ion_mode = m.ionization_mode;
            r.calibration_id = m.mass_cal_id;
            r.mass_cal_offset = f.mass_cal_offset;
            r.sampling_period = m.ms_period_ns;
            r.scan_method_id = Some(f.method_id);
            r.time_segment_id = f.time_segment_id;
            r.cycle_number = f.cycle_number;
            r.drift_time_ms = r
                .drift_bin
                .zip(m.drift_period_ms)
                .map(|(d, p)| f64::from(d - 1) * p);
        }
        Ok(Ims {
            frames,
            methods,
            frame_sums,
        })
    }

    /// Ion-mobility records: the scan window and base-peak m/z from the frame method's
    /// flight-time grid and the calibration (the records store bins, not m/z).
    fn finish_ims(&mut self) {
        let Some(ims) = &self.ims else {
            return;
        };
        let base_bin = self
            .layout
            .fields
            .iter()
            .position(|f| f.name == "BaseMsBin");
        let mut fill = Vec::with_capacity(self.records.len());
        for r in &self.records {
            let m = r
                .scan_method_id
                .and_then(|id| ims.methods.iter().find(|m| m.id == id));
            let (Some(cal), Some(m)) = (self.calibration(r), m) else {
                fill.push(None);
                continue;
            };
            let (Some(period), Some(min_bin)) = (m.ms_period_ns, m.min_ms_bin) else {
                fill.push(None);
                continue;
            };
            let window = r.blocks.first().map(|b| {
                let hi = min_bin as f64 + (b.point_count - 1).max(0) as f64;
                [cal.mz(min_bin as f64 * period), cal.mz(hi * period)]
            });
            let base = base_bin
                .and_then(|i| r.values.get(i).copied())
                .filter(|&b| b > 0.0)
                .map(|b| cal.mz(b * period));
            fill.push(Some((window, base)));
        }
        for (r, f) in self.records.iter_mut().zip(fill) {
            if let Some((window, base)) = f {
                if let (Some([lo, hi]), Some(b)) = (window, r.blocks.first_mut()) {
                    b.min_x = Some(lo);
                    b.max_x = Some(hi);
                }
                if let Some(b) = base {
                    r.base_peak_mz = b;
                }
            }
        }
    }

    /// An ion-mobility drift-bin spectrum: its profile block decoded, non-zero bins and the zero
    /// bins next to them, calibrated to m/z.
    fn ims_spectrum(&self, r: &ScanRecord, index: u64) -> Result<Spectrum> {
        let blk = r.blocks.first();
        let mut sp = Self::meta_of(r, index, blk, true, false);
        let Some(b) = blk else {
            return Ok(sp);
        };
        if b.uncompressed_byte_count
            .is_some_and(|u| u > 0 && u != b.byte_count)
        {
            return Err(Error::unsupported(
                FORMAT_ID,
                "compressed ion-mobility profile blocks",
                "Only uncompressed drift-bin profiles (MsProfFullByteCount = MsProfByteCount) have been seen and validated.",
            ));
        }
        let path = self
            .profile_path
            .as_ref()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "MSProfile.bin is missing"))?;
        let bytes = Self::read_block_bytes(&self.fs, path, b.offset, b.byte_count)?;
        let p = decode_ims_profile(&bytes).map_err(|e| {
            Error::unsupported(
                FORMAT_ID,
                format!("scan {}: {e}", r.scan_id),
                "This ion-mobility profile block does not match the validated encoding.",
            )
        })?;
        if i64::from(p.point_count) != b.point_count {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                b.offset as u64,
                format!(
                    "scan {}: profile block holds a {}-bin grid; the index records {}",
                    r.scan_id, p.point_count, b.point_count
                ),
            ));
        }
        let cal = self.calibration(r).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "ion-mobility profiles without a calibration",
                "Neither MSMassCal.bin nor DefaultMassCal.xml holds the frame method's calibration.",
            )
        })?;
        let mut bins: Vec<(u32, i64)> = Vec::with_capacity(p.bins.len() * 2);
        for (k, &(bin, v)) in p.bins.iter().enumerate() {
            if v == 0 {
                continue;
            }
            if bin > 0 && bins.last().is_none_or(|&(last, _)| last + 1 < bin) {
                bins.push((bin - 1, 0));
            }
            bins.push((bin, v));
            let next_stored = p
                .bins
                .get(k + 1)
                .is_some_and(|&(n, nv)| n == bin + 1 && nv != 0);
            if !next_stored && bin + 1 < p.point_count {
                bins.push((bin + 1, 0));
            }
        }
        sp.mz = bins
            .iter()
            .map(|&(bin, _)| cal.mz(p.step_x.mul_add(f64::from(bin), p.first_x)))
            .collect();
        sp.intensity = bins.iter().map(|&(_, v)| v as f32).collect();
        Ok(sp)
    }

    /// Decoded scan records in file order.
    pub fn records(&self) -> &[ScanRecord] {
        &self.records
    }

    /// The record layout in use and where it came from.
    pub fn layout(&self) -> (&ScanLayout, &'static str) {
        (&self.layout, self.layout_source)
    }

    fn ms_device(&self) -> Option<&Device> {
        self.devices.iter().find(|d| {
            let n = d.name.to_ascii_lowercase();
            n.contains("tof") || n.contains("quad") || n == "ms" || n.contains("msd")
        })
    }

    fn contents_tag(&self, tag: &str) -> Option<String> {
        self.contents.as_deref().and_then(|c| tag_text(c, tag))
    }

    fn sample(&self, name: &str) -> Option<String> {
        self.samples
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// The flight-time → m/z calibration of a scan, if the directory holds one for it.
    pub fn calibration(&self, r: &ScanRecord) -> Option<Calibration> {
        let default = r
            .calibration_id
            .and_then(|id| self.defaults.iter().find(|d| d.id == id));
        let flags = default.map_or(0, |d| d.flags);
        if let (Some(mc), Some(off)) = (&self.mass_cal, r.mass_cal_offset)
            && off >= DATA_START as i64
            && let Some(v) = mass_cal_values(mc, off as usize)
        {
            return Calibration::from_values(&v, flags);
        }
        default.and_then(|d| Calibration::from_values(&d.values, d.flags))
    }

    fn read_block_bytes(fs: &Fs, path: &Path, offset: i64, len: i64) -> Result<Vec<u8>> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let (Ok(off), Ok(n)) = (u64::try_from(offset), u64::try_from(len)) else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("negative spectrum offset {offset} or length {len}"),
            ));
        };
        if off.checked_add(n).is_none_or(|e| e > file_len) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                off,
                format!(
                    "spectrum block of {n} bytes at {off} runs past the end of {} ({file_len} bytes)",
                    path.display()
                ),
            ));
        }
        f.seek(SeekFrom::Start(off))
            .map_err(|e| Error::io(path, e))?;
        let mut buf = vec![0u8; n as usize];
        f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
        Ok(buf)
    }

    /// Peaks of a centroid/quadrupole block as (m/z, abundance), sorted by m/z.
    fn peaks(
        &self,
        r: &crate::layout::ScanRecord,
        blk: &crate::layout::SpectrumBlock,
    ) -> Result<(Vec<f64>, Vec<f32>)> {
        let path = self
            .peak_path
            .as_ref()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "MSPeak.bin is missing"))?;
        let n = usize::try_from(blk.point_count)
            .map_err(|_| Error::corrupt(FORMAT_ID, "negative point count"))?;
        let bytes = Self::read_block_bytes(&self.fs, path, blk.offset, blk.byte_count)?;
        let d = decode_peaks(&bytes, n).map_err(|e| {
            Error::unsupported(
                FORMAT_ID,
                e,
                "Only uncompressed peak blocks have been seen in corpus files.",
            )
        })?;
        if n > 0 && Some(bytes.len()) == n.checked_mul(16) && !self.is_gcms_points(blk) {
            return self.wide_tof_peaks(r, d);
        }
        let d = if n > 0 && Some(bytes.len()) == n.checked_mul(8) {
            crate::layout::eight_byte_abundances(&bytes, n, d, blk.max_y)
        } else {
            d
        };
        // Stored x is m/z when it already equals the block's MinX (quadrupole data, or files
        // without a calibration); otherwise it is a flight time to calibrate.
        let raw_is_mz = match (d.x.first(), blk.min_x) {
            (Some(&x0), Some(min)) => {
                (x0 - min).abs() <= 1e-6 * min.abs().max(1.0)
                    || d.x
                        .iter()
                        .all(|&x| (x - min).abs() > 1e-6 * min.abs().max(1.0))
                        && self.calibration(r).is_none()
            }
            _ => self.calibration(r).is_none(),
        };
        let mz: Vec<f64> = if raw_is_mz
            || blk.format_id == FORMAT_QUAD_PEAK
            || self.is_quad_points(blk)
            || self.is_gcms_points(blk)
        {
            d.x
        } else {
            let cal = self.calibration(r).ok_or_else(|| {
                Error::unsupported(FORMAT_ID, "time-of-flight peaks without a calibration", "Neither MSMassCal.bin nor DefaultMassCal.xml holds a calibration for this scan.")
            })?;
            d.x.iter().map(|&t| cal.mz(t)).collect()
        };
        Ok(sort_pairs(mz, d.y))
    }

    /// A centroid list of 16 bytes per point outside GC/MS point lists (a 7200 GC/Q-TOF): f64
    /// flight times and f64 abundances; MinX holds a flight time too, so the stored x is
    /// calibrated. The most intense point must land on the record's base-peak m/z, or the
    /// scan is refused.
    fn wide_tof_peaks(
        &self,
        r: &crate::layout::ScanRecord,
        d: crate::layout::PeakData,
    ) -> Result<(Vec<f64>, Vec<f32>)> {
        let cal = self.calibration(r).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "time-of-flight peaks without a calibration",
                "Neither MSMassCal.bin nor DefaultMassCal.xml holds a calibration for this scan.",
            )
        })?;
        let mz: Vec<f64> = d.x.iter().map(|&t| cal.mz(t)).collect();
        let top =
            d.y.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(k, _)| k);
        if let Some(k) = top
            && r.base_peak_mz > 0.0
            && (mz[k] - r.base_peak_mz).abs() > 1e-4 * r.base_peak_mz
        {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "scan {}: 16-byte centroids whose most intense point calibrates to m/z {:.4}, not the stored base peak {:.4}",
                    r.scan_id, mz[k], r.base_peak_mz
                ),
                "This centroid layout has been decoded on one GC/Q-TOF file only; report the file with `openreadout report`.",
            ));
        }
        Ok(sort_pairs(mz, d.y))
    }

    /// Layout-5 files come from triple-quadrupole instruments (collision-induced dissociation).
    fn triple_quad(&self) -> bool {
        self.header.layout_version <= 5
    }

    /// A layout-5 block of spectrum format 1 that holds `8n` bytes in `MSPeak.bin`: an MRM
    /// transition list (f32 m/z × n, then i32 counts × n), not a profile.
    fn is_quad_points(&self, blk: &crate::layout::SpectrumBlock) -> bool {
        blk.format_id == FORMAT_PROFILE
            && self.triple_quad()
            && self.peak_path.is_some()
            && blk.point_count > 0
            && blk.point_count.checked_mul(8) == Some(blk.byte_count)
    }

    /// A block of spectrum format 1 that lies in `MSPeak.bin` because the data directory has no
    /// `MSProfile.bin` (single-quadrupole GC/MS acquisitions): `16n` bytes, f64 m/z × n then
    /// f64 abundances × n.
    fn is_gcms_points(&self, blk: &crate::layout::SpectrumBlock) -> bool {
        blk.format_id == FORMAT_PROFILE
            && self.profile_path.is_none()
            && self.peak_path.is_some()
            && blk.point_count > 0
            && blk.point_count.checked_mul(16) == Some(blk.byte_count)
    }

    /// The record's profile block (when `MSProfile.bin` exists to hold it) and its point list.
    fn blocks_of<'a>(
        &self,
        r: &'a ScanRecord,
    ) -> (
        Option<&'a crate::layout::SpectrumBlock>,
        Option<&'a crate::layout::SpectrumBlock>,
    ) {
        let profile = r.block(FORMAT_PROFILE).filter(|b| {
            b.point_count > 0
                && b.byte_count > 0
                && self.profile_path.is_some()
                && !self.is_quad_points(b)
        });
        let peak = r
            .blocks
            .iter()
            .find(|b| {
                b.format_id == FORMAT_PEAK
                    || b.format_id == FORMAT_QUAD_PEAK
                    || self.is_quad_points(b)
                    || self.is_gcms_points(b)
            })
            .filter(|b| b.byte_count > 0 || b.point_count == 0);
        (profile, peak)
    }

    /// A block of spectrum format 2 that is a quadrupole profile in `MSProfile.bin`: layout-5
    /// (quadrupole) files whose block holds `8 + 4n` bytes (f32 start, f32 step, i32 counts).
    fn is_quad_profile(&self, blk: &crate::layout::SpectrumBlock) -> bool {
        blk.format_id == FORMAT_PEAK
            && self.header.layout_version <= 5
            && self.profile_path.is_some()
            && blk.point_count > 0
            && blk
                .point_count
                .checked_mul(4)
                .and_then(|v| v.checked_add(8))
                == Some(blk.byte_count)
    }

    /// A quadrupole profile as (m/z, abundance): non-zero bins and the zero bins next to them.
    ///
    /// The block's own step is stored with a reduced mantissa (0.1 reads as 0.1000061); the scan
    /// record's `SamplingPeriod` holds the step as set (0.1), so bin `k` is `first + k * period`,
    /// rounded to f32 as the vendor's m/z values are. Without a usable period the block's step
    /// is used.
    fn quad_profile(
        &self,
        r: &crate::layout::ScanRecord,
        blk: &crate::layout::SpectrumBlock,
    ) -> Result<(Vec<f64>, Vec<f32>)> {
        let path = self
            .profile_path
            .as_ref()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "MSProfile.bin is missing"))?;
        let n = usize::try_from(blk.point_count)
            .map_err(|_| Error::corrupt(FORMAT_ID, "negative point count"))?;
        if n > 1 << 26 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("profile declares {n} bins"),
            ));
        }
        let bytes = Self::read_block_bytes(&self.fs, path, blk.offset, blk.byte_count)?;
        let (first, block_step, counts) = decode_quad_profile(&bytes, n)
            .map_err(|e| Error::corrupt_at(FORMAT_ID, blk.offset as u64, e))?;
        // The period is stored as an f32 widened to f64; round it back to the nearest short
        // decimal the f32 represents (0.10000000149 -> 0.1).
        let step = r
            .sampling_period
            .filter(|p| p.is_finite() && *p > 0.0 && (p - block_step).abs() < 1e-3 * block_step)
            .map_or(block_step, |p| {
                let short: f64 = format!("{}", p as f32).parse().unwrap_or(p);
                short
            });
        let keep = |i: usize| {
            counts[i] != 0
                || (i > 0 && counts[i - 1] != 0)
                || (i + 1 < counts.len() && counts[i + 1] != 0)
        };
        let mut mz = Vec::new();
        let mut abundance = Vec::new();
        for i in (0..counts.len()).filter(|&i| keep(i)) {
            mz.push(f64::from(step.mul_add(i as f64, first) as f32));
            abundance.push(counts[i] as f32);
        }
        Ok((mz, abundance))
    }

    /// A profile block as (m/z, abundance), keeping non-zero bins and the zero bins next to them.
    fn profile(
        &self,
        r: &crate::layout::ScanRecord,
        blk: &crate::layout::SpectrumBlock,
    ) -> Result<(Vec<f64>, Vec<f32>)> {
        let path = self
            .profile_path
            .as_ref()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "MSProfile.bin is missing"))?;
        let n = usize::try_from(blk.point_count)
            .map_err(|_| Error::corrupt(FORMAT_ID, "negative point count"))?;
        if n > 1 << 26 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("profile declares {n} bins"),
            ));
        }
        let bytes = Self::read_block_bytes(&self.fs, path, blk.offset, blk.byte_count)?;
        let prof = decode_profile(&bytes, n, blk.uncompressed_byte_count)
            .map_err(|e| Error::corrupt_at(FORMAT_ID, blk.offset as u64, e))?;
        let cal = self.calibration(r).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "profile without a calibration",
                "Neither MSMassCal.bin nor DefaultMassCal.xml holds a calibration for this scan.",
            )
        })?;
        let counts = &prof.counts;
        let keep = |i: usize| {
            counts[i] != 0
                || (i > 0 && counts[i - 1] != 0)
                || (i + 1 < counts.len() && counts[i + 1] != 0)
        };
        let mut mz = Vec::new();
        let mut abundance = Vec::new();
        for i in (0..counts.len()).filter(|&i| keep(i)) {
            mz.push(cal.mz(prof.first_x + prof.step_x * i as f64));
            abundance.push(counts[i] as f32);
        }
        Ok((mz, abundance))
    }

    fn spectrum_of(&self, index: u64, view: SpectrumView) -> Result<Spectrum> {
        let r = usize::try_from(index)
            .ok()
            .and_then(|i| self.records.get(i))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectrum {index} does not exist ({} scans)",
                    self.records.len()
                ))
            })?;
        if r.frame_id.is_some() {
            return self.ims_spectrum(r, index);
        }
        let (profile, peak) = self.blocks_of(r);
        // Older quadrupole files store their profile as spectrum format 2 in MSProfile.bin.
        if let Some(b) = peak.filter(|b| self.is_quad_profile(b)) {
            let (mz, intensity) = self.quad_profile(r, b)?;
            let mut sp = Self::meta_of(r, index, Some(b), true, false);
            sp.mz = mz;
            sp.intensity = intensity;
            return Ok(sp);
        }
        // A profile kept in an MSProfile.bin the directory does not hold (a partial copy) is
        // missing data, not an empty spectrum.
        if peak.is_none()
            && profile.is_none()
            && self.profile_path.is_none()
            && let Some(b) = r
                .block(FORMAT_PROFILE)
                .filter(|b| b.point_count > 0 && b.byte_count > 0)
        {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "scan {}: its profile ({} points) is stored in AcqData/MSProfile.bin, which this data directory does not hold",
                    r.scan_id, b.point_count
                ),
            ));
        }
        let use_profile = match view {
            SpectrumView::Centroid => peak.is_none() && profile.is_some(),
            _ => profile.is_some(),
        };
        let (blk, (mz, intensity)) = if use_profile {
            let b = profile.expect("checked");
            (Some(b), self.profile(r, b)?)
        } else if let Some(b) = peak {
            (Some(b), self.peaks(r, b)?)
        } else {
            (None, (Vec::new(), Vec::new()))
        };
        let mrm_points = blk.is_some_and(|b| self.is_quad_points(b));
        let mut sp = Self::meta_of(r, index, blk, use_profile, mrm_points);
        if blk.is_some_and(|b| self.is_gcms_points(b)) {
            // these blocks' MinX/MaxX are not m/z (they hold a twentieth of it)
            sp.scan_window_mz = None;
        }
        sp.mz = mz;
        sp.intensity = intensity;
        Ok(sp)
    }

    /// Header of spectrum `index` from its scan record alone (no block read): the primary
    /// view's block (profile when stored, else the peak list) sets `centroided`, the scan
    /// window and `point_count`.
    fn header_of(&self, index: u64) -> Result<openreadout_core::ScanHeader> {
        let r = usize::try_from(index)
            .ok()
            .and_then(|i| self.records.get(i))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectrum {index} does not exist ({} scans)",
                    self.records.len()
                ))
            })?;
        let (profile, peak) = self.blocks_of(r);
        let ims_block = r.frame_id.and(r.blocks.first());
        let blk = ims_block.or(profile).or(peak);
        let quad_profile = ims_block.is_some() || peak.is_some_and(|b| self.is_quad_profile(b));
        let mut h = openreadout_core::ScanHeader::from(Self::meta_of(
            r,
            index,
            blk,
            profile.is_some() || quad_profile,
            blk.is_some_and(|b| self.is_quad_points(b)),
        ));
        if blk.is_some_and(|b| self.is_gcms_points(b)) {
            h.scan_window_mz = None;
        }
        h.point_count = blk.and_then(|b| u64::try_from(b.point_count).ok());
        Ok(h)
    }

    /// Spectrum metadata of scan record `r` read from block `blk` (arrays left empty).
    fn meta_of(
        r: &crate::layout::ScanRecord,
        index: u64,
        blk: Option<&crate::layout::SpectrumBlock>,
        use_profile: bool,
        mrm_points: bool,
    ) -> Spectrum {
        let ms2 = r.ms_level >= 2;
        let mut extra = BTreeMap::new();
        extra.insert("scan_type".into(), json!(r.scan_type));
        for (k, v) in [
            ("fragmentor_v", r.fragmentor.map(Value::from)),
            ("ion_mode", r.ion_mode.map(Value::from)),
            ("scan_method", r.scan_method_id.map(Value::from)),
            ("time_segment", r.time_segment_id.map(Value::from)),
            ("cycle", r.cycle_number.map(Value::from)),
        ] {
            if let Some(v) = v {
                extra.insert(k.into(), v);
            }
        }
        if let Some(p) = r.parent_scan_id.filter(|&p| p != r.scan_id && p > 0) {
            extra.insert("parent_scan".into(), json!(p));
        }
        // A neutral-loss scan's m/z of interest is the loss it scans for, not a precursor.
        let neutral_loss = r.scan_type == SCAN_TYPE_NEUTRAL_LOSS;
        if neutral_loss && let Some(m) = r.mz_of_interest.filter(|&m| m > 0.0) {
            extra.insert("neutral_loss_mz".into(), json!(m));
        }
        if let (Some(f), Some(d)) = (r.frame_id, r.drift_bin) {
            extra.insert("frame".into(), json!(f));
            extra.insert("drift_bin".into(), json!(d));
            if let Some(t) = r.drift_time_ms {
                extra.insert("drift_time_ms".into(), json!(t));
            }
        }
        let quad = mrm_points || blk.is_some_and(|b| b.format_id == FORMAT_QUAD_PEAK);
        Spectrum {
            index,
            scan_number: u64::try_from(r.scan_id).unwrap_or(0),
            ms_level: u32::try_from(r.ms_level).unwrap_or(0),
            rt_s: Some(r.scan_time_min * 60.0),
            polarity: match r.ion_polarity {
                Some(0) => "positive",
                Some(1) => "negative",
                _ => "unknown",
            }
            .into(),
            centroided: !use_profile,
            precursor_mz: r
                .mz_of_interest
                .filter(|&m| ms2 && m > 0.0 && !neutral_loss),
            precursor_charge: r.charge_state.filter(|&z| ms2 && z > 0),
            scan_filter: None,
            total_ion_current: Some(r.tic),
            native_id: Some(format!("scanId={}", r.scan_id)),
            base_peak_mz: Some(r.base_peak_mz),
            base_peak_intensity: Some(r.base_peak_value),
            precursor_intensity: None,
            isolation_window_mz: None,
            // Collision cells fragment by beam-type CID (MS:1000422, the term our mzML writer
            // uses for `HCD`), as ProteoWizard labels Q-TOF and triple-quadrupole scans; MRM
            // point lists are labelled CID, as the MRM exports label their transitions.
            activation: ms2.then(|| if quad { "CID" } else { "HCD" }.to_string()),
            // Negative-ion scans store the collision energy with the polarity's sign (-20 where
            // the method sets 20): the energy is its magnitude.
            collision_energy: r.collision_energy.filter(|_| ms2).map(f64::abs),
            inverse_reduced_mobility: None,
            scan_window_mz: blk
                .and_then(|b| Some([b.min_x?, b.max_x?]))
                .filter(|w| w[1] >= w[0] && w[1] > 0.0),
            extra,
            mz: Vec::new(),
            intensity: Vec::new(),
        }
    }

    fn ms_levels(&self) -> Vec<u32> {
        let s: BTreeSet<u32> = self
            .records
            .iter()
            .map(|r| u32::try_from(r.ms_level).unwrap_or(0))
            .collect();
        s.into_iter().collect()
    }

    fn spectra_info(&self) -> SpectraInfo {
        let rt = if self.records.is_empty() {
            None
        } else {
            let lo = self
                .records
                .iter()
                .map(|r| r.scan_time_min)
                .fold(f64::INFINITY, f64::min);
            let hi = self
                .records
                .iter()
                .map(|r| r.scan_time_min)
                .fold(f64::NEG_INFINITY, f64::max);
            Some([lo * 60.0, hi * 60.0])
        };
        let dev = self.ms_device();
        let mut extra = BTreeMap::new();
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                extra.insert(k.to_string(), Value::from(v));
            }
        };
        put("sample_name", self.sample("Sample Name"));
        put("sample_id", self.sample("Sample ID"));
        put("vial", self.sample("Sample Position"));
        put("acquired_at", self.contents_tag("AcquiredTime"));
        put("instrument_name", self.contents_tag("InstrumentName"));
        put("instrument_serial", dev.and_then(|d| d.serial.clone()));
        put("ms_device", dev.map(|d| d.name.clone()));
        put("method", self.method_name.clone());
        put("operator", self.sample("OperatorName"));
        let pols: BTreeSet<&str> = self
            .records
            .iter()
            .map(|r| match r.ion_polarity {
                Some(0) => "positive",
                Some(1) => "negative",
                _ => "unknown",
            })
            .collect();
        extra.insert(
            "polarities".into(),
            json!(pols.into_iter().collect::<Vec<_>>()),
        );
        let types: BTreeSet<i32> = self.records.iter().map(|r| r.scan_type).collect();
        extra.insert(
            "scan_types".into(),
            json!(types.into_iter().collect::<Vec<_>>()),
        );
        let data = if self
            .records
            .iter()
            .any(|r| r.block(FORMAT_PROFILE).is_some_and(|b| b.point_count > 0))
        {
            if self.records.iter().any(|r| r.block(FORMAT_PEAK).is_some()) {
                "profile+centroid"
            } else {
                "profile"
            }
        } else {
            "centroid"
        };
        let data = if self.ims.is_some() { "profile" } else { data };
        extra.insert("stored_spectra".into(), json!(data));
        // Scans whose only data is a profile kept in an MSProfile.bin the directory does not
        // hold (a partial copy). A centroid-only acquisition still declares profile blocks; its
        // scans are read from their peak lists.
        if self.profile_path.is_none()
            && self.records.iter().any(|r| {
                let (profile, peak) = self.blocks_of(r);
                profile.is_none()
                    && peak.is_none()
                    && r.block(FORMAT_PROFILE)
                        .is_some_and(|b| b.point_count > 0 && b.byte_count > 0)
            })
        {
            extra.insert("missing_files".into(), json!(["AcqData/MSProfile.bin"]));
        }
        if let Some(ims) = &self.ims {
            extra.insert(
                "ion_mobility".into(),
                json!({
                    "frames": ims.frames.len(),
                    "drift_spectra": self.records.len(),
                    "frame_sum_spectra_not_listed": ims.frame_sums,
                    "drift_time_unit": "ms",
                }),
            );
        }
        extra.insert(
            "method_summary".into(),
            json!({
                "instrument_method": self.sample("Method").or_else(|| self.method_name.clone()),
                "devices": self.devices.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
            }),
        );
        SpectraInfo {
            index: 0,
            name: self.sample("Sample Name").or_else(|| {
                self.dir
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            }),
            scan_count: self.records.len() as u64,
            ms_levels: self.ms_levels(),
            rt_range_s: rt,
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Agilent Technologies".into()),
                model: dev
                    .and_then(|d| d.model.clone())
                    .or_else(|| dev.map(|d| d.name.clone())),
                software: Some("MassHunter Data Acquisition".into()),
                software_version: self.contents_tag("AcqSoftwareVersion"),
                detector: None,
            }),
            extra,
        }
    }

    fn trace_infos(&self) -> Vec<TraceInfo> {
        let chan = |i: u32, name: &str, unit: Option<&str>| SignalChannelInfo {
            index: i,
            name: name.into(),
            unit: unit.map(str::to_string),
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        };
        let mut out = Vec::new();
        if !self.records.is_empty() {
            for (i, name) in ["TIC", "BPC"].iter().enumerate() {
                let mut extra = BTreeMap::new();
                extra.insert(
                    "kind".into(),
                    json!(if i == 0 {
                        "total ion current"
                    } else {
                        "base peak"
                    }),
                );
                extra.insert("source".into(), json!("MSScan.bin"));
                extra.insert("irregular_sampling".into(), json!(true));
                if self.ims.is_some() {
                    extra.insert("source".into(), json!("IMSFrame.bin (one point per frame)"));
                }
                out.push(TraceInfo {
                    index: i as u32,
                    name: Some((*name).to_string()),
                    sample_rate_hz: 0.0,
                    sample_count: self.chromatogram_rows().len() as u64,
                    sweep_count: 1,
                    channels: vec![
                        chan(0, "time", Some("s")),
                        chan(1, "intensity", Some("counts")),
                    ],
                    start_s: self.records.first().map(|r| r.scan_time_min * 60.0),
                    extra,
                });
            }
        }
        let first = out.len() as u32;
        for (k, s) in self.signals.iter().enumerate() {
            let step_s = self.signal_step(s) * 60.0;
            let mut extra = BTreeMap::new();
            extra.insert("device_file".into(), json!(format!("{}.cd", s.file_stem)));
            extra.insert("device_id".into(), json!(s.device_id));
            if let Some(n) = &s.device_name {
                extra.insert("device".into(), json!(n));
            }
            extra.insert("signal".into(), json!(s.def.id));
            extra.insert(
                "kind".into(),
                json!(if s.def.kind == 1 {
                    "detector signal"
                } else {
                    "instrument reading"
                }),
            );
            let desc = s.def.description.trim();
            let name = if s.def.id.is_empty() {
                format!("{} {desc}", s.file_stem)
            } else {
                format!("{} {}: {desc}", s.file_stem, s.def.id)
            };
            out.push(TraceInfo {
                index: first + k as u32,
                name: Some(name),
                sample_rate_hz: if step_s > 0.0 { 1.0 / step_s } else { 0.0 },
                sample_count: u64::try_from(s.def.count).unwrap_or(0),
                sweep_count: 1,
                channels: vec![
                    chan(0, "time", Some("s")),
                    chan(1, desc, Some(s.def.unit.as_str()).filter(|u| !u.is_empty())),
                ],
                start_s: Some(self.signal_start(s) * 60.0),
                extra,
            });
        }
        out
    }

    /// (time in minutes, TIC, base-peak abundance) per scan, or per frame in ion-mobility data.
    fn chromatogram_rows(&self) -> Vec<(f64, f64, f64)> {
        match &self.ims {
            Some(ims) => ims
                .frames
                .iter()
                .map(|f| (f.scan_time_min, f.tic, f.base_abundance))
                .collect(),
            None => self
                .records
                .iter()
                .map(|r| (r.scan_time_min, r.tic, r.base_peak_value))
                .collect(),
        }
    }

    fn signal_header(fs: &Fs, s: &Signal) -> Option<(f64, f64)> {
        let mut f = fs.open(&s.cg).ok()?;
        f.seek(SeekFrom::Start(u64::try_from(s.def.offset).ok()?))
            .ok()?;
        let mut b = [0u8; 16];
        f.read_exact(&mut b).ok()?;
        Some((le_f64(&b, 0)?, le_f64(&b, 8)?))
    }
    fn signal_start(&self, s: &Signal) -> f64 {
        Self::signal_header(&self.fs, s).map_or(0.0, |h| h.0)
    }
    fn signal_step(&self, s: &Signal) -> f64 {
        Self::signal_header(&self.fs, s).map_or(0.0, |h| h.1)
    }
}

fn sort_pairs(mz: Vec<f64>, y: Vec<f32>) -> (Vec<f64>, Vec<f32>) {
    if mz.windows(2).all(|w| w[0] <= w[1]) {
        return (mz, y);
    }
    let mut idx: Vec<usize> = (0..mz.len()).collect();
    idx.sort_by(|&a, &b| mz[a].total_cmp(&mz[b]));
    (
        idx.iter().map(|&i| mz[i]).collect(),
        idx.iter().map(|&i| y[i]).collect(),
    )
}

impl Dataset for AgilentDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.dir.display().to_string(),
            size_bytes: self.size_bytes,
            format: AgilentMsReader.descriptor(),
            format_version: Some(format!("MSScan layout {}", self.header.layout_version)),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()],
            traces: self.trace_infos(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let xml = |t: &Option<String>| t.as_deref().and_then(xml_to_json).unwrap_or(Value::Null);
        let first = self.records.first().map(|r| {
            let values: serde_json::Map<String, Value> = self
                .layout
                .fields
                .iter()
                .zip(&r.values)
                .map(|(f, v)| (f.name.clone(), json!(v)))
                .collect();
            json!({
                "values": values,
                "blocks": r.blocks.iter().map(|b| json!({
                    "SpectrumFormatID": b.format_id, "SpectrumOffset": b.offset, "ByteCount": b.byte_count,
                    "PointCount": b.point_count, "UncompressedByteCount": b.uncompressed_byte_count,
                    "MinX": b.min_x, "MaxX": b.max_x, "MinY": b.min_y, "MaxY": b.max_y,
                })).collect::<Vec<_>>(),
            })
        });
        Ok(json!({
            "Contents": xml(&self.contents),
            "Devices": xml(&self.devices_xml),
            "MSTS": xml(&self.msts),
            "sample_info": self.samples.iter().map(|(k, v)| (k.clone(), Value::from(v.clone()))).collect::<serde_json::Map<_, _>>(),
            "DefaultMassCal": self.defaults.iter().map(|d| json!({
                "DefaultCalibrationID": d.id, "CalibrationFormula": d.formulas, "Values": d.values, "ValueUseFlags": d.flags,
            })).collect::<Vec<_>>(),
            "MSScan": {
                "layout_version": self.header.layout_version,
                "spectrum_blocks_per_record": self.header.block_count,
                "first_record_offset": self.header.first_record,
                "record_layout_from": self.layout_source,
                "fields": self.layout.fields.iter().map(|f| json!({"name": f.name, "offset": f.offset, "bytes": f.ty.size()})).collect::<Vec<_>>(),
                "block_fields": self.layout.block_fields.iter().map(|f| json!({"name": f.name, "offset": f.offset, "bytes": f.ty.size()})).collect::<Vec<_>>(),
                "first_record": first,
            },
            "IMSFrame": self.ims.as_ref().map(|ims| json!({
                "frames": ims.frames.len(),
                "frame_sum_records": ims.frame_sums,
                "first_frame": ims.frames.first().map(|f| json!({
                    "FrameId": f.frame_id, "FrameMethodId": f.method_id, "TimeSegmentId": f.time_segment_id,
                    "CycleNumber": f.cycle_number, "FragClass": f.frag_class, "FrameScanTime": f.scan_time_min,
                    "FrameTic": f.tic, "FrameBaseAbund": f.base_abundance, "MassCalOffset": f.mass_cal_offset,
                    "ImsField": f.ims_field, "ImsPressure": f.ims_pressure, "ImsTemperature": f.ims_temperature,
                })),
                "methods": ims.methods.iter().map(|m| json!({
                    "FrameMethId": m.id, "FrameDtPeriod": m.drift_period_ms, "FrameMsXPeriod": m.ms_period_ns,
                    "MinMsBin": m.min_ms_bin, "DefMassCalId": m.mass_cal_id, "IonPolarity": m.ion_polarity,
                    "IonizationMode": m.ionization_mode, "FragOpMode": m.frag_op_mode, "FragEnergy": m.frag_energy,
                    "FragEnergySegments": m.frag_energy_ramp.iter().map(|(db, e)| json!({"db": db, "e": e})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })),
            "signals": self.signals.iter().map(|s| json!({
                "file": format!("{}.cd", s.file_stem), "device_id": s.device_id, "id": s.def.id,
                "description": s.def.description, "kind": s.def.kind, "offset": s.def.offset,
                "count": s.def.count, "unit": s.def.unit, "scale": s.def.scale,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].ms_levels",
            "spectra[0].rt_range_s",
            "traces[]",
        ] {
            m.insert(k.into(), Source::Inferred);
        }
        for k in [
            "spectra[0].instrument.model",
            "spectra[0].instrument.software_version",
            "spectra[0].extra.acquired_at",
            "spectra[0].extra.instrument_serial",
            "spectra[0].extra.sample_name",
            "spectra[0].extra.method",
        ] {
            m.insert(k.into(), Source::Inferred);
        }
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut files = Vec::new();
        walk(&self.fs, &self.dir, 0, &mut files);
        let mut out: Vec<LsEntry> = files
            .into_iter()
            .map(|(p, n)| {
                let rel = p
                    .strip_prefix(&self.dir)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/");
                let role = match p
                    .file_name()
                    .and_then(|f| f.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("msscan.bin") => "scan index",
                    Some("mspeak.bin") => "centroid spectra",
                    Some("msprofile.bin") => "profile spectra",
                    Some("msmasscal.bin") => "per-scan calibration",
                    Some("defaultmasscal.xml") => "default calibration",
                    Some("msts.xml") => "time segments",
                    Some("contents.xml") => "acquisition summary",
                    Some("devices.xml") => "device list",
                    Some("sample_info.xml") => "sample information",
                    Some("acqmethod.xml") => "acquisition method",
                    Some("imsframe.bin") => "ion-mobility frame index",
                    Some("imsframe.xsd") => "frame record schema",
                    Some("imsframemeth.xml") => "ion-mobility frame methods",
                    Some(f) if Path::new(f).extension().is_some_and(|e| e == "cd") => {
                        "signal descriptor"
                    }
                    Some(f) if Path::new(f).extension().is_some_and(|e| e == "cg") => "signal data",
                    _ => "file",
                };
                LsEntry {
                    kind: "file".into(),
                    name: rel,
                    offset: None,
                    size: Some(n),
                    image: None,
                    details: json!({ "role": role }),
                }
            })
            .collect();
        out.push(LsEntry {
            kind: "run".into(),
            name: "MS run".into(),
            offset: None,
            size: None,
            image: None,
            details: json!({
                "scans": self.records.len(),
                "record_bytes": self.layout.record_len(self.header.block_count as usize),
                "spectrum_blocks_per_record": self.header.block_count,
                "ms_levels": self.ms_levels(),
            }),
        });
        Ok(out)
    }

    fn read_plane(
        &mut self,
        _image: u32,
        _index: openreadout_core::PlaneIndex,
    ) -> Result<openreadout_core::Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "images",
            "MassHunter data holds spectra and signals; use `scans`, `spectrum` or `trace`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.dir.display().to_string(), FORMAT_ID);
        let rec_len = self.layout.record_len(self.header.block_count as usize);
        rep.performed(format!(
            "MSScan.bin: header, {} records of {rec_len} bytes (layout from {})",
            self.records.len(),
            self.layout_source
        ));
        if self.trailing_bytes != 0 {
            rep.push(
                Finding::error(
                    "truncated",
                    format!(
                        "MSScan.bin ends {} bytes into record {} (expected a multiple of {rec_len} bytes after the header); the scan index was cut off",
                        self.trailing_bytes,
                        self.records.len()
                    ),
                )
                .at(self.scan_file_len - self.trailing_bytes),
            );
        }
        if let Some(ts) = &self.msts {
            let mut declared = 0u64;
            let mut rest = ts.as_str();
            while let Some(v) = tag_text(rest, "NumOfScans") {
                declared += v.parse::<u64>().unwrap_or(0);
                let i = rest.find("</NumOfScans>").map_or(rest.len(), |i| i + 13);
                rest = &rest[i..];
            }
            // Ion-mobility data counts frames there.
            let (held, what) = match &self.ims {
                Some(ims) => (ims.frames.len() as u64, "IMSFrame.bin holds"),
                None => (self.records.len() as u64, "MSScan.bin holds"),
            };
            rep.performed("MSTS.xml: scan count per time segment against the index");
            if declared != held && declared != 0 {
                rep.push(Finding::error(
                    "scan_count_mismatch",
                    format!("MSTS.xml declares {declared} scans; {what} {held}"),
                ));
            }
        }
        let len_of = |p: &Option<PathBuf>| {
            p.as_ref()
                .and_then(|p| self.fs.metadata(p).ok())
                .map(|m| m.len())
        };
        let (peak_len, prof_len) = (len_of(&self.peak_path), len_of(&self.profile_path));
        let mut bad = 0usize;
        for r in &self.records {
            for b in &r.blocks {
                if b.byte_count <= 0 {
                    continue;
                }
                let (file, len) = if b.format_id == FORMAT_PROFILE && !self.is_gcms_points(b) {
                    ("MSProfile.bin", prof_len)
                } else {
                    ("MSPeak.bin", peak_len)
                };
                let end = u64::try_from(b.offset)
                    .ok()
                    .and_then(|o| o.checked_add(b.byte_count as u64));
                let ok = matches!((end, len), (Some(e), Some(l)) if e <= l);
                if !ok {
                    bad += 1;
                    if bad <= 5 {
                        let msg = match len {
                            Some(l) => format!(
                                "scan {}: {file} block of {} bytes at {} runs past the end of the file ({l} bytes)",
                                r.scan_id, b.byte_count, b.offset
                            ),
                            None => format!(
                                "scan {}: its {file} block of {} bytes cannot be read: the data directory holds no AcqData/{file}",
                                r.scan_id, b.byte_count
                            ),
                        };
                        rep.push(Finding::error("truncated", msg));
                    }
                }
            }
        }
        rep.performed(
            "every spectrum block's offset and byte count against MSPeak.bin / MSProfile.bin",
        );
        if bad > 5 {
            rep.push(Finding::error(
                "truncated",
                format!("{bad} spectrum blocks in all lie outside their files"),
            ));
        }
        if let Some(mc) = &self.mass_cal {
            let missing = self
                .records
                .iter()
                .filter(|r| {
                    r.mass_cal_offset.is_some_and(|o| {
                        o >= DATA_START as i64 && mass_cal_values(mc, o as usize).is_none()
                    })
                })
                .count();
            rep.performed("MSMassCal.bin: every scan's calibration record is inside the file");
            if missing > 0 {
                rep.push(Finding::error(
                    "truncated",
                    format!("{missing} scans point past the end of MSMassCal.bin"),
                ));
            }
        }
        // Decode the first and last spectrum and compare with the index's summaries.
        if bad == 0 && !self.records.is_empty() {
            let last = self.records.len() as u64 - 1;
            for i in [0, last] {
                let r = &self.records[i as usize];
                match self.spectrum_of(i, SpectrumView::Primary) {
                    Ok(sp) => {
                        let sum: f64 = sp.intensity.iter().map(|&v| f64::from(v)).sum();
                        if !sp.centroided && (sum - r.tic).abs() > 1e-6 * r.tic.abs().max(1.0) {
                            rep.push(Finding::warning(
                                "tic_mismatch",
                                format!(
                                    "scan {}: profile sums to {sum}, the index records TIC {}",
                                    r.scan_id, r.tic
                                ),
                            ));
                        }
                    }
                    Err(e) => rep.push(Finding::error(
                        "undecodable",
                        format!("scan {}: {e}", r.scan_id),
                    )),
                }
            }
            rep.performed(
                "decoded the first and last spectrum (profiles: counts sum to the recorded TIC)",
            );
        }
        for s in &self.signals {
            let ok = self
                .fs
                .read(&s.cg)
                .is_ok_and(|cg| signal_block(&cg, &s.def).is_some());
            if !ok {
                rep.push(Finding::error(
                    "truncated",
                    format!(
                        "{}.cg: signal {} ({} samples at {}) is cut off",
                        s.file_stem, s.def.id, s.def.count, s.def.offset
                    ),
                ));
            }
        }
        if !self.signals.is_empty() {
            rep.performed("device signals (*.cd/*.cg): every block inside its data file");
        }
        Ok(rep)
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let n_ms = if self.records.is_empty() { 0 } else { 2 };
        if sweep != 0 || index as usize >= n_ms + self.signals.len() {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} does not exist ({} traces, one sweep each)",
                n_ms + self.signals.len()
            )));
        }
        let start = usize::try_from(first_sample).unwrap_or(usize::MAX);
        let take = usize::try_from(max_samples).unwrap_or(usize::MAX);
        let channels = if (index as usize) < n_ms {
            let rows: Vec<(f64, f64, f64)> = self
                .chromatogram_rows()
                .into_iter()
                .skip(start)
                .take(take)
                .collect();
            vec![
                rows.iter().map(|r| r.0 * 60.0).collect(),
                rows.iter()
                    .map(|r| if index == 0 { r.1 } else { r.2 })
                    .collect(),
            ]
        } else {
            let s = &self.signals[index as usize - n_ms];
            let cg = read_all(&self.fs, &s.cg)?;
            let (t0, dt, v) = signal_block(&cg, &s.def).ok_or_else(|| {
                Error::corrupt(
                    FORMAT_ID,
                    format!("{}.cg: signal {} is cut off", s.file_stem, s.def.id),
                )
            })?;
            let idx: Vec<usize> = (0..v.len()).skip(start).take(take).collect();
            vec![
                idx.iter().map(|&i| (t0 + dt * i as f64) * 60.0).collect(),
                idx.iter()
                    .map(|&i| v[i] * if s.def.scale == 0.0 { 1.0 } else { s.def.scale })
                    .collect(),
            ]
        };
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels,
        })
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.read_spectrum_view(index, spectrum, SpectrumView::Primary)
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} out of range (a MassHunter .d holds one run)"
            )));
        }
        for i in first..self.records.len() as u64 {
            if !visit(self.header_of(i)?) {
                break;
            }
        }
        Ok(true)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        Ok((run == 0).then(|| {
            self.records
                .iter()
                .map(|r| u32::try_from(r.ms_level).unwrap_or(0))
                .collect()
        }))
    }

    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} does not exist (MassHunter data has one run, 0)"
            )));
        }
        self.spectrum_of(spectrum, view)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run != 0 {
            return Ok(None);
        }
        Ok(Some(
            self.records
                .iter()
                .position(|r| u64::try_from(r.scan_id).ok() == Some(scan_number))
                .map(|i| i as u64)
                .ok_or_else(|| Error::Usage(format!("no scan with scan id {scan_number}")))?,
        ))
    }
}

//! Chromeleon 7 sequence files (`.cmd`) beyond the signal descriptions: the object graph
//! (catalog records 18, data records 19), injection details, processing-method components,
//! instrument-method scripts, Chromeleon's own stored integration results (`ChmData.` blobs)
//! and the compressed blobs (`D?AC`, `Cp??`: LZMA2) that hold methods and audit trails.
//! Layout and evidence: `docs/formats/chromeleon.md`, `docs/provenance/chromeleon.md`.

use std::fmt::Write as _;

use openreadout_codecs::lzma2_decode;
use openreadout_core::bytes::{le_f64, le_u16, le_u32};

use crate::chromeleon::{PbValue, SignalError, pb_bytes, pb_f64, pb_fields, pb_get, pb_text};

/// Largest decompressed blob accepted (an instrument method's symbol table is about 1.4 MB).
pub const MAX_BLOB_BYTES: usize = 256 << 20;

/// A 16-byte object identifier (a .NET GUID's bytes, as protocol-buffers fields 1 and 2 of two
/// little-endian fixed64 values).
pub type ObjectId = [u8; 16];

/// Does `b` start like a compressed blob (`D?AC` or `Cp??` magic)?
pub fn is_blob(b: &[u8]) -> bool {
    b.len() > 9 && ((b[0] == b'D' && &b[2..4] == b"AC") || &b[..2] == b"Cp")
}

/// The 4-byte magic of a blob (`DIAC`, `DdAC`, `CpXm`, `CpAu`).
pub fn blob_magic(b: &[u8]) -> String {
    String::from_utf8_lossy(b.get(..4).unwrap_or_default()).into_owned()
}

/// The decompressed size a blob's header states, when it states one.
pub fn blob_stated_size(b: &[u8]) -> Option<usize> {
    let hl = usize::from(le_u16(b, 4)?);
    if hl < 16 {
        return None;
    }
    Some(le_u32(b, 12)? as usize)
}

/// Decompress a `D?AC`/`Cp??` blob: magic (4), u16 header length, u16 version, then (headers
/// of 16 bytes or more) u32 compressed length and u32 decompressed length; after the header an
/// LZMA2 dictionary-size byte and a raw LZMA2 stream.
///
/// # Errors
/// Not a blob, a header that does not fit, a dictionary byte out of range, a stated size above
/// [`MAX_BLOB_BYTES`], or an LZMA2 stream that does not decode to the stated size.
pub fn unpack_blob(b: &[u8]) -> Result<Vec<u8>, String> {
    if !is_blob(b) {
        return Err("not a compressed blob (no D?AC or Cp?? magic)".into());
    }
    let magic = blob_magic(b);
    let hl = usize::from(u16::from_le_bytes([b[4], b[5]]));
    if hl < 8 || hl >= b.len() {
        return Err(format!("{magic}: header length {hl} does not fit"));
    }
    let mut end = b.len();
    let expected = if hl >= 16 {
        let packed = u32::from_le_bytes(b[8..12].try_into().map_err(|_| "header")?) as usize;
        end = hl
            .checked_add(packed)
            .filter(|&e| e <= b.len())
            .ok_or_else(|| format!("{magic}: states {packed} compressed bytes, fewer remain"))?;
        blob_stated_size(b).unwrap_or(0)
    } else {
        0
    };
    if expected > MAX_BLOB_BYTES {
        return Err(format!(
            "{magic}: states {expected} decompressed bytes (more than {MAX_BLOB_BYTES})"
        ));
    }
    let dict = b[hl];
    if dict > 40 {
        return Err(format!("{magic}: dictionary byte {dict} out of range"));
    }
    let (out, _) = lzma2_decode(&b[hl + 1..end], expected).map_err(|e| format!("{magic}: {e}"))?;
    if out.len() > MAX_BLOB_BYTES {
        return Err(format!(
            "{magic}: decompresses to more than {MAX_BLOB_BYTES} bytes"
        ));
    }
    Ok(out)
}

fn object_id(b: &[u8]) -> Option<ObjectId> {
    let f = pb_fields(b)?;
    let (Some(PbValue::Fixed64(lo)), Some(PbValue::Fixed64(hi))) = (pb_get(&f, 1), pb_get(&f, 2))
    else {
        return None;
    };
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&lo.to_le_bytes());
    id[8..].copy_from_slice(&hi.to_le_bytes());
    Some(id)
}

/// Text of an enumeration value: a message whose field 3 is the name (`Unknown`, `Finished`).
fn enum_text(b: &[u8]) -> Option<String> {
    pb_fields(b).and_then(|f| pb_text(&f, 3))
}

/// One object of the sequence file: its catalog record (18) joined with its data record (19).
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceObject {
    /// Type name from the catalog (`Injection`, `Signal`, `Chromatogram`, ...).
    pub kind: String,
    /// The object's id.
    pub id: ObjectId,
    /// The parent object's id.
    pub parent: Option<ObjectId>,
    /// The object's number in the data vault (the `662` of `662.smp`).
    pub number: Option<u64>,
    /// Name from the data record (field 28).
    pub name: Option<String>,
}

/// Details of one injection (data record field 19).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InjectionDetails {
    /// The injection object's id.
    pub id: Option<ObjectId>,
    /// Name.
    pub name: String,
    /// Injection type (`Unknown`, `Standard`, `Blank`, ...).
    pub injection_type: Option<String>,
    /// Status (`Finished`, `Interrupted`, ...).
    pub status: Option<String>,
    /// Autosampler position (`GA8`, `1`).
    pub position: Option<String>,
    /// Injection volume (µL, as Chromeleon's exports label it).
    pub volume_ul: Option<f64>,
    /// Inject time as stored (ISO 8601 with the local offset).
    pub inject_time: Option<String>,
    /// Calibration level name (standards).
    pub level: Option<String>,
    /// The three per-injection factors of fields 9, 10 and 11 (dilution factor and weight among
    /// them; which is which is not established).
    pub factors: [Option<f64>; 3],
    /// Processing method name.
    pub processing_method: Option<String>,
    /// Instrument method name.
    pub instrument_method: Option<String>,
    /// The object's number in the data vault.
    pub number: Option<u64>,
}

/// A processing-method component (a named peak with an expected retention time).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Component {
    /// Processing method name.
    pub method: String,
    /// Component name.
    pub name: String,
    /// The key stored peaks use to name their component.
    pub key: Option<ObjectId>,
    /// Expected retention time, minutes.
    pub retention_min: Option<f64>,
    /// The component's second value that identified peaks repeat (field 14, a time in
    /// minutes; its meaning is not established).
    pub second_value: Option<f64>,
    /// Amount unit (`mM`, `ppm`, or empty).
    pub amount_unit: Option<String>,
    /// Calibration levels: level key and amount.
    pub levels: Vec<(String, f64)>,
}

/// One step of an instrument method's script.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodStep {
    /// Stage (`InstrumentSetup`, `Inject`, `StartRun`, `Run`, `StopRun`, `PostRun`, ...).
    pub stage: String,
    /// Time in minutes (`None`: the stage's initial step, stored as −∞).
    pub time_min: Option<f64>,
    /// `property` (an assignment) or `command`.
    pub kind: String,
    /// Symbol (`Pump_1.Flow`, `GC.Oven.Temperature.Nominal`).
    pub symbol: String,
    /// Value as written, with its unit (`0.250 [ml/min]`).
    pub value: String,
}

/// An instrument method: name, compressed script and its steps when decoded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InstrumentMethod {
    /// Name.
    pub name: String,
    /// The compressed `CpXm` blob.
    pub blob: Vec<u8>,
}

/// A stored integration: a point is (time min, value).
pub type Point = (f64, f64);

/// One side of a stored peak.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PeakFlank {
    /// The point at 10 % of the height above the baseline.
    pub at_10: Option<Point>,
    /// At 50 %.
    pub at_50: Option<Point>,
    /// At 5 %.
    pub at_5: Option<Point>,
    /// A point on the flank where a tangent is taken.
    pub tangent_point: Option<Point>,
    /// The tangent's slope (value per minute).
    pub tangent_slope: Option<f64>,
}

/// A straight skim line a rider peak is integrated above.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkimLine {
    /// Type byte (0: straight line; others are refused).
    pub kind: u8,
    /// First point.
    pub start: Point,
    /// Slope at the first point.
    pub start_slope: f64,
    /// Last point.
    pub end: Point,
    /// Slope at the last point.
    pub end_slope: f64,
}

/// One peak as Chromeleon stored it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPeak {
    /// Integration start (time, signal value).
    pub start: Point,
    /// Integration end.
    pub end: Point,
    /// A code word (bit meanings not established).
    pub code: u32,
    /// Apex (time, signal value).
    pub apex: Point,
    /// Leading side.
    pub leading: PeakFlank,
    /// Trailing side.
    pub trailing: PeakFlank,
    /// Retention time, minutes.
    pub retention_min: f64,
    /// Area (signal unit × min).
    pub area: f64,
    /// Height above the baseline at the apex (signal unit).
    pub height: f64,
    /// The key of the processing-method component the peak was identified as.
    pub component_key: Option<ObjectId>,
    /// The component's (expected retention time, second value) as stored with the peak.
    pub component_values: Option<(f64, f64)>,
    /// 1-based index of the skim line the peak rides on (0: none).
    pub skim: u8,
}

/// The stored integration results of one chromatogram.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StoredResults {
    /// Blob version byte (1: Chromeleon 7.2, 2: 7.3).
    pub version: u8,
    /// Skim lines.
    pub skims: Vec<SkimLine>,
    /// Peaks in stored order.
    pub peaks: Vec<StoredPeak>,
    /// Baseline segments, each a polyline.
    pub baselines: Vec<Vec<Point>>,
}

impl StoredResults {
    /// The baseline under `peak` as a function of time, when it is one straight line over the
    /// peak's span: the skim line it rides on, else the baseline segment containing its limits
    /// (piecewise linear; a vertex inside the peak's span returns `None`).
    pub fn baseline_line(&self, peak: &StoredPeak) -> Option<(Point, Point)> {
        let (t0, t1) = (peak.start.0, peak.end.0);
        let eps = 1e-9 * t1.abs().max(1.0);
        if peak.skim > 0 {
            let s = self.skims.get(usize::from(peak.skim) - 1)?;
            let at = |t: f64| s.start.1 + s.start_slope * (t - s.start.0);
            return Some(((t0, at(t0)), (t1, at(t1))));
        }
        let seg = self.baselines.iter().find(|s| {
            s.first().is_some_and(|p| p.0 <= t0 + eps) && s.last().is_some_and(|p| p.0 >= t1 - eps)
        })?;
        if seg.iter().any(|p| p.0 > t0 + eps && p.0 < t1 - eps) {
            return None;
        }
        let interp = |t: f64| -> Option<f64> {
            seg.windows(2).find_map(|w| {
                let ((x0, y0), (x1, y1)) = (w[0], w[1]);
                (x0 - eps <= t && t <= x1 + eps).then(|| {
                    if x1 > x0 {
                        y0 + (y1 - y0) * (t - x0) / (x1 - x0)
                    } else {
                        y0
                    }
                })
            })
        };
        Some(((t0, interp(t0)?), (t1, interp(t1)?)))
    }
}

/// Reader over a `ChmData.` peak section.
struct Cursor<'a> {
    b: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn corrupt(&self, what: &str) -> SignalError {
        SignalError::Corrupt(format!("stored results: {what} at byte {}", self.at))
    }
    fn u8(&mut self) -> Result<u8, SignalError> {
        let v = *self
            .b
            .get(self.at)
            .ok_or_else(|| self.corrupt("cut short"))?;
        self.at += 1;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, SignalError> {
        let v = le_u32(self.b, self.at).ok_or_else(|| self.corrupt("cut short"))?;
        self.at += 4;
        Ok(v)
    }
    fn f64(&mut self) -> Result<f64, SignalError> {
        let x = le_f64(self.b, self.at).ok_or_else(|| self.corrupt("cut short"))?;
        self.at += 8;
        if x.is_finite() {
            Ok(x)
        } else {
            Err(self.corrupt("a non-finite number"))
        }
    }
    fn point(&mut self) -> Result<Point, SignalError> {
        Ok((self.f64()?, self.f64()?))
    }
    /// 7-bit variable-length count.
    fn count(&mut self) -> Result<usize, SignalError> {
        let mut v = 0usize;
        for shift in [0u32, 7, 14, 21] {
            let x = self.u8()?;
            v |= usize::from(x & 0x7F) << shift;
            if x & 0x80 == 0 {
                // at least 20 bytes per item: a count the remaining bytes cannot hold is corrupt
                if v > self.b.len() {
                    return Err(self.corrupt("a count larger than the section"));
                }
                return Ok(v);
            }
        }
        Err(self.corrupt("a count longer than 4 bytes"))
    }
    fn present(&mut self) -> Result<bool, SignalError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            x => Err(self.corrupt(&format!("presence byte {x}"))),
        }
    }
    fn opt_point(&mut self) -> Result<Option<Point>, SignalError> {
        if self.present()? {
            Ok(Some(self.point()?))
        } else {
            Ok(None)
        }
    }
    fn absent(&mut self, what: &str) -> Result<(), SignalError> {
        if self.present()? {
            return Err(SignalError::Unsupported(format!(
                "stored results: a value never seen before ({what}) at byte {}",
                self.at - 1
            )));
        }
        Ok(())
    }
    fn flank(&mut self) -> Result<PeakFlank, SignalError> {
        self.absent("first flank value")?;
        let at_10 = self.opt_point()?;
        let at_50 = self.opt_point()?;
        let at_5 = self.opt_point()?;
        self.absent("fifth flank value")?;
        let tangent_point = self.opt_point()?;
        let tangent_slope = if self.present()? {
            Some(self.f64()?)
        } else {
            None
        };
        self.absent("last flank value")?;
        Ok(PeakFlank {
            at_10,
            at_50,
            at_5,
            tangent_point,
            tangent_slope,
        })
    }
}

/// The `ChmData.` blob header: magic, the 16 bytes seen, three length-prefixed values.
const CHM_MAGIC: &[u8; 8] = b"ChmData.";

/// Parse a chromatogram's stored results blob.
///
/// # Errors
/// [`SignalError::Corrupt`] for a truncated or inconsistent blob, [`SignalError::Unsupported`]
/// for a header version, skim-line type or optional value never seen.
pub fn parse_stored_results(b: &[u8]) -> Result<Option<StoredResults>, SignalError> {
    let corrupt = |m: &str| SignalError::Corrupt(format!("stored results: {m}"));
    if b.get(..8) != Some(CHM_MAGIC) {
        return Err(corrupt("no ChmData. magic"));
    }
    let head = b.get(8..24).ok_or_else(|| corrupt("header cut short"))?;
    let version = head[1];
    if head[0] != 1
        || !matches!(version, 1 | 2)
        || head[2..4] != [0, 0]
        || head[4..] != [1, 0, 0, 0, 0x67, 0, 0, 0, 1, 0, 0, 0]
    {
        return Err(SignalError::Unsupported(format!(
            "stored results header {} (only the two layouts of Chromeleon 7.2 and 7.3 are decoded)",
            head.iter().fold(String::new(), |mut s, x| {
                let _ = write!(s, "{x:02x}");
                s
            })
        )));
    }
    let mut at = 24usize;
    for _ in 0..3 {
        let n = usize::from(*b.get(at).ok_or_else(|| corrupt("header cut short"))?);
        at = at
            .checked_add(1 + n)
            .filter(|&e| e <= b.len())
            .ok_or_else(|| corrupt("header cut short"))?;
    }
    let mut peaks_section = None;
    while at < b.len() {
        let hdr = b
            .get(at..at + 8)
            .ok_or_else(|| corrupt("section header cut short"))?;
        let tag = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
        let body = at
            .checked_add(8)
            .and_then(|s| Some((s, s.checked_add(len)?)))
            .filter(|&(_, e)| e <= b.len())
            .map(|(s, e)| &b[s..e])
            .ok_or_else(|| corrupt("a section runs past the end"))?;
        if tag == 2 {
            peaks_section = Some(body);
        }
        at += 8 + len;
    }
    let Some(body) = peaks_section else {
        return Ok(None);
    };
    let mut c = Cursor { b: body, at: 0 };
    if body.get(..8).is_none_or(|z| z.iter().any(|&x| x != 0)) {
        return Err(SignalError::Unsupported(
            "stored results: the peak section does not start with the 8 zero bytes seen".into(),
        ));
    }
    c.at = 8;
    let mut out = StoredResults {
        version,
        ..StoredResults::default()
    };
    for _ in 0..c.count()? {
        let kind = c.u8()?;
        let (start, start_slope, end, end_slope) = (c.point()?, c.f64()?, c.point()?, c.f64()?);
        out.skims.push(SkimLine {
            kind,
            start,
            start_slope,
            end,
            end_slope,
        });
    }
    for _ in 0..c.count()? {
        let start = c.point()?;
        let end = c.point()?;
        let code = c.u32()?;
        let apex = c.point()?;
        let leading = c.flank()?;
        let trailing = c.flank()?;
        let (retention_min, area, height) = (c.f64()?, c.f64()?, c.f64()?);
        let (mut component_key, mut component_values) = (None, None);
        if c.present()? {
            if c.present()? {
                let k =
                    c.b.get(c.at..c.at + 16)
                        .ok_or_else(|| c.corrupt("cut short"))?;
                let mut id = [0u8; 16];
                id.copy_from_slice(k);
                c.at += 16;
                component_key = Some(id);
            }
            if c.present()? {
                component_values = Some((c.f64()?, c.f64()?));
            }
        }
        let skim = c.u8()?;
        out.peaks.push(StoredPeak {
            start,
            end,
            code,
            apex,
            leading,
            trailing,
            retention_min,
            area,
            height,
            component_key,
            component_values,
            skim,
        });
    }
    for _ in 0..c.count()? {
        let n = c.count()?;
        let mut seg = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            seg.push(c.point()?);
        }
        out.baselines.push(seg);
    }
    if c.at != body.len() {
        return Err(corrupt(&format!(
            "{} bytes after the last baseline segment",
            body.len() - c.at
        )));
    }
    for (i, s) in out.skims.iter().enumerate() {
        if s.kind != 0 || (s.start_slope - s.end_slope).abs() > 1e-12 * s.start_slope.abs().max(1.0)
        {
            return Err(SignalError::Unsupported(format!(
                "stored results: skim line {} of type {} with slopes {} and {} (only straight lines are decoded)",
                i + 1,
                s.kind,
                s.start_slope,
                s.end_slope
            )));
        }
    }
    for (i, p) in out.peaks.iter().enumerate() {
        if usize::from(p.skim) > out.skims.len() {
            return Err(corrupt(&format!(
                "peak {} names skim line {}",
                i + 1,
                p.skim
            )));
        }
        if p.end.0.is_nan() || p.start.0.is_nan() || p.end.0 < p.start.0 {
            return Err(corrupt(&format!("peak {} ends before it starts", i + 1)));
        }
    }
    Ok(Some(out))
}

/// A chromatogram's stored results, with the signal it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct ChromatogramRecord {
    /// The chromatogram's name (the signal name).
    pub name: Option<String>,
    /// The `RawDataFileId` of its signal.
    pub signal_file_id: Option<String>,
    /// The injection it belongs to.
    pub injection: Option<ObjectId>,
    /// The stored results: `None` when Chromeleon saved none; `Err` when not decoded.
    pub results: Result<Option<StoredResults>, SignalError>,
}

/// Everything read from a sequence file beyond signal descriptions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SequenceContents {
    /// Every object in catalog order.
    pub objects: Vec<SequenceObject>,
    /// Injections in sequence order.
    pub injections: Vec<InjectionDetails>,
    /// Chromatograms with stored results.
    pub chromatograms: Vec<ChromatogramRecord>,
    /// Components of every processing method.
    pub components: Vec<Component>,
    /// Instrument methods.
    pub instrument_methods: Vec<InstrumentMethod>,
    /// Processing methods: name and compressed settings blob (`DdAC`), when present.
    pub processing_methods: Vec<(String, Option<Vec<u8>>)>,
    /// Things not decoded (one line each).
    pub notes: Vec<String>,
}

fn find_blob(b: &[u8], depth: u32) -> Option<&[u8]> {
    if is_blob(b) {
        return Some(b);
    }
    if depth == 0 {
        return None;
    }
    let f = pb_fields(b)?;
    f.iter().find_map(|(_, v)| match v {
        PbValue::Bytes(x) => find_blob(x, depth - 1),
        _ => None,
    })
}

fn parse_injection(rec: &[(u64, PbValue<'_>)], obj: &SequenceObject) -> InjectionDetails {
    let mut d = InjectionDetails {
        id: Some(obj.id),
        name: obj.name.clone().unwrap_or_default(),
        number: obj.number,
        ..InjectionDetails::default()
    };
    if let Some(f) = pb_bytes(rec, 19).and_then(pb_fields) {
        let text = |n| pb_text(&f, n).filter(|s| !s.is_empty());
        d.level = text(2);
        d.injection_type = pb_bytes(&f, 3).and_then(enum_text);
        d.status = pb_bytes(&f, 4).and_then(enum_text);
        d.position = text(5);
        d.volume_ul = pb_f64(&f, 6);
        d.inject_time = pb_bytes(&f, 7)
            .and_then(pb_fields)
            .and_then(|t| pb_text(&t, 1))
            .filter(|s| !s.is_empty());
        d.factors = [pb_f64(&f, 9), pb_f64(&f, 10), pb_f64(&f, 11)];
    }
    for (n, v) in rec {
        let (40, PbValue::Bytes(link)) = (*n, *v) else {
            continue;
        };
        let Some(l) = pb_fields(link) else { continue };
        let which = match pb_get(&l, 1) {
            Some(PbValue::Varint(k)) => k,
            _ => 0,
        };
        let name = pb_bytes(&l, 2)
            .and_then(pb_fields)
            .and_then(|x| pb_text(&x, 5));
        match which {
            0 => d.processing_method = name,
            1 => d.instrument_method = name,
            _ => {}
        }
    }
    d
}

fn parse_components(
    method: &str,
    rec: &[(u64, PbValue<'_>)],
    catalog: &[(u64, PbValue<'_>)],
) -> Vec<Component> {
    // the catalog's `Component` entries (field 17): 1 = the component's id, 4 = its key
    let mut keys: Vec<(ObjectId, ObjectId)> = Vec::new();
    for (n, v) in catalog {
        let (17, PbValue::Bytes(e)) = (*n, *v) else {
            continue;
        };
        let Some(e) = pb_fields(e) else { continue };
        if let (Some(id), Some(key)) = (
            pb_bytes(&e, 1).and_then(object_id),
            pb_bytes(&e, 4).and_then(object_id),
        ) {
            keys.push((id, key));
        }
    }
    let Some(list) = pb_bytes(rec, 43).and_then(pb_fields) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (n, v) in &list {
        let (2, PbValue::Bytes(c)) = (*n, *v) else {
            continue;
        };
        let Some(c) = pb_fields(c) else { continue };
        let id = pb_bytes(&c, 10).and_then(object_id);
        let quant = pb_bytes(&c, 5)
            .and_then(pb_fields)
            .and_then(|q| pb_bytes(&q, 2).and_then(pb_fields));
        let mut levels = Vec::new();
        for (m, lv) in &c {
            let (22, PbValue::Bytes(set)) = (*m, *lv) else {
                continue;
            };
            let Some(set) = pb_fields(set) else { continue };
            if pb_text(&set, 1).as_deref() != Some("Components.ConcentrationLevelCollection") {
                continue;
            }
            for (k, item) in &set {
                let (2, PbValue::Bytes(item)) = (*k, *item) else {
                    continue;
                };
                let Some(item) = pb_fields(item) else {
                    continue;
                };
                if let (Some(key), Some(amount)) = (pb_text(&item, 1), pb_f64(&item, 3)) {
                    levels.push((key, amount));
                }
            }
        }
        out.push(Component {
            method: method.to_string(),
            name: pb_text(&c, 12).unwrap_or_default(),
            key: id.and_then(|id| keys.iter().find(|(i, _)| *i == id).map(|(_, k)| *k)),
            retention_min: quant.as_ref().and_then(|q| pb_f64(q, 9)),
            second_value: quant.as_ref().and_then(|q| pb_f64(q, 14)),
            amount_unit: quant.as_ref().and_then(|q| pb_text(q, 6)),
            levels,
        });
    }
    out
}

/// Parse a sequence file's objects.
pub fn parse_sequence_contents(cmd: &[u8]) -> SequenceContents {
    let mut out = SequenceContents::default();
    let Some(top) = pb_fields(cmd) else {
        out.notes
            .push("the sequence file is not a complete protocol-buffers stream: its objects were not read".into());
        return out;
    };
    let catalog: Vec<&[u8]> = top
        .iter()
        .filter_map(|(f, v)| match (*f, *v) {
            (18, PbValue::Bytes(b)) => Some(b),
            _ => None,
        })
        .collect();
    let records: Vec<&[u8]> = top
        .iter()
        .filter_map(|(f, v)| match (*f, *v) {
            (19, PbValue::Bytes(b)) => Some(b),
            _ => None,
        })
        .collect();
    // data records by object id
    let mut by_id: Vec<(ObjectId, Vec<(u64, PbValue<'_>)>)> = Vec::new();
    for r in &records {
        let Some(f) = pb_fields(r) else { continue };
        if let Some(id) = pb_bytes(&f, 25).and_then(object_id) {
            by_id.push((id, f));
        }
    }
    let record = |id: &ObjectId| {
        by_id
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, f)| f.as_slice())
    };
    let mut cat_fields: Vec<(SequenceObject, Vec<(u64, PbValue<'_>)>)> = Vec::new();
    for c in &catalog {
        let Some(f) = pb_fields(c) else { continue };
        let Some(id) = pb_bytes(&f, 5).and_then(object_id) else {
            continue;
        };
        let kind = pb_bytes(&f, 3).and_then(enum_text).unwrap_or_default();
        let obj = SequenceObject {
            kind,
            id,
            parent: pb_bytes(&f, 6).and_then(object_id),
            number: match pb_get(&f, 8) {
                Some(PbValue::Varint(n)) => Some(n),
                _ => None,
            },
            name: record(&id).and_then(|r| pb_text(r, 28)),
        };
        cat_fields.push((obj, f));
    }
    let find = |id: &ObjectId| cat_fields.iter().find(|(o, _)| o.id == *id).map(|(o, _)| o);
    for (obj, cat) in &cat_fields {
        let rec = record(&obj.id);
        match obj.kind.as_str() {
            "Injection" => {
                if let Some(r) = rec {
                    out.injections.push(parse_injection(r, obj));
                }
            }
            "Chromatogram" => {
                let signal = obj.parent.as_ref().and_then(find);
                let signal_file_id = signal.and_then(|s| {
                    record(&s.id)
                        .and_then(|r| pb_bytes(r, 5))
                        .and_then(pb_fields)
                        .and_then(|f| pb_text(&f, 6))
                });
                out.chromatograms.push(ChromatogramRecord {
                    name: obj.name.clone(),
                    signal_file_id,
                    injection: signal.and_then(|s| s.parent),
                    results: pb_bytes(cat, 16).map_or(Ok(None), parse_stored_results),
                });
            }
            "ProcessingMethod" => {
                let name = obj.name.clone().unwrap_or_default();
                if let Some(r) = rec {
                    out.components.extend(parse_components(&name, r, cat));
                    let blob = pb_bytes(r, 14)
                        .and_then(|b| find_blob(b, 1))
                        .map(<[u8]>::to_vec);
                    out.processing_methods.push((name, blob));
                }
            }
            "InstrumentMethod" => {
                if let Some(blob) = rec
                    .and_then(|r| pb_bytes(r, 11))
                    .and_then(|b| find_blob(b, 1))
                {
                    out.instrument_methods.push(InstrumentMethod {
                        name: obj.name.clone().unwrap_or_default(),
                        blob: blob.to_vec(),
                    });
                }
            }
            _ => {}
        }
        out.objects.push(obj.clone());
    }
    out
}

fn xml_doc(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 20_000_000,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

fn child_value<'a>(n: roxmltree::Node<'a, 'a>, tag: &str) -> Option<&'a str> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.attribute("value"))
}

/// The steps of an instrument method's script (the decompressed `CpXm` XML): stages, time
/// steps, property assignments and commands, in stored order.
///
/// # Errors
/// Not XML.
pub fn method_steps(xml: &str) -> Result<Vec<MethodStep>, String> {
    let doc = xml_doc(xml)?;
    let mut out = Vec::new();
    for stage in doc
        .descendants()
        .filter(|n| n.has_tag_name("Item") && n.attribute("type") == Some("StageNode"))
    {
        let name = child_value(stage, "StageName")
            .unwrap_or_default()
            .to_string();
        for step in stage
            .descendants()
            .filter(|n| n.has_tag_name("Item") && n.attribute("type") == Some("TimeStepNode"))
        {
            let time = step
                .children()
                .find(|c| c.has_tag_name("Time"))
                .and_then(|t| child_value(t, "InternalValue"))
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|t| t.is_finite());
            for item in step.descendants().filter(|n| n.has_tag_name("Item")) {
                let kind = match item.attribute("type") {
                    Some("PropertyStepNode") => "property",
                    Some("CommandStepNode") => "command",
                    _ => continue,
                };
                out.push(MethodStep {
                    stage: name.clone(),
                    time_min: time,
                    kind: kind.into(),
                    symbol: child_value(item, "SymbolPath")
                        .unwrap_or_default()
                        .to_string(),
                    value: child_value(item, "Value").unwrap_or_default().to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// One audit-trail message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuditMessage {
    /// Time (ISO 8601 with offset).
    pub time: Option<String>,
    /// Level (`Message`, `Normal`, `Advanced`, `Service`, `Warning`, `Error`, ...).
    pub level: Option<String>,
    /// Category (`Information`, `InjectionStart`, `PreconditionLog`, ...).
    pub category: Option<String>,
    /// Message text.
    pub message: String,
    /// Device, for device-property messages.
    pub device: Option<String>,
    /// Property name.
    pub property: Option<String>,
    /// Property value.
    pub value: Option<String>,
    /// Property unit.
    pub unit: Option<String>,
}

/// The messages of a decompressed audit trail (`CpAu`).
///
/// # Errors
/// Not XML.
pub fn audit_messages(xml: &str) -> Result<Vec<AuditMessage>, String> {
    // a trail appended to over time holds several `<CmData>` documents one after the other
    let body = xml.trim_start_matches('\u{feff}');
    let body = match body.strip_prefix("<?xml") {
        Some(rest) => rest.split_once("?>").map_or(rest, |(_, b)| b),
        None => body,
    };
    let wrapped = format!("<AuditTrail>{body}</AuditTrail>");
    let doc = xml_doc(&wrapped)?;
    Ok(doc
        .descendants()
        .filter(|n| n.has_tag_name("Item") && n.attribute("type") == Some("AuditTrailMessage"))
        .map(|n| {
            let v = |t: &str| child_value(n, t).map(str::to_string);
            AuditMessage {
                time: v("Time"),
                level: v("Level"),
                category: v("Category"),
                message: v("Message").unwrap_or_default(),
                device: v("Device"),
                property: v("PropertyName"),
                value: v("PropertyValue"),
                unit: v("PropertyUnit"),
            }
        })
        .collect())
}

/// Our integration of a stored peak on the decoded signal, between the peak's own limits and
/// under its own baseline (the skim line it rides on, else its baseline segment): the
/// trapezoidal area of (signal − baseline), minus the areas of the rider peaks inside a main
/// peak, and the height signal − baseline at the stored apex, both as magnitudes (Chromeleon
/// stores a peak below its baseline with positive area and height). `values` and `times`
/// (minutes) are the signal's points. `None` when a limit is not on a point's time or the
/// baseline is not one straight line over the peak.
pub fn integrate_stored_peak(
    values: &[f64],
    times: &[f64],
    results: &StoredResults,
    index: usize,
) -> Option<(f64, f64)> {
    let idx = |t: f64| -> Option<usize> {
        let j = times.partition_point(|&x| x < t);
        let near = [j.checked_sub(1), Some(j)]
            .into_iter()
            .flatten()
            .filter(|&k| k < times.len())
            .min_by(|&a, &b| (times[a] - t).abs().total_cmp(&(times[b] - t).abs()))?;
        let step = match (near.checked_sub(1), times.get(near + 1)) {
            (Some(a), _) => times[near] - times[a],
            (None, Some(&b)) => b - times[near],
            _ => 1.0,
        };
        ((times[near] - t).abs() <= 1e-4 * step.abs()).then_some(near)
    };
    let own = |p: &StoredPeak| -> Option<(f64, f64)> {
        let ((t0, b0), (t1, b1)) = results.baseline_line(p)?;
        let (i0, i1, ia) = (idx(t0)?, idx(t1)?, idx(p.apex.0)?);
        if i1 < i0 || ia < i0 || ia > i1 || i1 >= values.len() {
            return None;
        }
        let base = |i: usize| {
            let t = times[i];
            if t1 > t0 {
                b0 + (b1 - b0) * (t - t0) / (t1 - t0)
            } else {
                b0
            }
        };
        let mut area = 0.0;
        for i in i0..i1 {
            area += f64::midpoint(values[i] - base(i), values[i + 1] - base(i + 1))
                * (times[i + 1] - times[i]);
        }
        Some((area, values[ia] - base(ia)))
    };
    let p = results.peaks.get(index)?;
    let (mut area, height) = own(p)?;
    if p.skim == 0 {
        let eps = 1e-9 * p.end.0.abs().max(1.0);
        for q in &results.peaks {
            if q.skim > 0 && q.start.0 >= p.start.0 - eps && q.end.0 <= p.end.0 + eps {
                area -= own(q)?.0;
            }
        }
    }
    Some((area.abs(), height.abs()))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn peak_bytes(
        start: Point,
        end: Point,
        apex: Point,
        rt: f64,
        area: f64,
        height: f64,
        skim: u8,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        let f = |b: &mut Vec<u8>, x: f64| b.extend(x.to_le_bytes());
        f(&mut b, start.0);
        f(&mut b, start.1);
        f(&mut b, end.0);
        f(&mut b, end.1);
        b.extend(0x0fu32.to_le_bytes());
        f(&mut b, apex.0);
        f(&mut b, apex.1);
        for _ in 0..2 {
            // absent, 10 %, 50 %, 5 % absent, absent, tangent point absent, slope, absent
            b.push(0);
            b.push(1);
            f(&mut b, apex.0);
            f(&mut b, 1.0);
            b.push(1);
            f(&mut b, apex.0);
            f(&mut b, 2.0);
            b.push(0);
            b.push(0);
            b.push(0);
            b.push(1);
            f(&mut b, 3.5);
            b.push(0);
        }
        f(&mut b, rt);
        f(&mut b, area);
        f(&mut b, height);
        b.push(0); // not identified
        b.push(skim);
        b
    }

    pub(crate) fn chm_blob(peaks: &[Vec<u8>], baselines: &[Vec<Point>]) -> Vec<u8> {
        let mut body = vec![0u8; 8];
        body.push(0); // no skim lines
        body.push(peaks.len() as u8);
        for p in peaks {
            body.extend(p);
        }
        body.push(baselines.len() as u8);
        for s in baselines {
            body.push(s.len() as u8);
            for (t, v) in s {
                body.extend(t.to_le_bytes());
                body.extend(v.to_le_bytes());
            }
        }
        let mut b = b"ChmData.".to_vec();
        b.extend([1, 1, 0, 0, 1, 0, 0, 0, 0x67, 0, 0, 0, 1, 0, 0, 0]);
        for _ in 0..3 {
            b.push(32);
            b.extend([0xAB; 32]);
        }
        b.extend(2u32.to_le_bytes());
        b.extend((body.len() as u32).to_le_bytes());
        b.extend(&body);
        b.extend(4u32.to_le_bytes());
        b.extend(8u32.to_le_bytes());
        b.extend([1, 0, 0, 0, 3, 3, 2, 0]);
        b
    }

    #[test]
    fn stored_results_round_trip() {
        let p = peak_bytes(
            (5.35, 43.955),
            (5.415, 37.855),
            (5.369, 79.37),
            5.369,
            0.903,
            37.158,
            0,
        );
        let blob = chm_blob(&[p], &[vec![(5.35, 43.955), (5.415, 37.855)]]);
        let r = parse_stored_results(&blob).unwrap().unwrap();
        assert_eq!(r.version, 1);
        assert_eq!(r.peaks.len(), 1);
        let pk = &r.peaks[0];
        assert_eq!(pk.retention_min, 5.369);
        assert_eq!(pk.area, 0.903);
        assert_eq!(pk.leading.at_10, Some((5.369, 1.0)));
        assert_eq!(pk.leading.at_5, None);
        assert_eq!(pk.trailing.tangent_slope, Some(3.5));
        assert_eq!(r.baseline_line(pk), Some(((5.35, 43.955), (5.415, 37.855))));
        // every truncation is an error, never a panic
        for cut in 0..blob.len() {
            // a cut exactly between sections leaves a valid blob (no peak section: no results)
            let r = parse_stored_results(&blob[..cut]);
            assert!(
                r.is_err() || cut >= blob.len() - 16 || r == Ok(None),
                "cut {cut}"
            );
        }
        for i in 0..blob.len() {
            let mut b = blob.clone();
            b[i] ^= 0x5A;
            let _ = parse_stored_results(&b);
        }
        // an unknown header version is refused
        let mut v = blob.clone();
        v[9] = 3;
        assert!(matches!(
            parse_stored_results(&v),
            Err(SignalError::Unsupported(_))
        ));
    }

    #[test]
    fn blobs() {
        // CpXm-like: 8-byte header, dict byte, LZMA2 of "hello hello hello hello, lzma2!"
        let lz = [
            0xe0, 0x00, 0x1e, 0x00, 0x14, 0x5d, 0x00, 0x34, 0x19, 0x49, 0xee, 0x8d, 0xe9, 0x56,
            0x0a, 0xe7, 0x79, 0x9a, 0x19, 0x12, 0x45, 0x43, 0xa9, 0xdf, 0x4c, 0x4c, 0x40, 0x00,
        ];
        let mut b = b"CpXm".to_vec();
        b.extend([8, 0, 1, 0, 0x08]);
        b.extend(lz);
        assert_eq!(unpack_blob(&b).unwrap(), b"hello hello hello hello, lzma2!");
        // DIAC-like: 16-byte header with sizes
        let mut d = b"DIAC".to_vec();
        d.extend([16, 0, 1, 0]);
        d.extend(((lz.len() + 1) as u32).to_le_bytes());
        d.extend(31u32.to_le_bytes());
        d.push(0x08);
        d.extend(lz);
        assert_eq!(unpack_blob(&d).unwrap().len(), 31);
        assert_eq!(blob_stated_size(&d), Some(31));
        // wrong stated size
        let mut w = d.clone();
        w[12] = 30;
        assert!(unpack_blob(&w).is_err());
        for cut in 0..d.len() {
            assert!(unpack_blob(&d[..cut]).is_err());
        }
        assert!(unpack_blob(b"notablob..").is_err());
    }

    #[test]
    fn method_and_audit_xml() {
        let m = r#"<CmData><Method><Children><Item type="StageNode"><Children><Item type="TimeStepNode"><Time type="MethodTime"><InternalValue value="-Infinity" /></Time><Children><Item type="PropertyStepNode"><SymbolPath value="Pump_1.Flow" /><Value value="0.250 [ml/min]" /></Item></Children></Item><Item type="TimeStepNode"><Time type="MethodTime"><InternalValue value="2.80000000000000000E+001" /></Time><Children><Item type="CommandStepNode"><SymbolPath value="EDet1.Autozero" /><Value value="" /></Item></Children></Item></Children><StageName value="Run" /></Item></Children></Method></CmData>"#;
        let s = method_steps(m).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].time_min, None);
        assert_eq!(s[0].symbol, "Pump_1.Flow");
        assert_eq!(s[1].time_min, Some(28.0));
        assert_eq!(s[1].kind, "command");
        assert_eq!(s[1].stage, "Run");
        let a = r#"<CmData><Messages><Item type="AuditTrailMessage"><Level value="Normal" /><Category value="PreconditionLog" /><Time value="2022-08-19T18:57:48.0000000+02:00" /><Message value="Connected" /><Device value="Sampler" /><PropertyName value="Connected" /><PropertyValue value="Connected" /></Item></Messages></CmData>"#;
        let v = audit_messages(a).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].device.as_deref(), Some("Sampler"));
        assert!(method_steps("not xml").is_err());
    }
}

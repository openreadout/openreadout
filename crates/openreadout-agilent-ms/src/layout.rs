//! Byte layouts of the binary files in a MassHunter `AcqData` directory, as derived in
//! `docs/formats/agilent-masshunter.md`: the scan index (`MSScan.bin`), whose record fields
//! are listed by the `MSScan.xsd` shipped next to it; spectrum blocks in `MSPeak.bin`
//! (uncompressed centroids) and `MSProfile.bin` (LZF-compressed profiles); per-scan
//! calibration records (`MSMassCal.bin`); and device signals (`*.cd` descriptor + `*.cg` data).
//!
//! Every accessor is bounds-checked: a short or malformed file yields `None` or an error
//! string, never a panic.

use openreadout_core::bytes::{le_f32, le_f64, le_i16, le_i32, le_i64, le_u16, le_u32};

/// Offset of the first data byte in every MassHunter `.bin` file (after the 68-byte header).
pub const DATA_START: usize = 0x44;

/// File-type tag (little-endian `u16` at byte 0) of `MSScan.bin`.
pub const TAG_SCAN: u16 = 0x0101;
/// File-type tag of `MSProfile.bin`.
pub const TAG_PROFILE: u16 = 0x0102;
/// File-type tag of `MSPeak.bin`.
pub const TAG_PEAK: u16 = 0x0103;
/// File-type tag of `MSMassCal.bin`.
pub const TAG_MASS_CAL: u16 = 0x0104;

/// Spectrum block kind 1: a sampled profile in `MSProfile.bin`.
pub const FORMAT_PROFILE: i32 = 1;
/// Spectrum block kind 2: centroids (peak list) in `MSPeak.bin`.
pub const FORMAT_PEAK: i32 = 2;
/// Spectrum block kind 3: a quadrupole (MRM/SIM/scan) point list in `MSPeak.bin`.
pub const FORMAT_QUAD_PEAK: i32 = 3;
/// File-type tag of `IMSFrame.bin` (ion-mobility frame index).
pub const TAG_IMS_FRAME: u16 = 0x0116;
/// Header word (top byte of the `u32` after the grid) of every ion-mobility profile block seen:
/// the only variant decoded.
pub const IMS_PROFILE_FLAGS: u32 = 0x90;

// ---------------------------------------------------------------------------------------
// MSScan.xsd → record layout
// ---------------------------------------------------------------------------------------

/// Storage type of one scan-record field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// 1-byte signed integer (`xs:byte`).
    Int8,
    /// 2-byte signed integer (`xs:short`).
    Int16,
    /// 4-byte signed integer (`xs:int`).
    Int32,
    /// 8-byte signed integer (`xs:long`).
    Int64,
    /// 4-byte float (`xs:float`; also `ChromScaleFactor`, which the schema calls a double).
    Float32,
    /// 8-byte float (`xs:double`).
    Float64,
}

impl FieldType {
    /// Stored width in bytes.
    pub fn size(self) -> usize {
        match self {
            FieldType::Int8 => 1,
            FieldType::Int16 => 2,
            FieldType::Int32 | FieldType::Float32 => 4,
            FieldType::Int64 | FieldType::Float64 => 8,
        }
    }
    fn of_xsd(t: &str) -> Option<Self> {
        Some(match t {
            "xs:byte" => FieldType::Int8,
            "xs:short" => FieldType::Int16,
            "xs:int" => FieldType::Int32,
            "xs:long" => FieldType::Int64,
            "xs:float" => FieldType::Float32,
            "xs:double" => FieldType::Float64,
            _ => return None,
        })
    }
    /// Read the value at `at` as `f64` (integers convert exactly up to 2^53).
    pub fn read(self, b: &[u8], at: usize) -> Option<f64> {
        Some(match self {
            FieldType::Int8 => f64::from(*b.get(at)? as i8),
            FieldType::Int16 => f64::from(le_i16(b, at)?),
            FieldType::Int32 => f64::from(le_i32(b, at)?),
            FieldType::Int64 => le_i64(b, at)? as f64,
            FieldType::Float32 => f64::from(le_f32(b, at)?),
            FieldType::Float64 => le_f64(b, at)?,
        })
    }
}

/// One field of a scan record: its schema name, type and byte offset in the record.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Name as the file's `MSScan.xsd` spells it (a nested type's fields are `Parent.Child`).
    pub name: String,
    /// Storage type.
    pub ty: FieldType,
    /// Byte offset within the record (or within one spectrum block).
    pub offset: usize,
}

/// Layout of one `MSScan.bin` record: the fixed part, then `SpectrumParamValues` blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanLayout {
    /// Fields of the fixed part, in file order.
    pub fields: Vec<Field>,
    /// Length of the fixed part in bytes.
    pub fixed_len: usize,
    /// Fields of one spectrum block.
    pub block_fields: Vec<Field>,
    /// Length of one spectrum block in bytes.
    pub block_len: usize,
    /// Ion-mobility layout: one record per (frame, drift bin), no spectrum blocks; the frame
    /// holds the time and fragmentation (`IMSFrame.bin`).
    pub ims: bool,
}

impl ScanLayout {
    /// Record length with `blocks` spectrum blocks.
    pub fn record_len(&self, blocks: usize) -> usize {
        self.fixed_len + blocks * self.block_len
    }
    /// Offset and type of fixed-part field `name`.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<!--") {
        out.push_str(&rest[..i]);
        match rest[i..].find("-->") {
            Some(j) => rest = &rest[i + j + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let i = tag.find(&key)? + key.len();
    let j = tag[i..].find('"')?;
    Some(&tag[i..i + j])
}

/// `(name, type)` of every `xs:element` of complex type `ty`, in schema order.
fn complex_type(xsd: &str, ty: &str) -> Option<Vec<(String, String)>> {
    let open = format!("<xs:complexType name=\"{ty}\"");
    let start = xsd.find(&open)?;
    let body = &xsd[start..];
    let end = body.find("</xs:complexType>")?;
    let body = &body[..end];
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("<xs:element ") {
        let tag_end = rest[i..].find('>').map_or(rest.len(), |j| i + j);
        let tag = &rest[i..tag_end];
        if let (Some(n), Some(t)) = (attr(tag, "name"), attr(tag, "type")) {
            out.push((n.to_string(), t.to_string()));
        }
        rest = &rest[tag_end..];
    }
    Some(out)
}

fn flatten(
    xsd: &str,
    ty: &str,
    prefix: &str,
    depth: u32,
    out: &mut Vec<(String, FieldType)>,
) -> Result<(), String> {
    if depth > 4 {
        return Err(format!("MSScan.xsd: type {ty} nests too deeply"));
    }
    let elems = complex_type(xsd, ty).ok_or_else(|| format!("MSScan.xsd: no complex type {ty}"))?;
    for (name, t) in elems {
        let full = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if name == "SpectrumParamValues" {
            continue; // the repeated block, laid out separately
        }
        if name == "ChromScaleFactor" {
            // declared xs:double, stored as a 4-byte float (derived from record strides)
            out.push((full, FieldType::Float32));
        } else if let Some(ft) = FieldType::of_xsd(&t) {
            out.push((full, ft));
        } else {
            flatten(xsd, &t, &full, depth + 1, out)?;
        }
    }
    Ok(())
}

fn with_offsets(v: Vec<(String, FieldType)>) -> (Vec<Field>, usize) {
    let mut off = 0usize;
    let fields = v
        .into_iter()
        .map(|(name, ty)| {
            let f = Field {
                name,
                ty,
                offset: off,
            };
            off += ty.size();
            f
        })
        .collect();
    (fields, off)
}

/// Build the record layout from the text of the `MSScan.xsd` stored in the data directory.
/// Commented-out elements are not stored; every other element is, in schema order.
pub fn scan_layout_from_xsd(xsd: &str) -> Result<ScanLayout, String> {
    let xsd = strip_comments(xsd);
    let mut fixed = Vec::new();
    flatten(&xsd, "ScanRecordType", "", 0, &mut fixed)?;
    let has = |v: &[(String, FieldType)], n: &str| v.iter().any(|(f, _)| f == n);
    let has_blocks = complex_type(&xsd, "ScanRecordType")
        .is_some_and(|v| v.iter().any(|(n, _)| n == "SpectrumParamValues"));
    if !has_blocks && has(&fixed, "FrameID") && has(&fixed, "DriftBin") {
        for need in IMS_SCAN_FIELDS {
            if !has(&fixed, need) {
                return Err(format!(
                    "MSScan.xsd: ion-mobility record has no {need} field"
                ));
            }
        }
        let (fields, fixed_len) = with_offsets(fixed);
        return Ok(ScanLayout {
            fields,
            fixed_len,
            block_fields: Vec::new(),
            block_len: 0,
            ims: true,
        });
    }
    let block_type = complex_type(&xsd, "ScanRecordType")
        .and_then(|v| v.into_iter().find(|(n, _)| n == "SpectrumParamValues"))
        .map(|(_, t)| t)
        .ok_or("MSScan.xsd: no SpectrumParamValues element")?;
    let mut block = Vec::new();
    flatten(&xsd, &block_type, "", 0, &mut block)?;
    let (fields, fixed_len) = with_offsets(fixed);
    let (block_fields, block_len) = with_offsets(block);
    for need in ["ScanID", "ScanTime", "MSLevel"] {
        if !fields.iter().any(|f| f.name == need) {
            return Err(format!("MSScan.xsd: record has no {need} field"));
        }
    }
    for need in [
        "SpectrumFormatID",
        "SpectrumOffset",
        "ByteCount",
        "PointCount",
    ] {
        if !block_fields.iter().any(|f| f.name == need) {
            return Err(format!("MSScan.xsd: spectrum block has no {need} field"));
        }
    }
    Ok(ScanLayout {
        fields,
        fixed_len,
        block_fields,
        block_len,
        ims: false,
    })
}

/// Fields an ion-mobility `MSScan.xsd` record must have to be read.
const IMS_SCAN_FIELDS: [&str; 7] = [
    "ScanID",
    "FrameID",
    "DriftBin",
    "TIC",
    "MsProfByteCount",
    "MsProfOffset",
    "MsProfPointCount",
];

/// The schema text used when a data directory has no `MSScan.xsd`: the element list of the
/// header-version-6 files in the corpus (acquisition software B.05–B.08).
pub const FALLBACK_XSD_V6: &str = r#"
<xs:complexType name="ScanRecordType"><xs:sequence>
<xs:element name="ScanID" type="xs:int"/><xs:element name="ScanMethodID" type="xs:int"/>
<xs:element name="TimeSegmentID" type="xs:int"/><xs:element name="ScanTime" type="xs:double"/>
<xs:element name="MSLevel" type="xs:int"/><xs:element name="ScanType" type="xs:int"/>
<xs:element name="TIC" type="xs:double"/><xs:element name="BasePeakMZ" type="xs:double"/>
<xs:element name="BasePeakValue" type="xs:double"/><xs:element name="CalibrationID" type="xs:int"/>
<xs:element name="CycleNumber" type="xs:int"/><xs:element name="IonMode" type="xs:int"/>
<xs:element name="IonPolarity" type="xs:int"/><xs:element name="Fragmentor" type="xs:double"/>
<xs:element name="CollisionEnergy" type="xs:double"/><xs:element name="MzOfInterest" type="xs:double"/>
<xs:element name="AbundanceLimit" type="xs:double"/><xs:element name="SamplingPeriod" type="xs:double"/>
<xs:element name="Threshold" type="xs:double"/><xs:element name="ChargeState" type="xs:int"/>
<xs:element name="ChromScaleFactor" type="xs:double"/><xs:element name="MassCalOffset" type="xs:long"/>
<xs:element name="ActualsOffset" type="xs:long"/><xs:element name="NumOfActualsPerScan" type="xs:int"/>
<xs:element name="DataDependentScanParamType" type="DataDependentScanParamType"/>
<xs:element name="SpectrumParamValues" type="SpectrumParamsType"/>
</xs:sequence></xs:complexType>
<xs:complexType name="SpectrumParamsType"><xs:sequence>
<xs:element name="SpectrumFormatID" type="xs:int"/><xs:element name="SpectrumOffset" type="xs:long"/>
<xs:element name="ByteCount" type="xs:int"/><xs:element name="PointCount" type="xs:int"/>
<xs:element name="UncompressedByteCount" type="xs:int"/><xs:element name="MinX" type="xs:double"/>
<xs:element name="MaxX" type="xs:double"/><xs:element name="MinY" type="xs:double"/>
<xs:element name="MaxY" type="xs:double"/><xs:element name="MeasuredNoise" type="xs:double"/>
</xs:sequence></xs:complexType>
<xs:complexType name="DataDependentScanParamType"><xs:sequence>
<xs:element name="DDScanID" type="xs:int"/><xs:element name="DDScanParamMz" type="xs:double"/>
</xs:sequence></xs:complexType>"#;

/// Fallback schema for header-version-5 files (triple-quadrupole acquisitions in the corpus).
pub const FALLBACK_XSD_V5: &str = r#"
<xs:complexType name="ScanRecordType"><xs:sequence>
<xs:element name="ScanID" type="xs:int"/><xs:element name="ScanMethodID" type="xs:int"/>
<xs:element name="TimeSegmentID" type="xs:int"/><xs:element name="ScanTime" type="xs:double"/>
<xs:element name="MSLevel" type="xs:int"/><xs:element name="ScanType" type="xs:int"/>
<xs:element name="TIC" type="xs:double"/><xs:element name="BasePeakMZ" type="xs:double"/>
<xs:element name="BasePeakValue" type="xs:double"/><xs:element name="CycleNumber" type="xs:int"/>
<xs:element name="Status" type="xs:int"/><xs:element name="IonMode" type="xs:int"/>
<xs:element name="IonPolarity" type="xs:int"/><xs:element name="Fragmentor" type="xs:double"/>
<xs:element name="CollisionEnergy" type="xs:double"/><xs:element name="MzOfInterest" type="xs:double"/>
<xs:element name="SamplingPeriod" type="xs:double"/><xs:element name="MeasuredMassRangeMin" type="xs:double"/>
<xs:element name="MeasuredMassRangeMax" type="xs:double"/><xs:element name="Threshold" type="xs:double"/>
<xs:element name="IsFragmentorDynamic" type="xs:int"/><xs:element name="IsCollisionEnergyDynamic" type="xs:int"/>
<xs:element name="DataDependentScanParamType" type="DataDependentScanParamType"/>
<xs:element name="SpectrumParamValues" type="SpectrumParamsType"/>
</xs:sequence></xs:complexType>
<xs:complexType name="SpectrumParamsType"><xs:sequence>
<xs:element name="SpectrumFormatID" type="xs:int"/><xs:element name="SpectrumOffset" type="xs:long"/>
<xs:element name="ByteCount" type="xs:int"/><xs:element name="PointCount" type="xs:int"/>
<xs:element name="MinY" type="xs:double"/><xs:element name="MaxY" type="xs:double"/>
<xs:element name="MinX" type="xs:double"/><xs:element name="MaxX" type="xs:double"/>
</xs:sequence></xs:complexType>
<xs:complexType name="DataDependentScanParamType"><xs:sequence>
<xs:element name="DDScanID" type="xs:int"/><xs:element name="DDScanParamMz" type="xs:double"/>
</xs:sequence></xs:complexType>"#;

// ---------------------------------------------------------------------------------------
// MSScan.bin
// ---------------------------------------------------------------------------------------

/// The fixed header words of `MSScan.bin` (at bytes 0x44–0x5B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanFileHeader {
    /// File-type tag (0x0101).
    pub tag: u16,
    /// Record-layout generation (5 in the triple-quadrupole files, 6 in the Q-TOF files).
    pub layout_version: i32,
    /// Spectrum blocks per record (1 = peaks only, 2 = profile + peaks).
    pub block_count: i32,
    /// Byte offset of the first scan record.
    pub first_record: i32,
}

/// Parse the header of `MSScan.bin`.
pub fn scan_file_header(b: &[u8]) -> Result<ScanFileHeader, String> {
    let tag = le_u16(b, 0).ok_or("MSScan.bin is shorter than its 2-byte tag")?;
    if tag != TAG_SCAN {
        return Err(format!("MSScan.bin starts with tag {tag:#06x}, not 0x0101"));
    }
    let layout_version = le_i32(b, 0x48).ok_or("MSScan.bin header is truncated")?;
    let block_count = le_i32(b, 0x4C).ok_or("MSScan.bin header is truncated")?;
    let first_record = le_i32(b, 0x58).ok_or("MSScan.bin header is truncated")?;
    if !(1..=16).contains(&block_count) {
        return Err(format!(
            "MSScan.bin declares {block_count} spectrum blocks per record"
        ));
    }
    if first_record < 0x5C || first_record as usize > b.len() {
        return Err(format!(
            "MSScan.bin first record offset {first_record} is outside the file"
        ));
    }
    Ok(ScanFileHeader {
        tag,
        layout_version,
        block_count,
        first_record,
    })
}

/// One spectrum block of a scan record.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SpectrumBlock {
    /// 1 = profile (`MSProfile.bin`), 2 = centroids, 3 = quadrupole points (`MSPeak.bin`).
    pub format_id: i32,
    /// Byte offset of the block's data in its file.
    pub offset: i64,
    /// Stored byte count.
    pub byte_count: i64,
    /// Number of points (profile: sampled bins).
    pub point_count: i64,
    /// Byte count after decompression (0 or absent when stored uncompressed).
    pub uncompressed_byte_count: Option<i64>,
    /// Lowest m/z of the block.
    pub min_x: Option<f64>,
    /// Highest m/z of the block.
    pub max_x: Option<f64>,
    /// Lowest abundance.
    pub min_y: Option<f64>,
    /// Highest abundance.
    pub max_y: Option<f64>,
}

/// One `MSScan.bin` record, with the fields every reader needs typed and all schema fields
/// kept by name for `vendor` output.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScanRecord {
    /// Byte offset of the record in `MSScan.bin`.
    pub record_offset: u64,
    /// Instrument scan id (`scanId=` in converted files).
    pub scan_id: i64,
    /// Retention time in minutes.
    pub scan_time_min: f64,
    /// MS level (1 = MS, 2 = MS/MS and MRM).
    pub ms_level: i32,
    /// Scan type code (1 = scan, 256 = MRM, 512 = product ion; see the format notes).
    pub scan_type: i32,
    /// Total ion current as recorded.
    pub tic: f64,
    /// m/z of the base peak.
    pub base_peak_mz: f64,
    /// Abundance of the base peak.
    pub base_peak_value: f64,
    /// 0 = positive, 1 = negative (as observed).
    pub ion_polarity: Option<i32>,
    /// Ionization-mode code.
    pub ion_mode: Option<i32>,
    /// Fragmentor voltage (negative in data-dependent MS/MS scans, as stored).
    pub fragmentor: Option<f64>,
    /// Collision energy in V.
    pub collision_energy: Option<f64>,
    /// Precursor (MS/MS) or Q1 (MRM) m/z.
    pub mz_of_interest: Option<f64>,
    /// Precursor charge state (0 = unknown).
    pub charge_state: Option<i32>,
    /// Which default calibration (`DefaultMassCal.xml`) the scan uses.
    pub calibration_id: Option<i32>,
    /// Offset of the scan's calibration record in `MSMassCal.bin`.
    pub mass_cal_offset: Option<i64>,
    /// Sampling period of the digitizer in ns.
    pub sampling_period: Option<f64>,
    /// Scan id of the survey scan that triggered this data-dependent scan.
    pub parent_scan_id: Option<i64>,
    /// Scan-method number.
    pub scan_method_id: Option<i32>,
    /// Time-segment number (`MSTS.xml`).
    pub time_segment_id: Option<i32>,
    /// Acquisition cycle number.
    pub cycle_number: Option<i32>,
    /// Ion-mobility frame the record belongs to (`FrameID`; ion-mobility layout only).
    pub frame_id: Option<i32>,
    /// Drift bin of the record within its frame (`DriftBin`; 0 = the frame's summed spectrum).
    pub drift_bin: Option<i32>,
    /// Drift time, ms: `(DriftBin − 1) × FrameDtPeriod` of the frame's method.
    pub drift_time_ms: Option<f64>,
    /// Spectrum blocks, in file order.
    pub blocks: Vec<SpectrumBlock>,
    /// Every fixed-part field as stored, in the order of [`ScanLayout::fields`].
    pub values: Vec<f64>,
}

impl ScanRecord {
    /// The block of `format_id`, if the record has one with data.
    pub fn block(&self, format_id: i32) -> Option<&SpectrumBlock> {
        self.blocks.iter().find(|b| b.format_id == format_id)
    }
}

/// Decode record `index` (zero-based) of `MSScan.bin`.
pub fn read_record(
    b: &[u8],
    h: &ScanFileHeader,
    layout: &ScanLayout,
    index: usize,
) -> Option<ScanRecord> {
    let blocks = h.block_count as usize;
    let len = layout.record_len(blocks);
    let start = (h.first_record as usize).checked_add(index.checked_mul(len)?)?;
    let rec = b.get(start..start.checked_add(len)?)?;
    let mut values = Vec::with_capacity(layout.fields.len());
    for f in &layout.fields {
        values.push(f.ty.read(rec, f.offset)?);
    }
    let get = |n: &str| {
        layout
            .fields
            .iter()
            .position(|f| f.name == n)
            .and_then(|i| values.get(i).copied())
    };
    let int = |n: &str| get(n).map(|v| v as i32);
    if layout.ims {
        let byte_count = get("MsProfByteCount")? as i64;
        let mut out = ScanRecord {
            record_offset: start as u64,
            scan_id: get("ScanID")? as i64,
            ms_level: 1,
            tic: get("TIC")?,
            base_peak_value: get("BaseAbund").unwrap_or(0.0),
            frame_id: int("FrameID"),
            drift_bin: int("DriftBin"),
            ..ScanRecord::default()
        };
        if byte_count > 0 {
            out.blocks.push(SpectrumBlock {
                format_id: int("MsProfSpecFmtId").unwrap_or(0),
                offset: get("MsProfOffset")? as i64,
                byte_count,
                point_count: get("MsProfPointCount")? as i64,
                uncompressed_byte_count: get("MsProfFullByteCount").map(|v| v as i64),
                ..SpectrumBlock::default()
            });
        }
        out.values = values;
        return Some(out);
    }
    let mut out = ScanRecord {
        record_offset: start as u64,
        scan_id: get("ScanID")? as i64,
        scan_time_min: get("ScanTime")?,
        ms_level: int("MSLevel")?,
        scan_type: int("ScanType").unwrap_or(0),
        tic: get("TIC").unwrap_or(0.0),
        base_peak_mz: get("BasePeakMZ").unwrap_or(0.0),
        base_peak_value: get("BasePeakValue").unwrap_or(0.0),
        ion_polarity: int("IonPolarity"),
        ion_mode: int("IonMode"),
        fragmentor: get("Fragmentor"),
        collision_energy: get("CollisionEnergy"),
        mz_of_interest: get("MzOfInterest"),
        charge_state: int("ChargeState"),
        calibration_id: int("CalibrationID"),
        mass_cal_offset: get("MassCalOffset").map(|v| v as i64),
        sampling_period: get("SamplingPeriod"),
        parent_scan_id: get("DataDependentScanParamType.DDScanID").map(|v| v as i64),
        scan_method_id: int("ScanMethodID"),
        time_segment_id: int("TimeSegmentID"),
        cycle_number: int("CycleNumber"),
        frame_id: None,
        drift_bin: None,
        drift_time_ms: None,
        blocks: Vec::with_capacity(blocks),
        values: Vec::new(),
    };
    for k in 0..blocks {
        let at = layout.fixed_len + k * layout.block_len;
        let blk = rec.get(at..at + layout.block_len)?;
        let bget = |n: &str| {
            layout
                .block_fields
                .iter()
                .find(|f| f.name == n)
                .and_then(|f| f.ty.read(blk, f.offset))
        };
        out.blocks.push(SpectrumBlock {
            format_id: bget("SpectrumFormatID")? as i32,
            offset: bget("SpectrumOffset")? as i64,
            byte_count: bget("ByteCount")? as i64,
            point_count: bget("PointCount")? as i64,
            uncompressed_byte_count: bget("UncompressedByteCount").map(|v| v as i64),
            min_x: bget("MinX"),
            max_x: bget("MaxX"),
            min_y: bget("MinY"),
            max_y: bget("MaxY"),
        });
    }
    out.values = values;
    Some(out)
}

// ---------------------------------------------------------------------------------------
// Spectrum data
// ---------------------------------------------------------------------------------------

/// Decompress an LZF stream (the classic LZF scheme: a control byte below 32 copies that many
/// plus one literal bytes; otherwise its top three bits give a back-reference length, with an
/// extra length byte when they are all set, and the low five bits plus the next byte an
/// offset). `limit` caps the output size.
pub fn lzf_decompress(src: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if limit == 0 {
        return if src.is_empty() {
            Ok(Vec::new())
        } else {
            Err("LZF output exceeds the declared size".into())
        };
    }
    openreadout_codecs::lzf_decode(src, limit).map_err(|e| e.to_string())
}

/// A decoded centroid or quadrupole block: raw x (flight time or m/z) and abundances.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeakData {
    /// x values as stored (flight time in ns for time-of-flight centroids, else m/z).
    pub x: Vec<f64>,
    /// Abundances.
    pub y: Vec<f32>,
}

/// Decode an `MSPeak.bin` block: `n` little-endian f64 x values, then `n` f32 abundances.
pub fn decode_peaks(data: &[u8], n: usize) -> Result<PeakData, String> {
    let need = n.checked_mul(12).ok_or("peak count overflows")?;
    // Triple-quadrupole point lists (2006-2008 files): 8 bytes per point, `n` f32 m/z then
    // `n` i32 counts.
    if n > 0 && data.len() == 8 * n {
        let x = (0..n)
            .map(|k| f64::from(le_f32(data, 4 * k).unwrap_or(0.0)))
            .collect();
        let y = (0..n)
            .map(|k| le_i32(data, 4 * n + 4 * k).unwrap_or(0) as f32)
            .collect();
        return Ok(PeakData { x, y });
    }
    if data.len() != need {
        return Err(format!(
            "peak block holds {} bytes; {n} points need {need} (compressed peak blocks are not decoded)",
            data.len()
        ));
    }
    let x = (0..n).map(|k| le_f64(data, 8 * k).unwrap_or(0.0)).collect();
    let y = (0..n)
        .map(|k| le_f32(data, 8 * n + 4 * k).unwrap_or(0.0))
        .collect();
    Ok(PeakData { x, y })
}

/// A decoded profile: the flight time of bin 0, the bin width, and the counts per bin.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfileData {
    /// Flight time of the first bin, ns.
    pub first_x: f64,
    /// Bin width, ns (the digitizer sampling period).
    pub step_x: f64,
    /// Counts per bin.
    pub counts: Vec<i32>,
}

/// A quadrupole profile block (older triple-quadrupole and ion-trap-era files, `MSProfile.bin`,
/// spectrum format 2 with `8 + 4n` bytes): f32 first m/z, f32 m/z step, then `n` i32 counts.
/// Returns (first m/z, step, counts).
pub fn decode_quad_profile(data: &[u8], n: usize) -> Result<(f64, f64, Vec<i32>), String> {
    let need = n
        .checked_mul(4)
        .and_then(|v| v.checked_add(8))
        .ok_or("profile point count overflows")?;
    if data.len() != need {
        return Err(format!(
            "quadrupole profile block holds {} bytes; {n} points need {need}",
            data.len()
        ));
    }
    let first = f64::from(le_f32(data, 0).ok_or("profile header cut short")?);
    let step = f64::from(le_f32(data, 4).ok_or("profile header cut short")?);
    if !(first.is_finite() && step.is_finite() && step > 0.0) {
        return Err(format!(
            "quadrupole profile grid {first} + k * {step} is not usable"
        ));
    }
    let counts = (0..n)
        .map(|k| le_i32(data, 8 + 4 * k).unwrap_or(0))
        .collect();
    Ok((first, step, counts))
}

/// Decode an `MSProfile.bin` block (LZF-compressed unless its byte counts are equal):
/// f64 first flight time, f64 bin width, then `n` i32 counts.
pub fn decode_profile(
    data: &[u8],
    n: usize,
    uncompressed: Option<i64>,
) -> Result<ProfileData, String> {
    let need = n
        .checked_mul(4)
        .and_then(|v| v.checked_add(16))
        .ok_or("profile size overflows")?;
    let raw;
    let body: &[u8] = match uncompressed {
        Some(u) if u > 0 && u as usize != data.len() => {
            if u as usize != need {
                return Err(format!(
                    "profile declares {u} decompressed bytes; {n} bins need {need}"
                ));
            }
            raw = lzf_decompress(data, need)?;
            &raw
        }
        _ => data,
    };
    if body.len() != need {
        return Err(format!(
            "profile decodes to {} bytes; {n} bins need {need}",
            body.len()
        ));
    }
    let first_x = le_f64(body, 0).unwrap_or(0.0);
    let step_x = le_f64(body, 8).unwrap_or(0.0);
    let counts = (0..n)
        .map(|k| le_i32(body, 16 + 4 * k).unwrap_or(0))
        .collect();
    Ok(ProfileData {
        first_x,
        step_x,
        counts,
    })
}

/// A decoded ion-mobility profile block: the flight-time grid and its non-zero bins.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImsProfile {
    /// Flight time of bin 0, ns.
    pub first_x: f64,
    /// Bin width, ns.
    pub step_x: f64,
    /// Bins in the block's grid (the record's `MsProfPointCount`).
    pub point_count: u32,
    /// `(bin, count)` of every stored bin, in bin order.
    pub bins: Vec<(u32, i64)>,
}

/// Decode an ion-mobility profile block (`MSProfile.bin` of an ion-mobility data directory):
/// f64 first flight time, f64 bin width, u32 header (top byte [`IMS_PROFILE_FLAGS`], low 24
/// bits the bin count), i32 minus the first bin, then a stream of little-endian signed
/// integers whose width starts at 4 bytes. A value `v >= 0` is the count of the current bin
/// (the bin then advances by one); a negative `v` skips `!(v >> 2)` empty bins and sets the
/// width of the next values from its low two bits (3 → 1 byte, 2 → 2, 1 → 4, 0 → 8).
pub fn decode_ims_profile(data: &[u8]) -> Result<ImsProfile, String> {
    let first_x = le_f64(data, 0).ok_or("ion-mobility profile header cut short")?;
    let step_x = le_f64(data, 8).ok_or("ion-mobility profile header cut short")?;
    let head = le_u32(data, 16).ok_or("ion-mobility profile header cut short")?;
    let start = le_i32(data, 20).ok_or("ion-mobility profile header cut short")?;
    if !(first_x.is_finite() && step_x.is_finite() && step_x > 0.0) {
        return Err(format!(
            "ion-mobility profile grid {first_x} + k * {step_x} is not usable"
        ));
    }
    if head >> 24 != IMS_PROFILE_FLAGS {
        return Err(format!(
            "ion-mobility profile header {head:#010x}: only header byte {IMS_PROFILE_FLAGS:#04x} has been validated"
        ));
    }
    let point_count = head & 0x00FF_FFFF;
    let mut pos = i64::from(start)
        .checked_neg()
        .filter(|&p| p >= 0)
        .ok_or_else(|| format!("ion-mobility profile starts at bin {}", -i64::from(start)))?;
    let mut bins = Vec::new();
    let mut width = 4usize;
    let mut i = 24usize;
    while i < data.len() {
        let v = match width {
            1 => data.get(i).map(|&c| i64::from(c as i8)),
            2 => le_i16(data, i).map(i64::from),
            4 => le_i32(data, i).map(i64::from),
            _ => le_i64(data, i),
        }
        .ok_or_else(|| format!("ion-mobility profile value at byte {i} is cut off"))?;
        i += width;
        if v >= 0 {
            if pos >= i64::from(point_count) {
                return Err(format!(
                    "ion-mobility profile writes bin {pos} of a {point_count}-bin grid"
                ));
            }
            bins.push((pos as u32, v));
            pos += 1;
        } else {
            pos = pos
                .checked_add(!(v >> 2))
                .ok_or("ion-mobility profile skip overflows")?;
            width = match v & 3 {
                3 => 1,
                2 => 2,
                1 => 4,
                _ => 8,
            };
        }
    }
    Ok(ImsProfile {
        first_x,
        step_x,
        point_count,
        bins,
    })
}

// ---------------------------------------------------------------------------------------
// Ion-mobility frames: IMSFrame.bin (+ IMSFrame.xsd) and IMSFrameMeth.xml
// ---------------------------------------------------------------------------------------

/// One `IMSFrame.bin` record: a mobility separation, holding the drift-bin scans of
/// `MSScan.bin` that carry its `FrameID`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameRecord {
    /// Frame number (`FrameId`).
    pub frame_id: i32,
    /// Frame method in `IMSFrameMeth.xml` (`FrameMethodId`).
    pub method_id: i32,
    /// Time segment (`TimeSegmentId`).
    pub time_segment_id: Option<i32>,
    /// Acquisition cycle (`CycleNumber`; -1 when not set).
    pub cycle_number: Option<i32>,
    /// Fragmentation class (`FragClass`): 0 none, 1 low energy, 2 high energy (all-ions).
    pub frag_class: Option<i32>,
    /// Retention time in minutes (`FrameScanTime`).
    pub scan_time_min: f64,
    /// Total ion current of the frame (`FrameTic`).
    pub tic: f64,
    /// Largest abundance in the frame (`FrameBaseAbund`).
    pub base_abundance: f64,
    /// Offset of the frame's calibration in `MSMassCal.bin` (0 = the default calibration).
    pub mass_cal_offset: Option<i64>,
    /// Drift-tube field, V/cm (`ImsField`).
    pub ims_field: Option<f64>,
    /// Drift-tube pressure (`ImsPressure`, Torr as observed).
    pub ims_pressure: Option<f64>,
    /// Drift-tube temperature, °C (`ImsTemperature`).
    pub ims_temperature: Option<f64>,
    /// Every field as stored, in the order of the frame layout.
    pub values: Vec<f64>,
}

/// Field list of `IMSFrame.bin` records from `IMSFrame.xsd` (`FrameRecordType`), with the
/// record length.
pub fn frame_layout_from_xsd(xsd: &str) -> Result<(Vec<Field>, usize), String> {
    let xsd = strip_comments(xsd);
    let mut v = Vec::new();
    flatten(&xsd, "FrameRecordType", "", 0, &mut v)
        .map_err(|e| e.replace("MSScan.xsd", "IMSFrame.xsd"))?;
    for need in ["FrameId", "FrameMethodId", "FrameScanTime"] {
        if !v.iter().any(|(n, _)| n == need) {
            return Err(format!("IMSFrame.xsd: frame record has no {need} field"));
        }
    }
    Ok(with_offsets(v))
}

/// Read every record of `IMSFrame.bin` (tag 0x0116; i32 at 0x48 = offset of record 0).
/// Returns the frames and the number of bytes after the last whole record.
pub fn read_frames(
    b: &[u8],
    fields: &[Field],
    rec_len: usize,
) -> Result<(Vec<FrameRecord>, usize), String> {
    let tag = le_u16(b, 0).ok_or("IMSFrame.bin is shorter than its tag")?;
    if tag != TAG_IMS_FRAME {
        return Err(format!(
            "IMSFrame.bin starts with tag {tag:#06x}, not 0x0116"
        ));
    }
    let first = le_i32(b, 0x48).ok_or("IMSFrame.bin header is truncated")?;
    let first = usize::try_from(first)
        .ok()
        .filter(|&f| f >= 0x4C && f <= b.len())
        .ok_or_else(|| format!("IMSFrame.bin first record offset {first} is outside the file"))?;
    if rec_len == 0 {
        return Err("IMSFrame.xsd declares an empty record".into());
    }
    let n = (b.len() - first) / rec_len;
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let at = first + k * rec_len;
        let rec = &b[at..at + rec_len];
        let values: Vec<f64> = fields
            .iter()
            .map(|f| f.ty.read(rec, f.offset).unwrap_or(f64::NAN))
            .collect();
        let get = |name: &str| {
            fields
                .iter()
                .position(|f| f.name == name)
                .and_then(|i| values.get(i).copied())
                .filter(|v| v.is_finite())
        };
        let int = |name: &str| get(name).map(|v| v as i32);
        out.push(FrameRecord {
            frame_id: int("FrameId").unwrap_or(0),
            method_id: int("FrameMethodId").unwrap_or(0),
            time_segment_id: int("TimeSegmentId"),
            cycle_number: int("CycleNumber").filter(|&c| c >= 0),
            frag_class: int("FragClass"),
            scan_time_min: get("FrameScanTime").unwrap_or(0.0),
            tic: get("FrameTic").unwrap_or(0.0),
            base_abundance: get("FrameBaseAbund").unwrap_or(0.0),
            mass_cal_offset: get("MassCalOffset").map(|v| v as i64).filter(|&o| o > 0),
            ims_field: get("ImsField"),
            ims_pressure: get("ImsPressure"),
            ims_temperature: get("ImsTemperature"),
            values,
        });
    }
    Ok((out, (b.len() - first) % rec_len))
}

/// One `<FrameMethod>` of `IMSFrameMeth.xml`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameMethod {
    /// `FrameMethId` attribute.
    pub id: i32,
    /// Drift-bin width, ms (`FrameDtPeriod`).
    pub drift_period_ms: Option<f64>,
    /// Flight-time bin width, ns (`FrameMsXPeriod`).
    pub ms_period_ns: Option<f64>,
    /// First flight-time bin of the frame grid (`MinMsBin`).
    pub min_ms_bin: Option<i64>,
    /// Default calibration used (`DefMassCalId`).
    pub mass_cal_id: Option<i32>,
    /// Ion polarity code (`IonPolarity`: 0 positive, 1 negative).
    pub ion_polarity: Option<i32>,
    /// Ionization-mode code (`IonizationMode`).
    pub ionization_mode: Option<i32>,
    /// Fragmentation mode code (`FragOpMode`: 1 none, 8 high/low alternating, …).
    pub frag_op_mode: Option<i32>,
    /// Fixed fragmentation energy, V (`FragEnergy`).
    pub frag_energy: Option<f64>,
    /// Collision-energy ramp as stored: `(db, e)` of every `<EndPoint>` in `FragEnergySegments`.
    pub frag_energy_ramp: Vec<(f64, f64)>,
}

/// Parse the frame methods of `IMSFrameMeth.xml`.
pub fn frame_methods(xml: &str) -> Vec<FrameMethod> {
    let xml = strip_comments(xml);
    let mut out = Vec::new();
    let mut rest = xml.as_str();
    while let Some((body, after)) = between(rest, "<FrameMethod ", "</FrameMethod>") {
        rest = after;
        let tag_end = body.find('>').unwrap_or(body.len());
        let Some(id) = attr(&format!("<{}", &body[..tag_end]), "FrameMethId")
            .and_then(|v| v.trim().parse().ok())
        else {
            continue;
        };
        let num = |t: &str| {
            between(body, &format!("<{t}>"), &format!("</{t}>"))
                .and_then(|(v, _)| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
        };
        let mut ramp = Vec::new();
        if let Some((seg, _)) = between(body, "<FragEnergySegments>", "</FragEnergySegments>") {
            let mut r = seg;
            while let Some(i) = r.find("<EndPoint") {
                let tail = &r[i..];
                let end = tail.find('>').unwrap_or(tail.len());
                let tag = &tail[..end];
                if let (Some(db), Some(e)) = (
                    attr(tag, "db").and_then(|v| v.trim().parse().ok()),
                    attr(tag, "e").and_then(|v| v.trim().parse().ok()),
                ) {
                    ramp.push((db, e));
                }
                r = &tail[end..];
            }
        }
        out.push(FrameMethod {
            id,
            drift_period_ms: num("FrameDtPeriod").filter(|&v| v > 0.0),
            ms_period_ns: num("FrameMsXPeriod").filter(|&v| v > 0.0),
            min_ms_bin: num("MinMsBin").map(|v| v as i64),
            mass_cal_id: num("DefMassCalId").map(|v| v as i32),
            ion_polarity: num("IonPolarity").map(|v| v as i32),
            ionization_mode: num("IonizationMode").map(|v| v as i32),
            frag_op_mode: num("FragOpMode").map(|v| v as i32),
            frag_energy: num("FragEnergy"),
            frag_energy_ramp: ramp,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------
// Calibration
// ---------------------------------------------------------------------------------------

/// Flight time → m/z: `m = (a·(t − t0))² − Σ cₖ·clamp(t, lo, hi)^pₖ`, the powers `pₖ` being
/// the set bits of the polynomial step's use flags (see the format notes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Calibration {
    /// Traditional step: scale `a`.
    pub a: f64,
    /// Traditional step: time offset `t0` (ns).
    pub t0: f64,
    /// Polynomial step: flight-time range `[lo, hi]` the correction is evaluated in.
    pub range: Option<[f64; 2]>,
    /// Polynomial step: `(power, coefficient)` pairs.
    pub terms: Vec<(u32, f64)>,
}

impl Calibration {
    /// Build from the stored coefficient list (`[a, t0, lo, hi, c1, …]`) and the polynomial
    /// step's use flags.
    pub fn from_values(values: &[f64], flags: u32) -> Option<Self> {
        let (&a, &t0) = (values.first()?, values.get(1)?);
        let mut c = Calibration {
            a,
            t0,
            range: None,
            terms: Vec::new(),
        };
        if values.len() >= 4 {
            c.range = Some([values[2], values[3]]);
            let powers = (0..32).filter(|k| flags >> k & 1 == 1);
            c.terms = powers.zip(values[4..].iter().copied()).collect();
        }
        Some(c)
    }
    /// m/z of flight time `t`.
    pub fn mz(&self, t: f64) -> f64 {
        let r = self.a * (t - self.t0);
        let mut m = r * r;
        if let Some([lo, hi]) = self.range {
            let tc = t.clamp(lo.min(hi), hi.max(lo));
            // Sum the correction first, then subtract it once: this order reproduces the
            // depositors' float64 exports bit for bit.
            let mut corr = 0.0;
            for &(p, c) in &self.terms {
                corr += c * tc.powf(f64::from(p));
            }
            m -= corr;
        }
        m
    }
}

/// Read the calibration record at `offset` in `MSMassCal.bin`: i32 count, then that many f64.
pub fn mass_cal_values(b: &[u8], offset: usize) -> Option<Vec<f64>> {
    let n = le_i32(b, offset)?;
    if !(1..=64).contains(&n) {
        return None;
    }
    (0..n as usize)
        .map(|k| le_f64(b, offset + 4 + 8 * k))
        .collect()
}

/// One `DefaultCalibration` of `DefaultMassCal.xml`: its id, all step values in order, the
/// polynomial step's use flags and each step's formula name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefaultCalibration {
    /// `DefaultCalibrationID`, referenced by the scans' `CalibrationID`.
    pub id: i32,
    /// Values of all steps, concatenated in step order.
    pub values: Vec<f64>,
    /// Use flags of the last step that has any.
    pub flags: u32,
    /// `CalibrationFormula` of each step.
    pub formulas: Vec<String>,
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<(&'a str, &'a str)> {
    let i = s.find(open)? + open.len();
    let j = s[i..].find(close)?;
    Some((&s[i..i + j], &s[i + j + close.len()..]))
}

/// Parse `DefaultMassCal.xml`.
pub fn default_calibrations(xml: &str) -> Vec<DefaultCalibration> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<DefaultCalibration ") {
        let tail = &rest[i..];
        let tag_end = tail.find('>').unwrap_or(tail.len());
        let id = attr(&tail[..tag_end], "DefaultCalibrationID").and_then(|v| v.trim().parse().ok());
        let end = tail.find("</DefaultCalibration>").unwrap_or(tail.len());
        let body = &tail[..end];
        let mut cal = DefaultCalibration {
            id: id.unwrap_or(0),
            ..Default::default()
        };
        let mut steps = body;
        while let Some((step, after)) = between(steps, "<Step", "</Step>") {
            if let Some((f, _)) = between(step, "<CalibrationFormula>", "</CalibrationFormula>") {
                cal.formulas.push(f.trim().to_string());
            }
            if let Some((f, _)) = between(step, "<ValueUseFlags>", "</ValueUseFlags>")
                && let Ok(v) = f.trim().parse::<u32>()
                && v != 0
            {
                cal.flags = v;
            }
            let mut vals = step;
            while let Some((v, after_v)) = between(vals, "<Value ", "</Value>") {
                let text = v.split_once('>').map_or("", |(_, t)| t);
                cal.values.push(text.trim().parse().unwrap_or(0.0));
                vals = after_v;
            }
            steps = after;
        }
        if id.is_some() {
            out.push(cal);
        }
        rest = &tail[end.min(tail.len())..];
        if end == tail.len() {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Device signals: *.cd (descriptor) + *.cg (data)
// ---------------------------------------------------------------------------------------

/// One signal listed in a device's `.cd` file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignalDef {
    /// Signal letter (`A`, `B`, …) or empty.
    pub id: String,
    /// Description, e.g. `Sig=254.0,4.0  Ref=off` or ` Pressure`.
    pub description: String,
    /// 1 = detector signal, 2 = instrument reading (as observed).
    pub kind: i32,
    /// Byte offset of the signal's block in the `.cg` file.
    pub offset: i64,
    /// Number of samples.
    pub count: i32,
    /// Unit of the values (`mAU`, `bar`, `°C`, …).
    pub unit: String,
    /// Scale factor recorded with the unit.
    pub scale: f64,
}

/// A `.cd` file: the device id and its signals.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignalDirectory {
    /// Device number (matches `DeviceID` in `Devices.xml`).
    pub device_id: i32,
    /// Signals in file order.
    pub signals: Vec<SignalDef>,
}

fn pstr(b: &[u8], at: &mut usize) -> Option<String> {
    let n = *b.get(*at)? as usize;
    let s = b.get(*at + 1..*at + 1 + n)?;
    *at += 1 + n;
    Some(String::from_utf8_lossy(s).into_owned())
}

/// Parse a `.cd` signal descriptor.
pub fn signal_directory(b: &[u8]) -> Result<SignalDirectory, String> {
    let mut at = DATA_START;
    let _first = le_i32(b, at).ok_or("signal descriptor is truncated")?;
    let device_id = le_i32(b, at + 4).ok_or("signal descriptor is truncated")?;
    let n = le_i32(b, at + 8).ok_or("signal descriptor is truncated")?;
    if !(0..=4096).contains(&n) {
        return Err(format!("signal descriptor lists {n} signals"));
    }
    at += 12;
    let mut signals = Vec::with_capacity(n as usize);
    for k in 0..n {
        let bad = || format!("signal descriptor entry {k} is truncated");
        let id = pstr(b, &mut at).ok_or_else(bad)?;
        let description = pstr(b, &mut at).ok_or_else(bad)?;
        let kind = le_i32(b, at).ok_or_else(bad)?;
        let offset = le_i64(b, at + 4).ok_or_else(bad)?;
        let count = le_i32(b, at + 12).ok_or_else(bad)?;
        at += 16 + 40; // four words, a double, four words (constant in every corpus file)
        let unit = pstr(b, &mut at).ok_or_else(bad)?;
        let scale = le_f64(b, at).ok_or_else(bad)?;
        at += 16;
        signals.push(SignalDef {
            id,
            description,
            kind,
            offset,
            count,
            unit,
            scale,
        });
    }
    Ok(SignalDirectory { device_id, signals })
}

/// Read a signal block from a `.cg` file: f64 start time (min), f64 interval (min), then
/// `count` f64 values.
pub fn signal_block(cg: &[u8], def: &SignalDef) -> Option<(f64, f64, Vec<f64>)> {
    let off = usize::try_from(def.offset).ok()?;
    let n = usize::try_from(def.count).ok()?;
    let start = le_f64(cg, off)?;
    let step = le_f64(cg, off + 8)?;
    let end = off.checked_add(16)?.checked_add(n.checked_mul(8)?)?;
    if end > cg.len() {
        return None;
    }
    let v = (0..n)
        .map(|k| le_f64(cg, off + 16 + 8 * k).unwrap_or(0.0))
        .collect();
    Some((start, step, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzf_literal_and_backref() {
        // "abcabcabc": literal "abc", then a back-reference of 6 bytes at distance 3.
        let src = [2u8, b'a', b'b', b'c', (4 << 5) as u8, 2];
        assert_eq!(lzf_decompress(&src, 64).unwrap(), b"abcabcabc");
        assert!(lzf_decompress(&[0x20, 5], 64).is_err());
        assert!(lzf_decompress(&src, 4).is_err());
    }

    #[test]
    fn layouts_from_fallback_schemas() {
        let v6 = scan_layout_from_xsd(FALLBACK_XSD_V6).unwrap();
        assert_eq!(v6.fixed_len, 156);
        assert_eq!(v6.block_len, 64);
        let v5 = scan_layout_from_xsd(FALLBACK_XSD_V5).unwrap();
        assert_eq!(v5.record_len(1), 196);
        assert_eq!(v5.field("MzOfInterest").unwrap().offset, 84);
    }

    /// Grid header, then the header word and the first bin.
    fn ims_head(npts: u32, first_bin: i32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&18465.0f64.to_le_bytes());
        b.extend_from_slice(&0.5f64.to_le_bytes());
        b.extend_from_slice(&((IMS_PROFILE_FLAGS << 24) | npts).to_le_bytes());
        b.extend_from_slice(&(-first_bin).to_le_bytes());
        b
    }

    #[test]
    fn ims_profile_widths_and_skips() {
        // Start at bin 10 (4-byte mode); -1 = no skip, 1-byte mode; counts 1, 2; 0x83 skips 31
        // bins (1-byte); 0xfe = no skip, 2-byte mode; 300; then 0xffff (2-byte -1 → 1-byte).
        let mut b = ims_head(100, 10);
        b.extend_from_slice(&(-1i32).to_le_bytes());
        b.extend_from_slice(&[1, 2, 0x83, 0xfe]);
        b.extend_from_slice(&300i16.to_le_bytes());
        b.extend_from_slice(&(-1i16).to_le_bytes());
        b.push(5);
        let p = decode_ims_profile(&b).unwrap();
        assert_eq!(p.point_count, 100);
        assert_eq!(p.bins, vec![(10, 1), (11, 2), (43, 300), (44, 5)]);
        // one value in 4-byte mode straight after the start
        let mut one = ims_head(24960, 24959);
        one.extend_from_slice(&1i32.to_le_bytes());
        assert_eq!(decode_ims_profile(&one).unwrap().bins, vec![(24959, 1)]);
        // a bin past the grid, a cut value and an unseen header byte are errors
        let mut past = ims_head(5, 4);
        past.extend_from_slice(&(-1i32).to_le_bytes());
        past.extend_from_slice(&[1, 1]);
        assert!(decode_ims_profile(&past).is_err());
        let mut cut = ims_head(100, 0);
        cut.extend_from_slice(&[1, 0]);
        assert!(decode_ims_profile(&cut).is_err());
        let mut other = ims_head(100, 0);
        other[19] = 0x80;
        assert!(decode_ims_profile(&other).is_err());
    }

    #[test]
    fn quad_point_lists_are_planar() {
        let mut b = Vec::new();
        for x in [126.8f32, 574.8, 775.7] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        for y in [12i32, 7, 31] {
            b.extend_from_slice(&y.to_le_bytes());
        }
        let d = decode_peaks(&b, 3).unwrap();
        assert_eq!(d.y, vec![12.0, 7.0, 31.0]);
        assert!((d.x[2] - 775.7).abs() < 1e-4);
    }

    #[test]
    fn frame_methods_and_ramp() {
        let xml = r#"<FrameMethods><FrameMethod FrameMethId="2"><DefMassCalId>1</DefMassCalId>
<FragEnergySegments><EndPoint db="0" e="20" /><EndPoint db="280" e="30" /></FragEnergySegments>
<!--milliseconds--><FrameDtPeriod>0.169491525423729</FrameDtPeriod><FrameMsXPeriod>0.5</FrameMsXPeriod>
<IonPolarity>0</IonPolarity><MinMsBin>36930</MinMsBin></FrameMethod></FrameMethods>"#;
        let m = frame_methods(xml);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, 2);
        assert_eq!(m[0].min_ms_bin, Some(36930));
        assert_eq!(m[0].frag_energy_ramp, vec![(0.0, 20.0), (280.0, 30.0)]);
        assert!((m[0].drift_period_ms.unwrap() - 0.169_491_525_423_729).abs() < 1e-15);
    }

    #[test]
    fn calibration_clamps_the_polynomial() {
        let c = Calibration::from_values(&[0.001, 1000.0, 2000.0, 3000.0, 1e-4], 0b10).unwrap();
        let at = |t: f64| (0.001 * (t - 1000.0)).powi(2);
        assert!((c.mz(2500.0) - (at(2500.0) - 1e-4 * 2500f64.powi(1))).abs() < 1e-12);
        assert!((c.mz(5000.0) - (at(5000.0) - 1e-4 * 3000.0)).abs() < 1e-12);
    }
}

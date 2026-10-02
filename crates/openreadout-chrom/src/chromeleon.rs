//! Thermo Scientific Chromeleon 7 archives (`.cmbx`): the parsers. An archive is a zip of
//! `header.xml` (the tree of archived items: sequence, injections, their signals and audit
//! trails, methods), the sequence file `<name>.seq_<n>.cmd` (a protocol-buffers stream; per
//! signal a description with its unit, range, point count, device and scale factor) and one
//! `<n>_<id>.raw` member per signal. A 2D signal member is a run of sections, each an 8-byte tag
//! and a u64 length: `SignHdr` (signal name, acquisition start) and `PtsLDiff` blocks of
//! difference-coded integers. Layout and evidence: `docs/formats/chromeleon.md`,
//! `docs/provenance/chromeleon.md`.

use openreadout_core::bytes::{le_u32, le_u64};

/// Signal members larger than this are refused (a 2D signal of a whole run is well under 1 MiB
/// in every corpus file).
pub const MAX_SIGNAL_BYTES: u64 = 256 << 20;
/// Largest sequence file (`.cmd`) read.
pub const MAX_SEQUENCE_BYTES: u64 = 256 << 20;
/// Largest `header.xml` read.
pub const MAX_HEADER_BYTES: u64 = 64 << 20;
/// Most points one signal may decode to (about 3.9 days at 100 Hz).
pub const MAX_SIGNAL_POINTS: usize = 1 << 25;

/// Section tag of the signal header.
pub const SIGNAL_HEADER_TAG: &[u8; 8] = b"SignHdr\0";
/// Section tag of a block of difference-coded points.
pub const POINTS_TAG: &[u8; 8] = b"PtsLDiff";
/// The only block version seen (and decoded).
pub const POINTS_VERSION: u64 = 1;

/// Item type of a 2D signal (`GC_1`, `ED_1`, `Pump_1_Pressure`).
pub const SIGNAL_ITEM: &str = "Dionex.Chromeleon.Data.Signal";
/// Item type of an injection.
pub const INJECTION_ITEM: &str = "Dionex.Chromeleon.Data.Injection";
/// Item type of a sequence.
pub const SEQUENCE_ITEM: &str = "Dionex.Chromeleon.Data.Sequence";
/// Item type of a processing method (it may hold calibration standards' signals).
pub const PROCESSING_METHOD_ITEM: &str = "Dionex.Chromeleon.Data.ProcessingMethod";
/// Item type of 3D data (a spectral field, `.sfd` member).
pub const SPECTRAL_FIELD_ITEM: &str = "Dionex.Chromeleon.Data.SpectralField";
/// Item type of an injection's audit trail (a compressed `CpAu` member).
pub const AUDIT_TRAIL_ITEM: &str = "Dionex.Chromeleon.Data.AuditTrail";
/// Item type of mass-spectrometry data: an embedded Thermo `.raw` file.
pub const MS_RAW_ITEM: &str = "Dionex.Chromeleon.Data.MSRawItem";

/// One `ChromeleonElement` of `header.xml`, flattened in document order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchiveItem {
    /// `Id`.
    pub id: String,
    /// `Name` (XML entities resolved).
    pub name: String,
    /// `ItemType`, e.g. `Dionex.Chromeleon.Data.Signal`.
    pub item_type: String,
    /// `Url` (`chrom://<host>/<data vault>/...`).
    pub url: Option<String>,
    /// The archive member holding the item's data (`RawDataFilename`, or `Filename` for the
    /// sequence).
    pub member: Option<String>,
    /// `Size` in bytes.
    pub size: Option<u64>,
    /// `RawDataFileId` (`2025\11\16\164901056.raw`): the key into the sequence file.
    pub file_id: Option<String>,
    /// `InjectionType` (`Unknown`, `Blank`, `Standard`, ...).
    pub injection_type: Option<String>,
    /// Index of the enclosing item, if any.
    pub parent: Option<usize>,
    /// For a signal a processing method holds as a calibration standard
    /// (`FixedInjectionList/FixedInjection`): that injection's `FixedInjectionName`.
    pub fixed_injection: Option<String>,
}

/// `header.xml`: the archive's own attributes and its items.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchiveHeader {
    /// `GeneratorVersion`: the Chromeleon build that wrote the archive (`7.2.10.23925`).
    pub generator_version: Option<String>,
    /// `ContainerVersion` (`2.0`).
    pub container_version: Option<String>,
    /// `DateCreated`, as written (local wording, e.g. `Monday, 29 December 2025`).
    pub date_created: Option<String>,
    /// Items in document order.
    pub items: Vec<ArchiveItem>,
}

impl ArchiveHeader {
    /// Indices of the items of `item_type`.
    pub fn of_type<'a>(&'a self, item_type: &'a str) -> impl Iterator<Item = usize> + 'a {
        self.items
            .iter()
            .enumerate()
            .filter(move |(_, it)| it.item_type == item_type)
            .map(|(i, _)| i)
    }

    /// The nearest enclosing item of `item_type`.
    pub fn ancestor(&self, mut at: usize, item_type: &str) -> Option<usize> {
        let mut guard = 0usize;
        while let Some(p) = self.items.get(at).and_then(|it| it.parent) {
            if self
                .items
                .get(p)
                .is_some_and(|it| it.item_type == item_type)
            {
                return Some(p);
            }
            at = p;
            guard += 1;
            if guard > self.items.len() {
                return None;
            }
        }
        None
    }
}

/// Parse `header.xml`.
///
/// # Errors
/// Not XML, or the root is not `ChromeleonHeader`.
pub fn parse_header(text: &str) -> Result<ArchiveHeader, String> {
    let doc = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 4_000_000,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| format!("header.xml: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "ChromeleonHeader" {
        return Err(format!(
            "header.xml: root element is <{}>, not <ChromeleonHeader>",
            root.tag_name().name()
        ));
    }
    let attr = |n: roxmltree::Node, a: &str| n.attribute(a).map(str::to_string);
    let mut out = ArchiveHeader {
        generator_version: attr(root, "GeneratorVersion"),
        container_version: attr(root, "ContainerVersion"),
        date_created: attr(root, "DateCreated"),
        items: Vec::new(),
    };
    // depth-first, parents before children; other elements (a processing method's
    // `FixedInjectionList/FixedInjection`) are walked through
    let mut stack: Vec<(roxmltree::Node, Option<usize>, Option<String>)> = root
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|c| (c, None, None))
        .collect();
    stack.reverse();
    while let Some((node, parent, fixed)) = stack.pop() {
        if !node.has_tag_name("ChromeleonElement") {
            let fixed = if node.has_tag_name("FixedInjection") {
                attr(node, "FixedInjectionName").or(fixed)
            } else {
                fixed
            };
            let mut kids: Vec<_> = node
                .children()
                .filter(roxmltree::Node::is_element)
                .map(|c| (c, parent, fixed.clone()))
                .collect();
            kids.reverse();
            stack.extend(kids);
            continue;
        }
        let at = out.items.len();
        out.items.push(ArchiveItem {
            id: attr(node, "Id").unwrap_or_default(),
            name: attr(node, "Name").unwrap_or_default(),
            item_type: attr(node, "ItemType").unwrap_or_default(),
            url: attr(node, "Url"),
            member: attr(node, "RawDataFilename").or_else(|| attr(node, "Filename")),
            size: node.attribute("Size").and_then(|s| s.trim().parse().ok()),
            file_id: attr(node, "RawDataFileId"),
            injection_type: attr(node, "InjectionType"),
            parent,
            fixed_injection: fixed.clone(),
        });
        let mut kids: Vec<_> = node
            .children()
            .filter(roxmltree::Node::is_element)
            .map(|c| (c, Some(at), fixed.clone()))
            .collect();
        kids.reverse();
        stack.extend(kids);
    }
    Ok(out)
}

/// The `SignHdr` section of a signal member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalHeader {
    /// Signal name as stored (equals the `header.xml` name).
    pub name: String,
    /// Acquisition start of the signal, a Windows FILETIME (100 ns ticks since 1601, UTC).
    pub start_filetime: u64,
}

/// A decoded 2D signal.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedSignal {
    /// The `SignHdr` section, when its layout is the one seen (else `None`).
    pub header: Option<SignalHeader>,
    /// The point sections' encoding: `PtsLDiff`, `PtsLL2Df` or `PtsDDCmp`.
    pub encoding: &'static str,
    /// Time of the first point, minutes.
    pub start_min: f64,
    /// Points per minute, when the points are on a regular grid (0 otherwise: see `times`).
    pub points_per_min: f64,
    /// Scale from stored integers to the unit (integer encodings; 1 for `PtsDDCmp`).
    pub scale: f64,
    /// `PtsLDiff` scale as stored: numerator and denominator (0/0 for other encodings).
    pub scale_num: u64,
    /// See `scale_num`.
    pub scale_den: u64,
    /// The stored integers (integer encodings), one per point.
    pub raw: Vec<i64>,
    /// Values stored as floating point (`PtsDDCmp`), one per point.
    pub float_values: Option<Vec<f64>>,
    /// Times in minutes, one per point, when they are not a regular grid.
    pub times: Option<Vec<f64>>,
    /// Number of point sections.
    pub blocks: usize,
}

impl DecodedSignal {
    /// value = raw × scale (the scale factor).
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// Number of points.
    pub fn len(&self) -> usize {
        self.float_values.as_ref().map_or(self.raw.len(), Vec::len)
    }

    /// No points.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Scaled values.
    pub fn values(&self) -> Vec<f64> {
        if let Some(v) = &self.float_values {
            return v.clone();
        }
        let k = self.scale;
        self.raw.iter().map(|&v| v as f64 * k).collect()
    }

    /// Minutes between points of a regular grid (0 when the times are irregular).
    pub fn step_min(&self) -> f64 {
        if self.points_per_min > 0.0 {
            1.0 / self.points_per_min
        } else {
            0.0
        }
    }

    /// Time of point `i`, minutes.
    pub fn time(&self, i: usize) -> f64 {
        match &self.times {
            Some(t) => t.get(i).copied().unwrap_or(f64::NAN),
            None => self.start_min + i as f64 * self.step_min(),
        }
    }
}

/// Why a signal member was not decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalError {
    /// The bytes break the section or block layout (truncated, bad lengths, bad integers).
    Corrupt(String),
    /// The layout is intact but uses something never validated (another section tag, block
    /// version, gaps between blocks, changing rates).
    Unsupported(String),
}

impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalError::Corrupt(s) | SignalError::Unsupported(s) => f.write_str(s),
        }
    }
}

/// One integer of a `PtsLDiff` block: the first byte holds a continuation bit (0x80), the sign
/// (0x40) and the 6 lowest magnitude bits; each continuation byte 7 more bits.
///
/// # Errors
/// Truncated, or more than 63 bits.
pub fn signed_varint(b: &[u8], at: &mut usize) -> Result<i64, String> {
    let first = *b.get(*at).ok_or("block ends inside an integer")?;
    *at += 1;
    let negative = first & 0x40 != 0;
    let mut mag: u64 = u64::from(first & 0x3F);
    let mut shift = 6u32;
    let mut more = first & 0x80 != 0;
    while more {
        let x = *b.get(*at).ok_or("block ends inside an integer")?;
        *at += 1;
        let chunk = u64::from(x & 0x7F);
        if shift >= 64 || (chunk << shift) >> shift != chunk {
            return Err("an integer longer than 64 bits".into());
        }
        mag |= chunk << shift;
        shift += 7;
        more = x & 0x80 != 0;
    }
    let v = i64::try_from(mag).map_err(|_| "an integer beyond 63 bits".to_string())?;
    Ok(if negative { -v } else { v })
}

fn parse_signal_header(body: &[u8]) -> Option<SignalHeader> {
    // 01 02 07 xx xx | FILETIME (8) | a small kind byte (2, 3, 5 seen) | name length | name |
    // GUID (16) | ...
    if body.get(..3)? != [1, 2, 7] || !(1..=15).contains(body.get(13)?) {
        return None;
    }
    let ft = le_u64(body, 5)?;
    let n = usize::from(*body.get(14)?);
    let name = std::str::from_utf8(body.get(15..15usize.checked_add(n)?)?).ok()?;
    // plausible: 2000-01-01 .. 2100-01-01
    if !(125_911_584_000_000_000..=157_469_184_000_000_000).contains(&ft) {
        return None;
    }
    Some(SignalHeader {
        name: name.to_string(),
        start_filetime: ft,
    })
}

/// Decode a 2D signal member: a `SignHdr` section and point sections of one encoding
/// (`PtsLDiff`, `PtsLL2Df` or `PtsDDCmp`).
///
/// # Errors
/// [`SignalError::Corrupt`] for broken sections or integers, [`SignalError::Unsupported`] for
/// section tags, encodings mixed in one member, or timing never validated.
pub fn decode_signal(b: &[u8]) -> Result<DecodedSignal, SignalError> {
    let corrupt = |m: String| SignalError::Corrupt(m);
    let mut off = 0usize;
    let mut header = None;
    let mut acc = Accumulator::default();
    while off < b.len() {
        let tag = b
            .get(off..off + 8)
            .ok_or_else(|| corrupt(format!("section at byte {off} is cut short")))?;
        let len = le_u64(b, off + 8)
            .ok_or_else(|| corrupt(format!("section at byte {off} is cut short")))?;
        let len = usize::try_from(len)
            .ok()
            .filter(|&l| l >= 16 && off.checked_add(l).is_some_and(|e| e <= b.len()))
            .ok_or_else(|| {
                corrupt(format!(
                    "section at byte {off} states {len} bytes; {} remain",
                    b.len() - off
                ))
            })?;
        let body = &b[off + 16..off + len];
        if tag == SIGNAL_HEADER_TAG {
            header = parse_signal_header(body);
        } else if tag == POINTS_TAG {
            acc.encoding(*POINTS_TAG, off)?;
            decode_block(body, off, &mut acc)?;
        } else if tag == LL2_TAG {
            acc.encoding(*LL2_TAG, off)?;
            decode_ll2_block(body, off, &mut acc)?;
        } else if tag == DDCMP_TAG {
            acc.encoding(*DDCMP_TAG, off)?;
            decode_ddcmp_block(body, off, &mut acc)?;
        } else {
            return Err(SignalError::Unsupported(format!(
                "section `{}` at byte {off} (only SignHdr, PtsLDiff, PtsLL2Df and PtsDDCmp are decoded)",
                String::from_utf8_lossy(tag).trim_end_matches('\0')
            )));
        }
        off += len;
    }
    let mut s = acc.finish()?;
    s.header = header;
    Ok(s)
}

/// Section tag of second-difference-coded (time, value) pairs.
pub const LL2_TAG: &[u8; 8] = b"PtsLL2Df";
/// Section tag of LZMA2-compressed (time, value) pairs of f64.
pub const DDCMP_TAG: &[u8; 8] = b"PtsDDCmp";

/// Points gathered over a member's sections.
#[derive(Debug, Default)]
struct Accumulator {
    tag: Option<[u8; 8]>,
    /// `PtsLDiff`: the signal so far.
    ldiff: Option<DecodedSignal>,
    /// `PtsLL2Df`: time ticks per minute and value units, times (ticks) and values.
    ll2_units: Option<(f64, f64)>,
    ticks: Vec<i64>,
    ints: Vec<i64>,
    /// `PtsDDCmp`: times and values.
    ftimes: Vec<f64>,
    fvalues: Vec<f64>,
    blocks: usize,
}

impl Accumulator {
    fn encoding(&mut self, tag: [u8; 8], off: usize) -> Result<(), SignalError> {
        match self.tag {
            Some(t) if t != tag => Err(SignalError::Unsupported(format!(
                "section `{}` at byte {off} after `{}` sections (encodings mixed in one signal)",
                String::from_utf8_lossy(&tag),
                String::from_utf8_lossy(&t)
            ))),
            _ => {
                self.tag = Some(tag);
                Ok(())
            }
        }
    }

    fn check_points(&self, more: usize, off: usize) -> Result<(), SignalError> {
        let have =
            self.ints.len() + self.fvalues.len() + self.ldiff.as_ref().map_or(0, |s| s.raw.len());
        if have.saturating_add(more) > MAX_SIGNAL_POINTS {
            return Err(SignalError::Corrupt(format!(
                "block at byte {off}: more than {MAX_SIGNAL_POINTS} points"
            )));
        }
        Ok(())
    }

    fn finish(self) -> Result<DecodedSignal, SignalError> {
        let corrupt = |m: &str| SignalError::Corrupt(m.to_string());
        match self.tag {
            None => Err(corrupt("no point section (PtsLDiff, PtsLL2Df, PtsDDCmp)")),
            Some(t) if &t == POINTS_TAG => self.ldiff.ok_or_else(|| corrupt("no PtsLDiff block")),
            Some(t) if &t == LL2_TAG => {
                let (per_min, units) =
                    self.ll2_units.ok_or_else(|| corrupt("no PtsLL2Df block"))?;
                if self.ticks.windows(2).any(|w| w[1] <= w[0]) {
                    return Err(corrupt("PtsLL2Df times do not increase"));
                }
                let times: Vec<f64> = self.ticks.iter().map(|&t| t as f64 / per_min).collect();
                let regular = self.ticks.len() > 1
                    && self
                        .ticks
                        .windows(2)
                        .all(|w| w[1] - w[0] == self.ticks[1] - self.ticks[0]);
                Ok(DecodedSignal {
                    header: None,
                    encoding: "PtsLL2Df",
                    start_min: times.first().copied().unwrap_or(0.0),
                    points_per_min: if regular {
                        per_min / (self.ticks[1] - self.ticks[0]) as f64
                    } else {
                        0.0
                    },
                    scale: 1.0 / units,
                    scale_num: 0,
                    scale_den: 0,
                    raw: self.ints,
                    float_values: None,
                    times: (!regular).then_some(times),
                    blocks: self.blocks,
                })
            }
            Some(_) => {
                let t = &self.ftimes;
                if t.windows(2).any(|w| w[1] <= w[0]) {
                    return Err(corrupt("PtsDDCmp times do not increase"));
                }
                let n = t.len();
                let step = if n > 1 {
                    (t[n - 1] - t[0]) / (n - 1) as f64
                } else {
                    0.0
                };
                let regular = n > 1
                    && t.iter().enumerate().all(|(i, &x)| {
                        (x - (t[0] + i as f64 * step)).abs() <= 1e-9 * x.abs().max(1.0)
                    });
                Ok(DecodedSignal {
                    header: None,
                    encoding: "PtsDDCmp",
                    start_min: t.first().copied().unwrap_or(0.0),
                    points_per_min: if regular { 1.0 / step } else { 0.0 },
                    scale: 1.0,
                    scale_num: 0,
                    scale_den: 0,
                    raw: Vec::new(),
                    float_values: Some(self.fvalues),
                    times: (!regular).then(|| self.ftimes.clone()),
                    blocks: self.blocks,
                })
            }
        }
    }
}

fn decode_block(body: &[u8], off: usize, acc: &mut Accumulator) -> Result<(), SignalError> {
    let corrupt = |m: String| SignalError::Corrupt(format!("block at byte {off}: {m}"));
    let t0 = le_u64(body, 0)
        .map(f64::from_bits)
        .filter(|t| t.is_finite())
        .ok_or_else(|| corrupt("no start time".into()))?;
    let mut at = 8usize;
    let mut head = [0i64; 4];
    for h in &mut head {
        *h = signed_varint(body, &mut at).map_err(corrupt)?;
    }
    // points per minute = rate_num / rate_den; value = integer × num / den
    let [rate_den, rate_num, num, den] = head;
    let (Ok(rate_den), Ok(rate_num), Ok(num), Ok(den)) = (
        u64::try_from(rate_den),
        u64::try_from(rate_num),
        u64::try_from(num),
        u64::try_from(den),
    ) else {
        return Err(corrupt("negative rate or scale".into()));
    };
    if rate_num == 0 || rate_den == 0 || den == 0 {
        return Err(corrupt("zero rate or scale denominator".into()));
    }
    let mut diffs = Vec::new();
    while at < body.len() {
        diffs.push(signed_varint(body, &mut at).map_err(corrupt)?);
    }
    // every block ends with a 0 after its last difference
    if diffs.pop() != Some(0) {
        return Err(corrupt(
            "the block does not end with its 0 terminator".into(),
        ));
    }
    acc.check_points(diffs.len(), off)?;
    acc.blocks += 1;
    let ppm = rate_num as f64 / rate_den as f64;
    let s = acc.ldiff.get_or_insert_with(|| DecodedSignal {
        header: None,
        encoding: "PtsLDiff",
        start_min: t0,
        points_per_min: ppm,
        scale: num as f64 / den as f64,
        scale_num: num,
        scale_den: den,
        raw: Vec::new(),
        float_values: None,
        times: None,
        blocks: 0,
    });
    // every block must state the first block's rate exactly
    if s.points_per_min.to_bits() != ppm.to_bits() || (s.scale_num, s.scale_den) != (num, den) {
        return Err(SignalError::Unsupported(format!(
            "block at byte {off} changes the rate or scale ({rate_num}/{rate_den} per min, {num}/{den}; first block {} per min, {}/{})",
            s.points_per_min, s.scale_num, s.scale_den
        )));
    }
    // blocks follow each other without gaps: t0 = start + points so far / rate
    let expect = s.start_min + s.raw.len() as f64 / ppm;
    if ((t0 - expect) * ppm).abs() > 1e-3 {
        return Err(SignalError::Unsupported(format!(
            "block at byte {off} starts at {t0} min, not {expect} min after the points before it (gaps are not decoded)"
        )));
    }
    // the first value of each block is absolute, the others differences
    let mut sum: i64 = 0;
    for d in diffs {
        sum = sum
            .checked_add(d)
            .ok_or_else(|| corrupt("the summed values overflow 64 bits".into()))?;
        s.raw.push(sum);
    }
    s.blocks += 1;
    Ok(())
}

/// `PtsLL2Df`: f64 time ticks per minute, f64 value units per unit, then (time, value) integer
/// pairs: the first absolute, the second first differences, the rest second differences; a
/// final 0.
fn decode_ll2_block(body: &[u8], off: usize, acc: &mut Accumulator) -> Result<(), SignalError> {
    let corrupt = |m: String| SignalError::Corrupt(format!("block at byte {off}: {m}"));
    let f = |at| {
        le_u64(body, at)
            .map(f64::from_bits)
            .filter(|x| x.is_finite() && *x > 0.0)
    };
    let (per_min, units) = f(0)
        .zip(f(8))
        .ok_or_else(|| corrupt("no positive time and value units".into()))?;
    match acc.ll2_units {
        Some(u) if u != (per_min, units) => {
            return Err(SignalError::Unsupported(format!(
                "block at byte {off} changes the time or value units ({per_min}, {units}; first block {}, {})",
                u.0, u.1
            )));
        }
        _ => acc.ll2_units = Some((per_min, units)),
    }
    let mut at = 16usize;
    let mut ints = Vec::new();
    while at < body.len() {
        ints.push(signed_varint(body, &mut at).map_err(corrupt)?);
    }
    if ints.pop() != Some(0) || ints.len() % 2 != 0 {
        return Err(corrupt(
            "the block does not hold (time, value) pairs and its 0 terminator".into(),
        ));
    }
    acc.check_points(ints.len() / 2, off)?;
    acc.blocks += 1;
    let add = |a: i64, b: i64| {
        a.checked_add(b)
            .ok_or_else(|| corrupt("values overflow 64 bits".into()))
    };
    let (mut t, mut y, mut dt, mut dy) = (0i64, 0i64, 0i64, 0i64);
    for (k, p) in ints.as_chunks::<2>().0.iter().enumerate() {
        match k {
            0 => (t, y) = (p[0], p[1]),
            1 => {
                (dt, dy) = (p[0], p[1]);
                t = add(t, dt)?;
                y = add(y, dy)?;
            }
            _ => {
                dt = add(dt, p[0])?;
                dy = add(dy, p[1])?;
                t = add(t, dt)?;
                y = add(y, dy)?;
            }
        }
        acc.ticks.push(t);
        acc.ints.push(y);
    }
    Ok(())
}

/// `PtsDDCmp`: integers 1, 0, the decompressed and compressed lengths; an LZMA2 dictionary
/// byte and stream decompressing to (time min, value) pairs of little-endian f64.
fn decode_ddcmp_block(body: &[u8], off: usize, acc: &mut Accumulator) -> Result<(), SignalError> {
    let corrupt = |m: String| SignalError::Corrupt(format!("block at byte {off}: {m}"));
    let mut at = 0usize;
    let mut head = [0i64; 4];
    for h in &mut head {
        *h = signed_varint(body, &mut at).map_err(corrupt)?;
    }
    let [a, b, unpacked, packed] = head;
    if (a, b) != (1, 0) {
        return Err(SignalError::Unsupported(format!(
            "PtsDDCmp block at byte {off} starts {a}, {b} (only 1, 0 is decoded)"
        )));
    }
    let (Ok(unpacked), Ok(packed)) = (usize::try_from(unpacked), usize::try_from(packed)) else {
        return Err(corrupt("negative lengths".into()));
    };
    if packed != body.len() - at || unpacked % 16 != 0 || packed < 2 {
        return Err(corrupt(format!(
            "states {packed} compressed bytes ({} remain) and {unpacked} decompressed",
            body.len() - at
        )));
    }
    acc.check_points(unpacked / 16, off)?;
    if body[at] > 40 {
        return Err(corrupt("dictionary byte out of range".into()));
    }
    let (out, _) = openreadout_codecs::lzma2_decode(&body[at + 1..], unpacked)
        .map_err(|e| corrupt(e.to_string()))?;
    for p in out.as_chunks::<16>().0 {
        let (t, v) = p.split_at(8);
        let t = f64::from_le_bytes(t.try_into().map_err(|_| corrupt("pair".into()))?);
        let v = f64::from_le_bytes(v.try_into().map_err(|_| corrupt("pair".into()))?);
        if !(t.is_finite() && v.is_finite()) {
            return Err(corrupt("a non-finite time or value".into()));
        }
        acc.ftimes.push(t);
        acc.fvalues.push(v);
    }
    acc.blocks += 1;
    Ok(())
}

/// Magic of a spectral field member (`.sfd`, 3D PDA data).
pub const FIELD_MAGIC: &[u8; 8] = b"3DRawSpc";
/// Section tag of a spectrum's header inside a field record.
pub const SPECTRUM_HEADER_TAG: &[u8; 8] = b"RawSpHdr";

/// Where each spectrum of a field member is.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldIndex {
    /// The spectrum count the member's header states.
    pub stated: u32,
    /// Per spectrum: its time (minutes), and the byte range of its record's sections.
    pub records: Vec<(f64, usize, usize)>,
}

/// The wavelength grid and scale of a spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpectrumGrid {
    /// First wavelength (nm).
    pub x0: f64,
    /// Wavelength step = `step_num` / `step_den` (nm).
    pub step_num: u64,
    /// See `step_num`.
    pub step_den: u64,
    /// Value = integer × `scale_num` / `scale_den`.
    pub scale_num: u64,
    /// See `scale_num`.
    pub scale_den: u64,
    /// Points per spectrum.
    pub points: usize,
}

impl SpectrumGrid {
    /// Wavelength step (nm).
    pub fn step(&self) -> f64 {
        self.step_num as f64 / self.step_den as f64
    }
    /// Scale factor.
    pub fn scale(&self) -> f64 {
        self.scale_num as f64 / self.scale_den as f64
    }

    /// A stored integer as a value: (integer × numerator) / denominator, the order that gives
    /// Chromeleon's own extracted channels bit for bit.
    pub fn value(&self, raw: i64) -> f64 {
        (raw as f64 * self.scale_num as f64) / self.scale_den as f64
    }
}

/// Walk a field member's records (a prefix of the member walks the records it holds whole).
///
/// # Errors
/// Not a field member, or a header never seen ([`SignalError::Unsupported`]); a record running
/// past the end when `complete` ([`SignalError::Corrupt`]).
pub fn index_field(b: &[u8], complete: bool) -> Result<FieldIndex, SignalError> {
    if b.get(..8) != Some(FIELD_MAGIC) {
        return Err(SignalError::Corrupt(
            "not a spectral field (no 3DRawSpc magic)".into(),
        ));
    }
    let h = |at| le_u32(b, at).ok_or_else(|| SignalError::Corrupt("field header cut short".into()));
    let (version, zero, stated, eight) = (h(8)?, h(12)?, h(16)?, h(20)?);
    if (version, zero, eight) != (1, 0, 8) {
        return Err(SignalError::Unsupported(format!(
            "spectral field header {version}, {zero}, {eight} (only 1, 0, 8 is decoded)"
        )));
    }
    let mut records = Vec::new();
    let mut at = 24usize;
    while at < b.len() {
        let (Some(t), Some(len)) = (le_u64(b, at).map(f64::from_bits), le_u32(b, at + 8)) else {
            if complete {
                return Err(SignalError::Corrupt(format!(
                    "field record at byte {at} is cut short"
                )));
            }
            break;
        };
        let start = at + 12;
        let Some(end) = start.checked_add(len as usize).filter(|&e| e <= b.len()) else {
            if complete {
                return Err(SignalError::Corrupt(format!(
                    "field record at byte {at} states {len} bytes; fewer remain"
                )));
            }
            break;
        };
        if !t.is_finite() {
            return Err(SignalError::Corrupt(format!(
                "field record at byte {at}: time is not finite"
            )));
        }
        records.push((t, start, end));
        if records.len() > MAX_SIGNAL_POINTS {
            return Err(SignalError::Corrupt(format!(
                "more than {MAX_SIGNAL_POINTS} spectra"
            )));
        }
        at = end;
    }
    Ok(FieldIndex { stated, records })
}

/// Decode one field record's sections: a `RawSpHdr` and one `PtsLDiff` spectrum (first
/// wavelength, then integers: step numerator and denominator, scale numerator and denominator,
/// the first value and differences, a final 0).
///
/// # Errors
/// Sections other than those, or a broken spectrum.
pub fn decode_spectrum(rec: &[u8]) -> Result<(SpectrumGrid, Vec<i64>), SignalError> {
    let corrupt = |m: String| SignalError::Corrupt(format!("spectrum: {m}"));
    let mut off = 0usize;
    let mut out = None;
    while off < rec.len() {
        let tag = rec
            .get(off..off + 8)
            .ok_or_else(|| corrupt("section cut short".into()))?;
        let len = le_u64(rec, off + 8)
            .and_then(|l| usize::try_from(l).ok())
            .filter(|&l| l >= 16 && off.checked_add(l).is_some_and(|e| e <= rec.len()))
            .ok_or_else(|| corrupt(format!("section at byte {off} runs past the record")))?;
        let body = &rec[off + 16..off + len];
        if tag == POINTS_TAG {
            if out.is_some() {
                return Err(SignalError::Unsupported(
                    "a spectrum record with two PtsLDiff sections".into(),
                ));
            }
            let x0 = le_u64(body, 0)
                .map(f64::from_bits)
                .filter(|x| x.is_finite())
                .ok_or_else(|| corrupt("no first wavelength".into()))?;
            let mut at = 8usize;
            let mut head = [0u64; 4];
            for h in &mut head {
                *h = u64::try_from(signed_varint(body, &mut at).map_err(corrupt)?)
                    .map_err(|_| corrupt("negative step or scale".into()))?;
            }
            if head.contains(&0) {
                return Err(corrupt("zero step or scale".into()));
            }
            let mut vals = Vec::new();
            let mut sum = 0i64;
            let mut diffs = Vec::new();
            while at < body.len() {
                diffs.push(signed_varint(body, &mut at).map_err(corrupt)?);
            }
            if diffs.pop() != Some(0) {
                return Err(corrupt("no 0 terminator".into()));
            }
            for d in diffs {
                sum = sum
                    .checked_add(d)
                    .ok_or_else(|| corrupt("values overflow 64 bits".into()))?;
                vals.push(sum);
            }
            out = Some((
                SpectrumGrid {
                    x0,
                    step_num: head[0],
                    step_den: head[1],
                    scale_num: head[2],
                    scale_den: head[3],
                    points: vals.len(),
                },
                vals,
            ));
        } else if tag != SPECTRUM_HEADER_TAG {
            return Err(SignalError::Unsupported(format!(
                "section `{}` in a spectrum record (only RawSpHdr and PtsLDiff are decoded)",
                String::from_utf8_lossy(tag).trim_end_matches('\0')
            )));
        }
        off += len;
    }
    out.ok_or_else(|| corrupt("no PtsLDiff section".into()))
}

/// A protocol-buffers field value (wire types 0, 1, 2 and 5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PbValue<'a> {
    /// Wire type 0.
    Varint(u64),
    /// Wire type 1 (8 bytes, read as f64 where a double is meant).
    Fixed64(u64),
    /// Wire type 2.
    Bytes(&'a [u8]),
    /// Wire type 5.
    Fixed32(u32),
}

fn pb_varint(b: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let x = *b.get(*at)?;
        *at += 1;
        let chunk = u64::from(x & 0x7F);
        if shift >= 64 || (chunk << shift) >> shift != chunk {
            return None;
        }
        v |= chunk << shift;
        if x & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
    }
}

/// The fields of a protocol-buffers message, or `None` when `b` is not one complete message.
pub fn pb_fields(b: &[u8]) -> Option<Vec<(u64, PbValue<'_>)>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < b.len() {
        let key = pb_varint(b, &mut at)?;
        let (field, wire) = (key >> 3, key & 7);
        if field == 0 {
            return None;
        }
        let v = match wire {
            0 => PbValue::Varint(pb_varint(b, &mut at)?),
            1 => {
                let v = le_u64(b, at)?;
                at += 8;
                PbValue::Fixed64(v)
            }
            2 => {
                let n = usize::try_from(pb_varint(b, &mut at)?).ok()?;
                let end = at.checked_add(n).filter(|&e| e <= b.len())?;
                let v = &b[at..end];
                at = end;
                PbValue::Bytes(v)
            }
            5 => {
                let v = le_u32(b, at)?;
                at += 4;
                PbValue::Fixed32(v)
            }
            _ => return None,
        };
        out.push((field, v));
    }
    Some(out)
}

pub(crate) fn pb_get<'a>(fields: &[(u64, PbValue<'a>)], n: u64) -> Option<PbValue<'a>> {
    fields.iter().find(|(f, _)| *f == n).map(|(_, v)| *v)
}

pub(crate) fn pb_bytes<'a>(fields: &[(u64, PbValue<'a>)], n: u64) -> Option<&'a [u8]> {
    match pb_get(fields, n)? {
        PbValue::Bytes(b) => Some(b),
        _ => None,
    }
}

pub(crate) fn pb_text(fields: &[(u64, PbValue<'_>)], n: u64) -> Option<String> {
    pb_bytes(fields, n)
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(str::to_string)
}

pub(crate) fn pb_f64(fields: &[(u64, PbValue<'_>)], n: u64) -> Option<f64> {
    match pb_get(fields, n)? {
        PbValue::Fixed64(v) => Some(f64::from_bits(v)).filter(|x| x.is_finite()),
        _ => None,
    }
}

/// An axis of a signal description: range, unit and quantity.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignalAxis {
    /// Smallest value (0 when not stored).
    pub min: f64,
    /// Largest value (0 when not stored).
    pub max: f64,
    /// Unit (`min`, `mV`, `nC`, `psi`).
    pub unit: Option<String>,
    /// Quantity (`Time`, `Pressure`; often empty).
    pub quantity: Option<String>,
}

fn axis(b: &[u8]) -> Option<SignalAxis> {
    let f = pb_fields(b)?;
    let text = |n| pb_text(&f, n).filter(|s| !s.is_empty());
    Some(SignalAxis {
        min: pb_f64(&f, 1).unwrap_or(0.0),
        max: pb_f64(&f, 2).unwrap_or(0.0),
        unit: text(3),
        quantity: text(4),
    })
}

/// The sequence file's description of one signal.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignalDescription {
    /// The signal's `RawDataFileId` (`2025\11\16\164901056.raw`).
    pub file_id: String,
    /// Number of points.
    pub points: Option<u64>,
    /// Time axis (range in minutes).
    pub time: SignalAxis,
    /// Signal axis: the stored minimum and maximum and the unit.
    pub signal: SignalAxis,
    /// Device name (`GC`, `Pump_1`), when stored as text.
    pub device: Option<String>,
    /// Instrument module or driver (`Thermo Scientific Trace GC`, `DC-6000`).
    pub module: Option<String>,
    /// Scale factor from stored integers to the unit.
    pub scale: Option<f64>,
    /// Smallest and largest step between points (minutes), from the description's `<CmData>`
    /// (`StepMin`, `StepMax`): equal for a regular signal.
    pub step_range: Option<(f64, f64)>,
    /// The description's label (field 7, e.g. `WVL:254 nm`), when not empty.
    pub label: Option<String>,
}

impl SignalDescription {
    /// Are the points on a regular grid (the stored smallest and largest steps agree)?
    pub fn is_regular(&self) -> bool {
        self.step_range
            .is_none_or(|(a, b)| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1e-12))
    }
}

/// The value of `<Tag value="…" />` in a `<CmData>` string.
fn cmdata_value(xml: &str, tag: &str) -> Option<f64> {
    let at = xml.find(&format!("<{tag} value=\""))? + tag.len() + 9;
    let rest = xml.get(at..)?;
    rest.get(..rest.find('"')?)?
        .trim()
        .parse()
        .ok()
        .filter(|x: &f64| x.is_finite())
}

/// The sequence file's description of a spectral field (3D data): its time, wavelength and
/// absorbance axes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FieldDescription {
    /// The field's `RawDataFileId` (`…\144519125.sfd`).
    pub file_id: String,
    /// Time axis: first and last spectrum (minutes).
    pub time: SignalAxis,
    /// Wavelength axis (nm).
    pub wavelength: SignalAxis,
    /// Value axis: the smallest and largest value and the unit (`mAU`).
    pub value: SignalAxis,
    /// Device (`UV`, `PDA`).
    pub device: Option<String>,
    /// Driver (`DAD3000.dll`, `Acquity.dll`).
    pub module: Option<String>,
}

/// The spectral-field descriptions of a sequence file: top-level field 19 records whose first
/// field (5) holds a field description (field 4) and the field's file id (field 6).
pub fn parse_sequence_fields(cmd: &[u8]) -> Vec<FieldDescription> {
    let Some(top) = pb_fields(cmd) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (f, v) in &top {
        let (19, PbValue::Bytes(rec)) = (*f, *v) else {
            continue;
        };
        let Some(rec) = pb_fields(rec) else { continue };
        let Some((5, PbValue::Bytes(outer))) = rec.first().copied() else {
            continue;
        };
        let Some(outer) = pb_fields(outer) else {
            continue;
        };
        let Some(file_id) = pb_text(&outer, 6).filter(|s| s.to_ascii_lowercase().ends_with(".sfd"))
        else {
            continue;
        };
        let Some(desc) = pb_bytes(&outer, 4).and_then(pb_fields) else {
            continue;
        };
        out.push(FieldDescription {
            file_id,
            time: pb_bytes(&desc, 5).and_then(axis).unwrap_or_default(),
            wavelength: pb_bytes(&desc, 6).and_then(axis).unwrap_or_default(),
            value: pb_bytes(&desc, 7).and_then(axis).unwrap_or_default(),
            device: pb_text(&desc, 11).filter(|s| !s.is_empty()),
            module: pb_text(&desc, 13).filter(|s| !s.is_empty()),
        });
    }
    out
}

fn is_file_id(s: &str) -> bool {
    // `YYYY\MM\DD\hhmmssmmm.raw`, the day folder sometimes `DD_2`
    let b = s.as_bytes();
    s.len() > 15
        && s.get(s.len() - 4..)
            .is_some_and(|e| e.eq_ignore_ascii_case(".raw"))
        && b.get(4) == Some(&b'\\')
        && b.get(7) == Some(&b'\\')
        && s.matches('\\').count() == 3
}

/// The signal descriptions of a sequence file (`.cmd`): top-level field 19 records whose first
/// field (5) holds the description (field 3) and the signal's file id (field 6).
pub fn parse_sequence_signals(cmd: &[u8]) -> Vec<SignalDescription> {
    let Some(top) = pb_fields(cmd) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (f, v) in &top {
        let (19, PbValue::Bytes(rec)) = (*f, *v) else {
            continue;
        };
        let Some(rec) = pb_fields(rec) else { continue };
        let Some((5, PbValue::Bytes(outer))) = rec.first().copied() else {
            continue;
        };
        let Some(outer) = pb_fields(outer) else {
            continue;
        };
        let Some(file_id) = pb_text(&outer, 6).filter(|s| is_file_id(s)) else {
            continue;
        };
        let Some(desc) = pb_bytes(&outer, 3).and_then(pb_fields) else {
            continue;
        };
        let text = |n| pb_text(&desc, n).filter(|s| !s.is_empty());
        out.push(SignalDescription {
            file_id,
            points: match pb_get(&desc, 4) {
                Some(PbValue::Varint(n)) => Some(n),
                _ => None,
            },
            time: pb_bytes(&desc, 5).and_then(axis).unwrap_or_default(),
            signal: pb_bytes(&desc, 6).and_then(axis).unwrap_or_default(),
            device: text(11).filter(|s| s.chars().all(|c| !c.is_control())),
            module: text(12),
            scale: pb_f64(&desc, 15),
            step_range: pb_text(&desc, 3)
                .and_then(|x| Some((cmdata_value(&x, "StepMin")?, cmdata_value(&x, "StepMax")?))),
            label: text(7),
        });
    }
    out
}

/// Does `head` start like a Chromeleon archive: a zip whose first member is a sequence file
/// (`*.seq_<n>.cmd`)?
pub fn first_member_is_sequence(first: &str) -> bool {
    let lower = first.to_ascii_lowercase();
    lower
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| ext == "cmd" && stem.contains(".seq_"))
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::trivially_copy_pass_by_ref)]
mod tests {
    use super::*;

    /// Encode one integer the way `PtsLDiff` blocks store them.
    pub(crate) fn enc(v: i64) -> Vec<u8> {
        let mut m = v.unsigned_abs();
        let mut first = (m & 0x3F) as u8;
        if v < 0 {
            first |= 0x40;
        }
        m >>= 6;
        let mut out = vec![first];
        while m > 0 {
            *out.last_mut().unwrap() |= 0x80;
            out.push((m & 0x7F) as u8);
            m >>= 7;
        }
        out
    }

    pub(crate) fn block(t0: f64, rate: i64, num: i64, den: i64, diffs: &[i64]) -> Vec<u8> {
        let mut body = t0.to_le_bytes().to_vec();
        for v in [1, rate, num, den].iter().chain(diffs).chain(&[0]) {
            body.extend(enc(*v));
        }
        section(POINTS_TAG, &body)
    }

    pub(crate) fn section(tag: &[u8; 8], body: &[u8]) -> Vec<u8> {
        let mut s = tag.to_vec();
        s.extend((body.len() as u64 + 16).to_le_bytes());
        s.extend(body);
        s
    }

    pub(crate) fn header(name: &str, ft: u64) -> Vec<u8> {
        let mut b = vec![1, 2, 7, 2, 0x0a];
        b.extend(ft.to_le_bytes());
        b.extend([2, name.len() as u8]);
        b.extend(name.as_bytes());
        b.extend([0u8; 16]);
        b.extend([0xb5, 0xf5, 0x02, 0x00]);
        section(SIGNAL_HEADER_TAG, &b)
    }

    #[test]
    fn integers() {
        for v in [
            0i64,
            1,
            -1,
            50,
            -50,
            63,
            64,
            -64,
            1600,
            -1600,
            142_452_800,
            1 << 61,
            1 << 62,
        ] {
            let e = enc(v);
            let mut at = 0;
            assert_eq!(signed_varint(&e, &mut at).unwrap(), v, "{v}");
            assert_eq!(at, e.len());
        }
        // values seen in the corpus
        let mut at = 0;
        assert_eq!(signed_varint(&[0xe4, 0x01], &mut at).unwrap(), -100);
        at = 0;
        assert_eq!(
            signed_varint(
                &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x40],
                &mut at
            )
            .unwrap(),
            1 << 61
        );
        at = 0;
        assert!(signed_varint(&[0x80], &mut at).is_err());
        at = 0;
        assert!(signed_varint(&[0xff; 12], &mut at).is_err());
    }

    #[test]
    fn signal() {
        let ft = 134_077_804_531_041_000;
        let mut file = header("GC_1", ft);
        file.extend(block(0.0, 3000, 1, 10_000, &[61_800, 650, -800]));
        file.extend(block(3.0 / 3000.0, 3000, 1, 10_000, &[62_000, -50]));
        let signal = decode_signal(&file).unwrap();
        assert_eq!(signal.raw, vec![61_800, 62_450, 61_650, 62_000, 61_950]);
        assert_eq!(signal.blocks, 2);
        assert_eq!(signal.points_per_min, 3000.0);
        assert!((signal.values()[0] - 6.18).abs() < 1e-12);
        assert_eq!(
            signal.header,
            Some(SignalHeader {
                name: "GC_1".into(),
                start_filetime: ft
            })
        );
        // a gap between blocks is refused, not bridged
        let mut gapped = header("GC_1", ft);
        gapped.extend(block(0.0, 3000, 1, 10_000, &[1, 2]));
        gapped.extend(block(0.5, 3000, 1, 10_000, &[1]));
        assert!(matches!(
            decode_signal(&gapped),
            Err(SignalError::Unsupported(_))
        ));
        // a rate denominator: 600000 / 10000 = 60 points per minute
        let mut flow = header("FlowChan", ft);
        let mut body = 0f64.to_le_bytes().to_vec();
        for x in [10_000i64, 600_000, 1, 100, 20, 0, 0] {
            body.extend(enc(x));
        }
        flow.extend(section(POINTS_TAG, &body));
        let decoded = decode_signal(&flow).unwrap();
        assert_eq!(decoded.points_per_min, 60.0);
        assert_eq!(decoded.values(), vec![0.2, 0.2]);
        // a zero rate denominator is corrupt
        let mut zero_rate = header("GC_1", ft);
        let mut body = 0f64.to_le_bytes().to_vec();
        for x in [0i64, 3000, 1, 10_000, 5, 0] {
            body.extend(enc(x));
        }
        zero_rate.extend(section(POINTS_TAG, &body));
        assert!(matches!(
            decode_signal(&zero_rate),
            Err(SignalError::Corrupt(_))
        ));
        // unknown section
        let mut unknown = file.clone();
        unknown.extend(section(b"PtsFloat", &[0; 8]));
        assert!(matches!(
            decode_signal(&unknown),
            Err(SignalError::Unsupported(_))
        ));
    }

    #[test]
    fn second_difference_pairs() {
        // two blocks: (0, 0); then (4e7 ns, 444), (Δ 4e7, 4084), (ΔΔ 0, 15466)
        let mut b = header("UV_VIS_1", 134_077_804_531_041_000);
        let block = |ints: &[i64]| {
            let mut body = 6.0e10f64.to_le_bytes().to_vec();
            body.extend(1.0e8f64.to_le_bytes());
            for x in ints {
                body.extend(enc(*x));
            }
            section(LL2_TAG, &body)
        };
        b.extend(block(&[0, 0, 0]));
        b.extend(block(&[40_000_000, 444, 40_000_000, 4084, 0, 15_466, 0]));
        let d = decode_signal(&b).unwrap();
        assert_eq!(d.encoding, "PtsLL2Df");
        assert_eq!(d.raw, vec![0, 444, 4528, 24_078]);
        assert!((d.points_per_min - 1500.0).abs() < 1e-9);
        assert!(d.times.is_none());
        assert!((d.values()[1] - 4.44e-6).abs() < 1e-18);
        // an odd number of integers is corrupt; mixed encodings are refused
        let mut c = header("UV_VIS_1", 134_077_804_531_041_000);
        c.extend(block(&[1, 2, 3, 0]));
        assert!(matches!(decode_signal(&c), Err(SignalError::Corrupt(_))));
        let mut m = b.clone();
        m.extend(block_ldiff());
        assert!(matches!(
            decode_signal(&m),
            Err(SignalError::Unsupported(_))
        ));
        for cut in 0..b.len() {
            let _ = decode_signal(&b[..cut]);
        }
    }

    fn block_ldiff() -> Vec<u8> {
        block(0.0, 3000, 1, 10_000, &[1, 2])
    }

    #[test]
    fn compressed_pairs() {
        // LZMA2 (CPython lzma, FORMAT_RAW) of two (time, value) f64 pairs is not at hand here:
        // an uncompressed LZMA2 chunk (control 1) is a valid stream
        let mut pairs = Vec::new();
        for (t, v) in [(0.0f64, 1.5f64), (0.5, 2.5), (1.0, -3.0)] {
            pairs.extend(t.to_le_bytes());
            pairs.extend(v.to_le_bytes());
        }
        let mut lz = vec![1u8, 0, (pairs.len() - 1) as u8];
        lz.extend(&pairs);
        lz.push(0);
        let mut body = Vec::new();
        for x in [1i64, 0, pairs.len() as i64, lz.len() as i64 + 1] {
            body.extend(enc(x));
        }
        body.push(8); // dictionary byte
        body.extend(&lz);
        let mut b = header("EXT350NM", 134_077_804_531_041_000);
        b.extend(section(DDCMP_TAG, &body));
        let d = decode_signal(&b).unwrap();
        assert_eq!(d.encoding, "PtsDDCmp");
        assert_eq!(d.values(), vec![1.5, 2.5, -3.0]);
        assert_eq!(d.points_per_min, 2.0);
        // a wrong compressed length is corrupt
        let mut bad = body.clone();
        bad[3] ^= 1;
        let mut c = header("EXT350NM", 134_077_804_531_041_000);
        c.extend(section(DDCMP_TAG, &bad));
        assert!(decode_signal(&c).is_err());
        for cut in 0..b.len() {
            let _ = decode_signal(&b[..cut]);
        }
    }

    #[test]
    fn spectral_fields() {
        let rec = |t: f64, vals: &[i64]| {
            let mut spec = 220.0f64.to_le_bytes().to_vec();
            for x in [10i64, 10, 1000, 16_777_215] {
                spec.extend(enc(x));
            }
            let mut prev = 0;
            for (i, v) in vals.iter().enumerate() {
                spec.extend(enc(if i == 0 { *v } else { v - prev }));
                prev = *v;
            }
            spec.extend(enc(0));
            let mut r = section(SPECTRUM_HEADER_TAG, &[1, 2, 2]);
            r.extend(section(POINTS_TAG, &spec));
            let mut out = t.to_le_bytes().to_vec();
            out.extend((r.len() as u32).to_le_bytes());
            out.extend(r);
            out
        };
        let mut f = FIELD_MAGIC.to_vec();
        for x in [1u32, 0, 2, 8] {
            f.extend(x.to_le_bytes());
        }
        f.extend(rec(0.0006, &[0, 5, 7]));
        f.extend(rec(0.0012, &[1, 6, 9]));
        let idx = index_field(&f, true).unwrap();
        assert_eq!(idx.stated, 2);
        assert_eq!(idx.records.len(), 2);
        let (g, raw) = decode_spectrum(&f[idx.records[1].1..idx.records[1].2]).unwrap();
        assert_eq!(raw, vec![1, 6, 9]);
        assert_eq!(g.points, 3);
        assert_eq!(g.step(), 1.0);
        assert_eq!(g.value(16_777_215), 1000.0);
        // a cut member walks what it holds when not complete, and is corrupt when complete
        let cut = &f[..f.len() - 3];
        assert_eq!(index_field(cut, false).unwrap().records.len(), 1);
        assert!(index_field(cut, true).is_err());
        let mut v = f.clone();
        v[8] = 2;
        assert!(matches!(
            index_field(&v, true),
            Err(SignalError::Unsupported(_))
        ));
        for c in 0..f.len() {
            if let Ok(i) = index_field(&f[..c], false) {
                for (_, a, b) in i.records {
                    let _ = decode_spectrum(&f[a..b]);
                }
            }
        }
    }

    #[test]
    fn malformed_signals_are_errors() {
        let mut b = header("GC_1", 134_077_804_531_041_000);
        b.extend(block(0.0, 3000, 1, 10_000, &[1, 2, 3]));
        for cut in 0..b.len() {
            let _ = decode_signal(&b[..cut]);
        }
        for i in 0..b.len() {
            let mut c = b.clone();
            c[i] ^= 0xA5;
            let _ = decode_signal(&c);
        }
        // no terminator
        let mut body = 0f64.to_le_bytes().to_vec();
        for x in [1i64, 3000, 1, 10_000, 5, 7] {
            body.extend(enc(x));
        }
        assert!(matches!(
            decode_signal(&section(POINTS_TAG, &body)),
            Err(SignalError::Corrupt(_))
        ));
        assert!(decode_signal(&[]).is_err());
        // overflowing sums
        let over = block(0.0, 3000, 1, 1, &[i64::MAX, i64::MAX]);
        assert!(matches!(decode_signal(&over), Err(SignalError::Corrupt(_))));
    }

    fn pb_len(field: u64, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut key = (field << 3) | 2;
        loop {
            let b = (key & 0x7F) as u8;
            key >>= 7;
            if key == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
        let mut n = body.len() as u64;
        loop {
            let b = (n & 0x7F) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
        out.extend(body);
        out
    }

    fn pb_double(field: u64, v: f64) -> Vec<u8> {
        let mut out = vec![((field << 3) | 1) as u8];
        out.extend(v.to_le_bytes());
        out
    }

    #[test]
    fn sequence_descriptions() {
        let mut time = pb_double(2, 19.4);
        time.extend(pb_len(3, b"min"));
        time.extend(pb_len(4, b"Time"));
        let mut sig = pb_double(1, 0.235);
        sig.extend(pb_double(2, 14_246.24));
        sig.extend(pb_len(3, b"mV"));
        sig.extend(pb_len(4, b""));
        let mut desc = pb_len(3, b"<CmData />");
        desc.extend([0x20, 0xd9, 0xc6, 0x03]); // field 4 = 58201
        desc.extend(pb_len(5, &time));
        desc.extend(pb_len(6, &sig));
        desc.extend(pb_len(11, b"GC"));
        desc.extend(pb_len(12, b"Thermo Scientific Trace GC"));
        desc.extend(pb_double(15, 0.0001));
        let mut outer = pb_len(3, &desc);
        outer.extend(pb_len(6, b"2025\\11\\16\\162413117.raw"));
        let rec = pb_len(5, &outer);
        let mut cmd = pb_len(18, b"\x12\x00");
        cmd.extend(pb_len(19, &rec));
        cmd.extend(pb_len(19, b"\x2a\x00"));
        let v = parse_sequence_signals(&cmd);
        assert_eq!(v.len(), 1);
        let d = &v[0];
        assert_eq!(d.file_id, "2025\\11\\16\\162413117.raw");
        assert_eq!(d.points, Some(58_201));
        assert_eq!(d.time.max, 19.4);
        assert_eq!(d.time.unit.as_deref(), Some("min"));
        assert_eq!(d.signal.unit.as_deref(), Some("mV"));
        assert_eq!(d.signal.quantity, None);
        assert_eq!(d.signal.max, 14_246.24);
        assert_eq!(d.device.as_deref(), Some("GC"));
        assert_eq!(d.scale, Some(0.0001));
        // truncated input: nothing, no panic
        for cut in 0..cmd.len() {
            let _ = parse_sequence_signals(&cmd[..cut]);
        }
    }

    #[test]
    fn header_xml() {
        let x = r#"<?xml version="1.0" encoding="UTF-8"?><ChromeleonHeader GeneratorVersion="7.2.10.23925" ContainerVersion="2.0" DateCreated="Monday, 29 December 2025"><ChromeleonElement Id="1" Name="Seq &amp; 1" ItemType="Dionex.Chromeleon.Data.Sequence" Filename="Seq.seq_1.cmd" Size="10"><ChromeleonElement Id="2" Name="Wash" ItemType="Dionex.Chromeleon.Data.Injection" InjectionType="Blank"><ChromeleonElement Id="3" Name="GC_1" ItemType="Dionex.Chromeleon.Data.Signal" RawDataFilename="2_1.raw" Size="5" RawDataFileId="2025\11\16\1.raw" /></ChromeleonElement><ChromeleonElement Id="4" Name="PHA" ItemType="Dionex.Chromeleon.Data.InstrumentMethod" /></ChromeleonElement></ChromeleonHeader>"#;
        let h = parse_header(x).unwrap();
        assert_eq!(h.generator_version.as_deref(), Some("7.2.10.23925"));
        assert_eq!(h.items.len(), 4);
        assert_eq!(h.items[0].name, "Seq & 1");
        assert_eq!(h.items[0].member.as_deref(), Some("Seq.seq_1.cmd"));
        assert_eq!(h.items[2].parent, Some(1));
        assert_eq!(h.items[3].parent, Some(0));
        assert_eq!(h.ancestor(2, INJECTION_ITEM), Some(1));
        assert_eq!(h.ancestor(2, SEQUENCE_ITEM), Some(0));
        assert_eq!(h.of_type(SIGNAL_ITEM).collect::<Vec<_>>(), vec![2]);
        let f = r#"<ChromeleonHeader><ChromeleonElement Id="1" Name="S" ItemType="Dionex.Chromeleon.Data.Sequence"><ChromeleonElement Id="2" Name="Quant" ItemType="Dionex.Chromeleon.Data.ProcessingMethod"><FixedInjectionList><FixedInjection FixedInjectionName="Std 1 mM"><ChromeleonElement Id="3" Name="GC_1" ItemType="Dionex.Chromeleon.Data.Signal" RawDataFilename="10_1.raw" /></FixedInjection></FixedInjectionList></ChromeleonElement></ChromeleonElement></ChromeleonHeader>"#;
        let h = parse_header(f).unwrap();
        assert_eq!(h.items.len(), 3);
        assert_eq!(h.items[2].parent, Some(1));
        assert_eq!(h.items[2].fixed_injection.as_deref(), Some("Std 1 mM"));
        assert_eq!(h.items[1].fixed_injection, None);
        assert!(parse_header("<Other/>").is_err());
        assert!(parse_header("not xml").is_err());
    }
}

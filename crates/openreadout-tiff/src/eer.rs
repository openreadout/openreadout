//! Thermo Fisher EER (Electron Event Representation) movies from Falcon 4/4i/C cameras: a
//! BigTIFF with one page per detector frame, each page one strip of electron events coded as
//! run lengths (compressions 65000, 65001, 65002), plus XML metadata in private tags 65001
//! (acquisition) and 65002 (frame). See `docs/formats/tiff.md` § EER.

use openreadout_core::model::{ChannelInfo, ImageInfo, InstrumentInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, PixelType};
use serde_json::{Map, Value, json};

use crate::container::Ifd;
use crate::dataset::{PlaneSrc, Series, TiffDataset, check_plane_count};
use crate::decode::PageLayout;
use crate::{FORMAT_ID, tags};

/// Acquisition metadata (`<metadata><item name=.. unit=..>value</item>…`), page 0.
pub(crate) const ACQUISITION_METADATA: u16 = 65001;
/// Frame metadata, every page.
pub(crate) const FRAME_METADATA: u16 = 65002;
/// Compression 65002: bits of the run-length (skip) code.
pub(crate) const SKIP_BITS: u16 = 65007;
/// Compression 65002: bits of the horizontal sub-pixel position.
pub(crate) const HORZ_SUB_BITS: u16 = 65008;
/// Compression 65002: bits of the vertical sub-pixel position.
pub(crate) const VERT_SUB_BITS: u16 = 65009;

/// True for the three EER compression codes.
pub fn is_eer_compression(compression: u16) -> bool {
    matches!(compression, 65000..=65002)
}

/// How the events of a page are coded: a `skip_bits` run length, then (for an event)
/// `horz_bits` and `vert_bits` of sub-pixel position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EerCoding {
    pub skip_bits: u32,
    pub horz_bits: u32,
    pub vert_bits: u32,
}

impl EerCoding {
    /// The coding of a page with an EER compression: 65000 is 8 + 2 + 2 bits, 65001 is 7 + 2 + 2,
    /// 65002 takes the three sizes from tags 65007–65009 (7, 2, 2 when absent).
    pub(crate) fn of(compression: u16, ifd: &Ifd) -> Option<Self> {
        let tag = |t: u16, d: u32| {
            ifd.uint(t)
                .map_or(Some(d), |v| u32::try_from(v).ok())
                .unwrap_or(u32::MAX)
        };
        let c = match compression {
            65000 => EerCoding {
                skip_bits: 8,
                horz_bits: 2,
                vert_bits: 2,
            },
            65001 => EerCoding {
                skip_bits: 7,
                horz_bits: 2,
                vert_bits: 2,
            },
            65002 => EerCoding {
                skip_bits: tag(SKIP_BITS, 7),
                horz_bits: tag(HORZ_SUB_BITS, 2),
                vert_bits: tag(VERT_SUB_BITS, 2),
            },
            _ => return None,
        };
        Some(c)
    }

    /// Bits of one event code.
    pub fn event_bits(self) -> u32 {
        self.skip_bits
            .saturating_add(self.horz_bits)
            .saturating_add(self.vert_bits)
    }

    /// A coding the decoder handles: a run length of 2–16 bits and an event code of at most
    /// 24 bits (one 32-bit little-endian read at any bit offset covers it).
    pub fn check(self) -> Result<(), String> {
        if !(2..=16).contains(&self.skip_bits) || self.event_bits() > 24 {
            return Err(format!(
                "EER coding with {} run-length bits and {}+{} sub-pixel bits is not decoded",
                self.skip_bits, self.horz_bits, self.vert_bits
            ));
        }
        Ok(())
    }
}

/// Walk the events of one strip of `pixels` pixels, calling `on_event(pixel index)` for each;
/// returns the number of events. The bit stream is read least significant bit first: a
/// `skip_bits` code adds that many pixels without an event; a code of all ones continues the
/// run (no event follows it); any other code is followed by the event's sub-pixel bits, the
/// event lies on the pixel reached, and the next run starts on the pixel after it. The stream
/// ends when the run reaches the last pixel or the bits run out.
pub fn walk(
    src: &[u8],
    pixels: usize,
    coding: EerCoding,
    mut on_event: impl FnMut(usize),
) -> Result<u64, String> {
    coding.check()?;
    let skip_mask = (1u32 << coding.skip_bits) - 1;
    let nbits = coding.event_bits() as usize;
    let total_bits = src.len().saturating_mul(8);
    let mut bit = 0usize;
    let mut pixel = 0usize;
    let mut events = 0u64;
    while bit.saturating_add(nbits) <= total_bits {
        let byte = bit / 8;
        let mut word = [0u8; 4];
        let avail = src.len() - byte;
        let n = avail.min(4);
        word[..n].copy_from_slice(&src[byte..byte + n]);
        let code = (u32::from_le_bytes(word) >> (bit % 8)) & skip_mask;
        pixel = pixel
            .checked_add(code as usize)
            .ok_or_else(|| "EER run length overflows".to_string())?;
        if pixel == pixels {
            break;
        }
        if pixel > pixels {
            return Err(format!(
                "EER events run past the end of the strip (pixel {pixel} of {pixels}, bit {bit})"
            ));
        }
        if code == skip_mask {
            bit += coding.skip_bits as usize;
            continue;
        }
        on_event(pixel);
        events += 1;
        pixel += 1;
        bit += nbits;
    }
    Ok(events)
}

/// Decode one strip to event counts per pixel (`width × rows` bytes, 0 or 1: a pixel holds at
/// most one event of a frame).
pub fn decode_counts(
    src: &[u8],
    width: usize,
    rows: usize,
    coding: EerCoding,
) -> Result<Vec<u8>, String> {
    let pixels = width
        .checked_mul(rows)
        .ok_or_else(|| "EER strip size overflows".to_string())?;
    let mut out = vec![0u8; pixels];
    walk(src, pixels, coding, |p| {
        out[p] = out[p].saturating_add(1);
    })?;
    Ok(out)
}

/// The `<item>`s of an EER metadata block as `name → value` (numbers as numbers, `Yes`/`No` as
/// booleans, other text as strings) and `name → unit`. `None` when the bytes are not such a block.
pub fn parse_items(bytes: &[u8]) -> Option<(Map<String, Value>, Map<String, Value>)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.trim_end_matches('\0');
    // gain references start with `<?xml version="1.0" encoding="utf - 8"?>`, a declaration
    // XML parsers refuse: the element is read from `<metadata` on
    let start = text.find("<metadata")?;
    let doc = roxmltree::Document::parse(&text[start..]).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "metadata" {
        return None;
    }
    let mut items = Map::new();
    let mut units = Map::new();
    for it in root.children().filter(|n| n.has_tag_name("item")) {
        let Some(name) = it.attribute("name").filter(|n| !n.is_empty()) else {
            continue;
        };
        let raw = it.text().unwrap_or_default().trim();
        let value = match raw {
            "Yes" => Value::Bool(true),
            "No" => Value::Bool(false),
            _ => raw
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| {
                    raw.parse::<f64>()
                        .ok()
                        .filter(|f| f.is_finite())
                        .map(Value::from)
                })
                .unwrap_or_else(|| Value::String(raw.to_string())),
        };
        items.insert(name.to_string(), value);
        if let Some(u) = it.attribute("unit") {
            units.insert(name.to_string(), Value::String(u.to_string()));
        }
    }
    Some((items, units))
}

/// True when page 0 carries EER acquisition metadata (tag 65001 holding a `<metadata>` block).
pub(crate) fn has_metadata(ifd: &Ifd) -> bool {
    ifd.bytes(ACQUISITION_METADATA)
        .is_some_and(|b| parse_items(b).is_some())
}

/// One EER frame's (events, pixels, recorded dose in electrons per pixel).
pub(crate) type EerFrameEvents = (u64, u64, Option<f64>);

impl TiffDataset {
    /// Thermo Fisher EER: every page is one detector frame (T), decoded to electron counts on
    /// the sensor grid; acquisition metadata from tag 65001 of page 0.
    pub(crate) fn build_eer(&mut self, page0: &Ifd) -> openreadout_core::Result<()> {
        let order = self.main().header.byte_order;
        let l = PageLayout::from_ifd(page0, order)?;
        let coding = l.eer.ok_or_else(|| {
            Error::corrupt(FORMAT_ID, "EER file whose first page has no EER coding")
        })?;
        coding.check().map_err(|e| {
            Error::unsupported(
                FORMAT_ID,
                e,
                "EER pages are decoded with run lengths of 2-16 bits and event codes of at most 24 bits.",
            )
        })?;
        let (pages, other) = self.frame_pages(&l);
        check_plane_count(pages.len() as u64)?;
        let acq = Acquisition::of(page0);
        let orientation = page0.uint(tags::ORIENTATION).unwrap_or(1);
        let mut info = acq.image_info(&l, pages.len());
        info.extra.insert(
            "eer".into(),
            json!({
                "acquisition": acq.items,
                "units": acq.units,
                "compression": l.compression,
                "run_length_bits": coding.skip_bits,
                "subpixel_bits": [coding.horz_bits, coding.vert_bits],
                "orientation": orientation,
            }),
        );
        info.extra.insert(
            "pages".into(),
            json!({"first": pages.first(), "count": pages.len()}),
        );
        info.extra.insert("compression".into(), json!("eer"));
        let planes = pages
            .iter()
            .map(|&p| {
                Some(PlaneSrc::Page {
                    file: 0,
                    page: p,
                    sample: None,
                })
            })
            .collect();
        self.series.push(Series {
            info: info.finish(),
            planes,
            levels: Vec::new(),
        });
        self.notes.push(format!(
            "EER electron events: each of the {} frames is decoded to counts on the {}x{} sensor grid (0 or 1 per pixel and frame; sum frames for a dose image); the {}+{} sub-pixel position bits of each event are not used",
            pages.len(),
            l.width,
            l.height,
            coding.horz_bits,
            coding.vert_bits
        ));
        if orientation != 1 {
            self.notes.push(format!(
                "TIFF Orientation {orientation}: frames are returned as stored, not reoriented ({})",
                orientation_words(orientation)
            ));
        }
        if let Some(n) = acq.declared_frames()
            && (n - pages.len() as f64).abs() > 0.5
        {
            self.notes.push(format!(
                "numberOfFrames says {n} frames, the file holds {} frame pages",
                pages.len()
            ));
        }
        if !other.is_empty() {
            self.notes.push(format!(
                "{} page(s) with another geometry or compression are not frames and are skipped (first: page {})",
                other.len(),
                other[0]
            ));
        }
        self.set_provenance(&[
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].physical_size", Source::Inferred),
            ("images[].time_increment_s", Source::Inferred),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].instrument", Source::PriorArt),
        ]);
        Ok(())
    }

    /// The frame pages (the size and compression of page 0) and the other pages.
    fn frame_pages(&self, l: &PageLayout) -> (Vec<usize>, Vec<usize>) {
        let mut pages = Vec::new();
        let mut other = Vec::new();
        for (p, ifd) in self.main().ifds.iter().enumerate() {
            let same = ifd.uint(tags::IMAGE_WIDTH) == Some(u64::from(l.width))
                && ifd.uint(tags::IMAGE_LENGTH) == Some(u64::from(l.height))
                && ifd.uint(tags::COMPRESSION) == Some(u64::from(l.compression));
            if same {
                pages.push(p);
            } else {
                other.push(p);
            }
        }
        (pages, other)
    }
}

/// The acquisition metadata items of page 0 (tag 65001) and their units.
struct Acquisition {
    items: Map<String, Value>,
    units: Map<String, Value>,
}

impl Acquisition {
    fn of(page0: &Ifd) -> Self {
        let (items, units) = page0
            .bytes(ACQUISITION_METADATA)
            .and_then(parse_items)
            .unwrap_or_default();
        Acquisition { items, units }
    }

    fn num(&self, k: &str) -> Option<f64> {
        self.items.get(k).and_then(Value::as_f64)
    }

    fn unit(&self, k: &str) -> Option<&str> {
        self.units.get(k).and_then(Value::as_str)
    }

    fn text(&self, k: &str) -> Option<String> {
        self.items
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// A positive length item in metres, in µm.
    fn um(&self, k: &str) -> Option<f64> {
        self.num(k)
            .filter(|v| *v > 0.0 && v.is_finite() && self.unit(k) == Some("m"))
            .map(|v| v * 1e6)
    }

    fn declared_frames(&self) -> Option<f64> {
        self.num("numberOfFrames").filter(|v| *v >= 1.0)
    }

    /// The movie as an image of `frames` time points: sensor pixel size, frame time (the
    /// exposure over the declared frame count), start time and camera.
    fn image_info(&self, l: &PageLayout, frames: usize) -> ImageInfo {
        let mut info = ImageInfo::new(0, l.width, l.height, PixelType::Uint8);
        info.size_t = frames as u32;
        info.dimension_order = "XYCZT".into();
        info.physical_size = PhysicalSize::micrometres(
            self.um("sensorPixelSize.width"),
            self.um("sensorPixelSize.height"),
            None,
        );
        let exposure_s = self
            .num("exposureTime")
            .filter(|v| *v > 0.0 && self.unit("exposureTime") == Some("s"));
        info.time_increment_s = exposure_s.zip(self.declared_frames()).map(|(e, n)| e / n);
        info.acquired_at = self.text("timestamp");
        info.channels = vec![ChannelInfo {
            index: 0,
            name: Some("electron events".into()),
            exposure_ms: info.time_increment_s.map(|s| s * 1000.0),
            ..ChannelInfo::default()
        }];
        let detector = match (self.text("commercialName"), self.text("cameraName")) {
            (Some(c), Some(n)) => Some(format!("{c} ({n})")),
            (c, n) => c.or(n),
        };
        if detector.is_some() {
            info.instrument = Some(InstrumentInfo {
                detector,
                ..InstrumentInfo::default()
            });
        }
        info
    }
}

impl TiffDataset {
    /// A TIFF that is not an EER movie but carries EER metadata (a Falcon gain reference):
    /// the items are copied to `extra.eer` of its first image.
    pub(crate) fn apply_eer_metadata(&mut self, page0: &Ifd) {
        let Some((items, units)) = page0.bytes(ACQUISITION_METADATA).and_then(parse_items) else {
            return;
        };
        if let Some(s) = self.series.first_mut() {
            s.info
                .extra
                .insert("eer".into(), json!({"acquisition": items, "units": units}));
        }
        self.notes.push(
            "EER metadata (tag 65001) on a page that is not EER-coded (a Falcon gain reference or a processed image): its items are in `extra.eer`".into(),
        );
    }

    /// Per-frame EER metadata (tag 65002 of every frame page).
    pub(crate) fn eer_frames(&self, limit: Option<usize>) -> (u64, Vec<Value>) {
        let Some(s) = self.series.first() else {
            return (0, Vec::new());
        };
        let pages: Vec<usize> = s
            .planes
            .iter()
            .filter_map(|p| match p {
                Some(PlaneSrc::Page { page, .. }) => Some(*page),
                _ => None,
            })
            .collect();
        let any = pages.iter().any(|&p| {
            self.main()
                .ifds
                .get(p)
                .is_some_and(|i| i.bytes(FRAME_METADATA).is_some())
        });
        if !any {
            return (0, Vec::new());
        }
        let records = pages
            .iter()
            .enumerate()
            .take(limit.unwrap_or(usize::MAX))
            .map(|(t, &p)| {
                let mut m = Map::new();
                m.insert("t".into(), json!(t));
                m.insert("page".into(), json!(p));
                if let Some((items, units)) = self
                    .main()
                    .ifds
                    .get(p)
                    .and_then(|i| i.bytes(FRAME_METADATA))
                    .and_then(parse_items)
                {
                    for (k, v) in items {
                        m.insert(k, v);
                    }
                    if !units.is_empty() {
                        m.insert("units".into(), Value::Object(units));
                    }
                }
                Value::Object(m)
            })
            .collect();
        (pages.len() as u64, records)
    }
}

/// TIFF Orientation in words (where row 0 and column 0 of the stored image are displayed).
fn orientation_words(o: u64) -> &'static str {
    match o {
        2 => "row 0 is the top, column 0 the right: mirrored left-right",
        3 => "rotated 180 degrees",
        4 => "mirrored top-bottom",
        5 => "row 0 is the left side, column 0 the top: transposed",
        6 => "row 0 is the right side, column 0 the top: rotated 90 degrees clockwise",
        7 => "row 0 is the right side, column 0 the bottom: transposed and rotated",
        8 => "row 0 is the left side, column 0 the bottom: rotated 90 degrees counter-clockwise",
        _ => "an orientation TIFF does not define",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pack `(value, bits)` codes least significant bit first.
    fn pack(codes: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut acc = 0u64;
        let mut n = 0u32;
        for &(v, bits) in codes {
            acc |= u64::from(v) << n;
            n += bits;
            while n >= 8 {
                out.push((acc & 0xFF) as u8);
                acc >>= 8;
                n -= 8;
            }
        }
        if n > 0 {
            out.push((acc & 0xFF) as u8);
        }
        out
    }

    const V1: EerCoding = EerCoding {
        skip_bits: 7,
        horz_bits: 2,
        vert_bits: 2,
    };

    #[test]
    fn runs_events_and_continuations() {
        // event at pixel 3; a continuation (127) plus 10 → event at 4 + 127 + 10 = 141; then
        // the run reaches the end (200 pixels)
        let src = pack(&[(3, 7), (1, 2), (2, 2), (127, 7), (10, 7), (0, 4), (58, 7)]);
        let out = decode_counts(&src, 20, 10, V1).unwrap();
        let hits: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(hits, vec![3, 141]);
    }

    #[test]
    fn adjacent_events_and_overrun() {
        let src = pack(&[(0, 11), (0, 11), (0, 11)]);
        let out = decode_counts(&src, 4, 1, V1).unwrap();
        assert_eq!(out, vec![1, 1, 1, 0]);
        // a run past the strip is an error, not a panic
        let bad = pack(&[(100, 7), (0, 4)]);
        assert!(decode_counts(&bad, 4, 1, V1).is_err());
        // absurd codings are refused
        let odd = EerCoding {
            skip_bits: 30,
            horz_bits: 2,
            vert_bits: 2,
        };
        assert!(decode_counts(&src, 4, 1, odd).is_err());
    }

    #[test]
    fn metadata_items() {
        let (items, units) = parse_items(
            br#"<?xml version="1.0" encoding="utf - 8"?><metadata><item name="numberOfFrames">648</item><item name="exposureTime" unit="s">2.11</item><item name="commercialName">Falcon 4i</item><item name="x">Yes</item></metadata>"#,
        )
        .unwrap();
        assert_eq!(items["numberOfFrames"], 648);
        assert_eq!(items["exposureTime"], 2.11);
        assert_eq!(items["commercialName"], "Falcon 4i");
        assert_eq!(items["x"], true);
        assert_eq!(units["exposureTime"], "s");
        assert!(parse_items(b"<defects/>").is_none());
    }
}

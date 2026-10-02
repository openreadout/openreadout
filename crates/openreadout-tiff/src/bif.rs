//! Roche Ventana BIF whole-slide files (iScan HT, Coreo, DP 200/600): a BigTIFF whose pages are
//! named by their `ImageDescription` (`level=N mag=M quality=Q` pyramid levels, `Label_Image`,
//! `Probability_Image`, `Thumbnail`) and whose XMP packets (tag 700) hold the scanner's `iScan`
//! element and, on the full-resolution page, the `EncodeInfo` tile-stitching record. See
//! `docs/formats/tiff.md` § Ventana BIF.

use openreadout_core::model::{InstrumentInfo, ObjectiveInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::dataset::{Attachment, Level, TiffDataset, text_value, tiff_datetime};
use crate::{FORMAT_ID, tags};

/// XMP packet tag.
pub(crate) const XMP: u16 = 700;

/// The role of a BIF page, from its `ImageDescription`.
#[derive(Debug, Clone, PartialEq)]
pub enum BifPage {
    /// `level=N mag=M ...`: pyramid level `N` at magnification `M`.
    Level {
        level: u32,
        magnification: Option<f64>,
    },
    /// `Label_Image` / `Label Image`: the slide overview with its label.
    Label,
    /// `Probability_Image`: the tissue-detection map.
    Probability,
    /// `Thumbnail`.
    Thumbnail,
    /// Anything else.
    Other,
}

/// Classify a page by its description.
pub fn page_role(desc: &str) -> BifPage {
    let d = desc.trim();
    if let Some(rest) = d.strip_prefix("level=") {
        let mut parts = rest.split_whitespace();
        let Some(level) = parts.next().and_then(|v| v.parse().ok()) else {
            return BifPage::Other;
        };
        let magnification = parts
            .find_map(|p| p.strip_prefix("mag="))
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0);
        return BifPage::Level {
            level,
            magnification,
        };
    }
    match d {
        "Label_Image" | "Label Image" => BifPage::Label,
        "Probability_Image" => BifPage::Probability,
        "Thumbnail" => BifPage::Thumbnail,
        _ => BifPage::Other,
    }
}

/// The attributes of the `iScan` element of an XMP packet (the root, or a child of a
/// `Metadata` root, or anywhere below `EncodeInfo`), names as stored.
pub fn iscan_attributes(xmp: &str) -> Option<Map<String, Value>> {
    let start = xmp.find('<')?;
    let doc = roxmltree::Document::parse(xmp[start..].trim_end_matches('\0')).ok()?;
    let node = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "iScan")?;
    let mut m = Map::new();
    for a in node.attributes() {
        m.insert(a.name().to_string(), Value::String(a.value().to_string()));
    }
    Some(m)
}

/// The tile-stitching record of the full-resolution page (`EncodeInfo/SlideStitchInfo`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BifStitching {
    /// Tile rows and columns of the scanned area, and the tile size (`ImageInfo`).
    pub rows: Option<u32>,
    pub cols: Option<u32>,
    pub tile_width: Option<u32>,
    pub tile_height: Option<u32>,
    /// `TileJointInfo` entries, and how many record a non-zero `OverlapX` or `OverlapY`.
    pub joints: u32,
    pub overlapping_joints: u32,
    /// Largest absolute overlap recorded (pixels).
    pub max_overlap: u32,
    /// `Direction` values seen (`LEFT`, `RIGHT`, `UP`, ...).
    pub directions: Vec<String>,
}

impl BifStitching {
    fn to_json(&self) -> Value {
        json!({
            "rows": self.rows,
            "cols": self.cols,
            "tile_width": self.tile_width,
            "tile_height": self.tile_height,
            "joints": self.joints,
            "overlapping_joints": self.overlapping_joints,
            "max_overlap_px": self.max_overlap,
            "directions": self.directions,
        })
    }
}

/// `None` unless the packet holds an `EncodeInfo` element.
pub fn stitching(xmp: &str) -> Option<BifStitching> {
    let start = xmp.find('<')?;
    let doc = roxmltree::Document::parse(xmp[start..].trim_end_matches('\0')).ok()?;
    let enc = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "EncodeInfo")?;
    let num = |n: roxmltree::Node<'_, '_>, a: &str| {
        n.attribute(a).and_then(|v| v.trim().parse::<u32>().ok())
    };
    let mut s = BifStitching::default();
    if let Some(info) = enc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "ImageInfo")
    {
        s.rows = num(info, "NumRows");
        s.cols = num(info, "NumCols");
        s.tile_width = num(info, "Width");
        s.tile_height = num(info, "Height");
    }
    for j in enc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "TileJointInfo")
    {
        s.joints += 1;
        let ov = |a: &str| {
            j.attribute(a)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .map_or(0, i64::unsigned_abs)
        };
        let m = ov("OverlapX").max(ov("OverlapY"));
        if m > 0 {
            s.overlapping_joints += 1;
            s.max_overlap = s.max_overlap.max(u32::try_from(m).unwrap_or(u32::MAX));
        }
        if let Some(d) = j.attribute("Direction")
            && !s.directions.iter().any(|x| x == d)
        {
            s.directions.push(d.to_string());
        }
    }
    Some(s)
}

/// TIFF ImageDepth (SGI extension; volumetric BIF pages).
const IMAGE_DEPTH: u16 = 32997;

impl TiffDataset {
    /// The XMP packet of page `p` as text.
    fn xmp_text(&self, p: usize) -> Option<String> {
        let ifd = self.main().ifds.get(p)?;
        ifd.bytes(XMP)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .or_else(|| ifd.text(XMP).map(str::to_string))
    }

    /// The `iScan` attributes of the first page whose XMP packet has them (a Ventana BIF).
    pub(crate) fn bif_iscan(&self) -> Option<Map<String, Value>> {
        (0..self.main().ifds.len().min(16))
            .filter_map(|p| self.xmp_text(p))
            .filter(|x| x.contains("<iScan"))
            .find_map(|x| iscan_attributes(&x))
    }

    /// Ventana BIF: the `level=N` pages are one image and its pyramid levels; label,
    /// probability and thumbnail pages are attachments.
    pub(crate) fn build_bif(&mut self, iscan: &Map<String, Value>) -> Result<()> {
        let level_pages = self.bif_level_pages();
        let Some(&(0, base, mag0)) = level_pages.first() else {
            return Err(Error::corrupt(
                FORMAT_ID,
                "Ventana BIF file without a `level=0` page",
            ));
        };
        let depth = self.main().ifds[base].uint(IMAGE_DEPTH).unwrap_or(1);
        if depth > 1 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a volumetric BIF (ImageDepth {depth})"),
                "Z layers stored as TIFF ImageDepth are not decoded; export one layer with the scanner software.",
            ));
        }
        let base_dims = self.page_dims(base);
        let mut levels = Vec::new();
        for &(_, p, _) in &level_pages[1..] {
            let d = self.page_dims(p);
            if d.0 < base_dims.0 && d.1 < base_dims.1 {
                levels.push(Level {
                    width: d.0,
                    height: d.1,
                    page: Some(p),
                    sub_ifd: None,
                    pages: vec![p],
                });
            } else {
                self.attachments.push(Attachment {
                    role: "associated".into(),
                    page: p,
                });
            }
        }
        let mut info = self.push_slide(None, &[base], &[], levels)?;
        let attr = |k: &str| {
            iscan
                .get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let num = |k: &str| {
            attr(k)
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        if let Some(res) = num("ScanRes") {
            info.physical_size = PhysicalSize::micrometres(Some(res), Some(res), None);
        }
        let mag = num("Magnification").or(mag0);
        if mag.is_some() {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: mag,
                ..ObjectiveInfo::default()
            });
        }
        let page0 = self.main().ifds[base].clone();
        info.instrument = Some(InstrumentInfo {
            model: attr("ScannerModel").map(str::to_string),
            software: text_value(&page0, tags::SOFTWARE),
            software_version: attr("BuildVersion").map(str::to_string),
            ..InstrumentInfo::default()
        });
        info.acquired_at = text_value(&page0, tags::DATE_TIME).and_then(|d| tiff_datetime(&d));
        let mut bif = Map::new();
        bif.insert("iscan".into(), Value::Object(iscan.clone()));
        if let Some(s) = self.xmp_text(base).as_deref().and_then(stitching) {
            if s.overlapping_joints > 0 {
                self.notes.push(format!(
                    "Ventana BIF: {} of {} tile joints record an overlap (up to {} px); tiles are returned on their stored grid, not stitched, so content across those joints may be offset by up to {} px",
                    s.overlapping_joints, s.joints, s.max_overlap, s.max_overlap
                ));
            }
            bif.insert("stitching".into(), s.to_json());
        }
        info.extra.insert("bif".into(), Value::Object(bif));
        self.replace_last(info);
        self.set_provenance(&[
            ("images[].physical_size", Source::PriorArt),
            ("images[].objective.nominal_magnification", Source::PriorArt),
            ("images[].pyramid_levels", Source::PriorArt),
            ("images[].instrument", Source::Inferred),
        ]);
        Ok(())
    }

    /// The `level=N` pages as (level, page, magnification), by level; the other pages become
    /// attachments by their role.
    fn bif_level_pages(&mut self) -> Vec<(u32, usize, Option<f64>)> {
        let mut level_pages = Vec::new();
        for p in 0..self.main().ifds.len() {
            let desc = self.main().ifds[p]
                .text(tags::IMAGE_DESCRIPTION)
                .unwrap_or_default();
            let role = match page_role(desc) {
                BifPage::Level {
                    level,
                    magnification,
                } => {
                    level_pages.push((level, p, magnification));
                    continue;
                }
                BifPage::Label => "label",
                BifPage::Probability => "probability",
                BifPage::Thumbnail => "thumbnail",
                BifPage::Other => "associated",
            };
            self.attachments.push(Attachment {
                role: role.into(),
                page: p,
            });
        }
        level_pages.sort_by_key(|x| x.0);
        level_pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_packets() {
        assert_eq!(
            page_role("level=2 mag=10 quality=95"),
            BifPage::Level {
                level: 2,
                magnification: Some(10.0)
            }
        );
        assert_eq!(page_role("Label_Image"), BifPage::Label);
        assert_eq!(page_role("Probability_Image"), BifPage::Probability);
        assert_eq!(page_role("level=x"), BifPage::Other);
        let xmp = r#"<?xml version='1.0' encoding='utf-8' ?><EncodeInfo Ver="2"><SlideInfo><iScan Magnification="40" ScanRes="0.25" ScannerModel="VENTANA DP 200"/></SlideInfo>
<SlideStitchInfo><ImageInfo NumRows="21" NumCols="23" Width="1024" Height="1024">
<TileJointInfo Direction="LEFT" Tile1="1" Tile2="2" OverlapX="0" OverlapY="0"/>
<TileJointInfo Direction="UP" Tile1="1" Tile2="46" OverlapX="-3" OverlapY="24"/>
</ImageInfo></SlideStitchInfo></EncodeInfo>"#;
        let i = iscan_attributes(xmp).unwrap();
        assert_eq!(i["ScanRes"], "0.25");
        let s = stitching(xmp).unwrap();
        assert_eq!(
            (s.rows, s.cols, s.tile_width),
            (Some(21), Some(23), Some(1024))
        );
        assert_eq!((s.joints, s.overlapping_joints, s.max_overlap), (2, 1, 24));
        assert_eq!(s.directions, vec!["LEFT", "UP"]);
        assert!(stitching("<Metadata/>").is_none());
    }
}

//! Leica SCN whole-slide files (SCN400, SCN400F, Aperio Versa exports of that layout): a BigTIFF
//! whose first `ImageDescription` is an XML document (namespace
//! `http://www.leica-microsystems.com/scn/2010/10/01`) listing the slide's images and, per image,
//! which page holds each (resolution, channel, focal plane). See `docs/formats/tiff.md` § Leica SCN.

use openreadout_core::Result;
use openreadout_core::model::{
    ChannelInfo, ImageInfo, InstrumentInfo, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::Source;
use openreadout_core::xml::child;
use serde_json::{Map, Value, json};

use crate::dataset::{Attachment, Level, PlaneSrc, Series, TiffDataset, resolution_um};
use crate::decode::PageLayout;

/// The XML namespace that marks an SCN description.
pub const SCN_NAMESPACE: &str = "http://www.leica-microsystems.com/scn/2010/10/01";

/// One page of an SCN image: its size, resolution level `r`, channel `c`, focal plane `z` and
/// page index `ifd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScnDimension {
    pub size_x: u32,
    pub size_y: u32,
    pub r: u32,
    pub c: u32,
    pub z: u32,
    pub ifd: usize,
}

/// A channel of a fluorescence image (`scanSettings/channelSettings/channel`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScnChannel {
    pub index: u32,
    pub name: Option<String>,
    /// `rgb`, `#RRGGBB`.
    pub rgb: Option<String>,
    /// `fluorescenceCube/excitationFilter`, e.g. `BP 405/60`.
    pub excitation_filter: Option<String>,
    /// `fluorescenceCube/suppressionFilter`, e.g. `470/50`.
    pub suppression_filter: Option<String>,
    /// `fluorescenceCube/dichromaticMirror`.
    pub dichroic: Option<String>,
    /// `exposureTime` as stored (its unit is not recorded).
    pub exposure_raw: Option<f64>,
    pub ccd_gain: Option<f64>,
}

/// One `<image>` of the collection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScnImage {
    pub name: Option<String>,
    pub uuid: Option<String>,
    pub creation_date: Option<String>,
    pub device_model: Option<String>,
    pub device_version: Option<String>,
    pub size_x: u32,
    pub size_y: u32,
    pub dimensions: Vec<ScnDimension>,
    /// `view` size and offset on the slide, in nanometres.
    pub view_size_nm: Option<(f64, f64)>,
    pub view_offset_nm: Option<(f64, f64)>,
    pub spacing_z: Option<f64>,
    /// `objectiveSettings/objective`: the objective's magnification.
    pub objective: Option<f64>,
    pub numerical_aperture: Option<f64>,
    /// `brightfield`, `fluorescence`.
    pub illumination: Option<String>,
    pub channels: Vec<ScnChannel>,
}

/// The parsed description.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScnDocument {
    pub collection_name: Option<String>,
    pub uuid: Option<String>,
    /// Collection (slide) size in nanometres.
    pub collection_size_nm: Option<(f64, f64)>,
    /// `barcode` as stored (base64 text in the corpus).
    pub barcode: Option<String>,
    pub images: Vec<ScnImage>,
}

impl ScnImage {
    /// Pixel size (µm) of the full-resolution level: the view size over the pixel count.
    pub fn pixel_size_um(&self) -> (Option<f64>, Option<f64>) {
        let Some((w, h)) = self.view_size_nm else {
            return (None, None);
        };
        let f = |nm: f64, n: u32| (nm > 0.0 && n > 0).then(|| nm / f64::from(n) / 1000.0);
        (f(w, self.size_x), f(h, self.size_y))
    }

    /// True when the image covers the whole collection from its origin (a macro / overview
    /// image of the slide).
    pub fn covers_collection(&self, doc: &ScnDocument) -> bool {
        matches!(
            (self.view_size_nm, self.view_offset_nm, doc.collection_size_nm),
            (Some(v), Some((0.0, 0.0)) | None, Some(c)) if (v.0 - c.0).abs() < 1.0 && (v.1 - c.1).abs() < 1.0
        )
    }
}

/// The first child element named `name`.
/// `None` unless `desc` is an SCN description with at least one image.
pub fn parse_scn(desc: &str) -> Option<ScnDocument> {
    if !desc.contains(SCN_NAMESPACE) {
        return None;
    }
    let start = desc.find("<scn")?;
    let doc = roxmltree::Document::parse(&desc[start..]).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "scn" {
        return None;
    }
    let text = |n: Option<roxmltree::Node<'_, '_>>| {
        n.and_then(|n| n.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let num = |s: Option<&str>| {
        s.and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
    };
    let uint = |s: Option<&str>| s.and_then(|v| v.trim().parse::<u32>().ok());
    let collection = child(root, "collection")?;
    let pair =
        |n: roxmltree::Node<'_, '_>, a: &str, b: &str| num(n.attribute(a)).zip(num(n.attribute(b)));
    let mut out = ScnDocument {
        collection_name: collection.attribute("name").map(str::to_string),
        uuid: root.attribute("uuid").map(str::to_string),
        collection_size_nm: pair(collection, "sizeX", "sizeY"),
        barcode: text(child(collection, "barcode")),
        images: Vec::new(),
    };
    for im in collection
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "image")
    {
        let pixels = child(im, "pixels")?;
        let dimensions = pixels
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "dimension")
            .filter_map(|d| {
                Some(ScnDimension {
                    size_x: uint(d.attribute("sizeX"))?,
                    size_y: uint(d.attribute("sizeY"))?,
                    r: uint(d.attribute("r")).unwrap_or(0),
                    c: uint(d.attribute("c")).unwrap_or(0),
                    z: uint(d.attribute("z")).unwrap_or(0),
                    ifd: d.attribute("ifd")?.trim().parse().ok()?,
                })
            })
            .collect();
        let device = child(im, "device");
        let view = child(im, "view");
        let settings = child(im, "scanSettings");
        let illum = settings.and_then(|s| child(s, "illuminationSettings"));
        let channels = settings
            .and_then(|s| child(s, "channelSettings"))
            .map(|cs| {
                cs.children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "channel")
                    .enumerate()
                    .map(|(i, c)| {
                        let cube = child(c, "fluorescenceCube");
                        ScnChannel {
                            index: uint(c.attribute("index"))
                                .unwrap_or(u32::try_from(i).unwrap_or(u32::MAX)),
                            name: c.attribute("name").map(str::to_string),
                            rgb: c.attribute("rgb").map(str::to_string),
                            excitation_filter: text(
                                cube.and_then(|k| child(k, "excitationFilter")),
                            ),
                            suppression_filter: text(
                                cube.and_then(|k| child(k, "suppressionFilter")),
                            ),
                            dichroic: text(cube.and_then(|k| child(k, "dichromaticMirror"))),
                            exposure_raw: num(child(c, "exposureTime").and_then(|n| n.text())),
                            ccd_gain: num(child(c, "ccdGain").and_then(|n| n.text())),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.images.push(ScnImage {
            name: im.attribute("name").map(str::to_string),
            uuid: im.attribute("uuid").map(str::to_string),
            creation_date: text(child(im, "creationDate")),
            device_model: device
                .and_then(|d| d.attribute("model"))
                .map(str::to_string),
            device_version: device
                .and_then(|d| d.attribute("version"))
                .map(str::to_string),
            size_x: uint(pixels.attribute("sizeX")).unwrap_or(0),
            size_y: uint(pixels.attribute("sizeY")).unwrap_or(0),
            dimensions,
            view_size_nm: view.and_then(|v| pair(v, "sizeX", "sizeY")),
            view_offset_nm: view.and_then(|v| pair(v, "offsetX", "offsetY")),
            spacing_z: view.and_then(|v| num(v.attribute("spacingZ"))),
            objective: num(settings
                .and_then(|s| child(s, "objectiveSettings"))
                .and_then(|o| child(o, "objective"))
                .and_then(|o| o.text())),
            numerical_aperture: num(illum
                .and_then(|i| child(i, "numericalAperture"))
                .and_then(|n| n.text())),
            illumination: text(illum.and_then(|i| child(i, "illuminationSource"))),
            channels,
        });
    }
    (!out.images.is_empty()).then_some(out)
}

/// The centre wavelength (nm) of a filter named like `BP 405/60` or `470/50`.
pub fn filter_centre_nm(name: &str) -> Option<f64> {
    let s = name.trim().trim_start_matches("BP").trim();
    let (c, w) = s.split_once('/')?;
    let c: f64 = c.trim().parse().ok()?;
    let _: f64 = w.trim().parse().ok()?;
    (100.0..2000.0).contains(&c).then_some(c)
}

/// The band (start, end) in nm of a `centre/width` filter name.
pub fn filter_band_nm(name: &str) -> Option<(f64, f64)> {
    let s = name.trim().trim_start_matches("BP").trim();
    let (c, w) = s.split_once('/')?;
    let (c, w): (f64, f64) = (c.trim().parse().ok()?, w.trim().parse().ok()?);
    ((100.0..2000.0).contains(&c) && w > 0.0 && w < c).then(|| (c - w / 2.0, c + w / 2.0))
}

impl TiffDataset {
    /// Leica SCN: one image per `<image>` of the collection, its planes and pyramid levels
    /// from the `<dimension>` elements (page per resolution, channel and focal plane).
    pub(crate) fn build_scn(&mut self, doc: &ScnDocument) -> Result<()> {
        let n_pages = self.main().ifds.len();
        let mut used = vec![false; n_pages];
        for (ii, im) in doc.images.iter().enumerate() {
            self.push_scn_image(doc, ii, im, &mut used)?;
        }
        for (p, u) in used.iter().enumerate() {
            if !u {
                self.attachments.push(Attachment {
                    role: "associated".into(),
                    page: p,
                });
            }
        }
        let macros: Vec<usize> = doc
            .images
            .iter()
            .enumerate()
            .filter(|(_, im)| im.covers_collection(doc))
            .map(|(i, _)| i)
            .collect();
        if !macros.is_empty() && macros.len() < doc.images.len() {
            self.notes.push(format!(
                "Leica SCN: image(s) {macros:?} cover the whole slide (overview/macro images); the others are scanned regions"
            ));
        }
        self.set_provenance(&[
            ("images[].size_c", Source::PriorArt),
            ("images[].size_z", Source::PriorArt),
            ("images[].pyramid_levels", Source::PriorArt),
            ("images[].physical_size", Source::Inferred),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].objective", Source::Inferred),
            ("images[].channels[].name", Source::PriorArt),
            ("images[].channels[].excitation_nm", Source::Inferred),
        ]);
        Ok(())
    }

    /// SCN image `ii` as a series, its pages marked in `used`; an image whose pages are
    /// missing or of another layout is skipped with a note.
    fn push_scn_image(
        &mut self,
        doc: &ScnDocument,
        ii: usize,
        im: &ScnImage,
        used: &mut [bool],
    ) -> Result<()> {
        let order = self.main().header.byte_order;
        let planes = ScnPlanes::of(im);
        let bases = match self.scn_full_pages(im, &planes) {
            Ok(b) => b,
            Err(problem) => {
                self.notes.push(format!(
                    "SCN image {ii} ({}) is not read: {problem}",
                    im.name.as_deref().unwrap_or("unnamed"),
                ));
                return Ok(());
            }
        };
        let l = PageLayout::from_ifd(&self.main().ifds[bases[0]], order)?;
        let pt = match l.pixel_type() {
            Ok(pt) => pt,
            Err(e) => {
                self.notes.push(format!("SCN image {ii} is not read: {e}"));
                return Ok(());
            }
        };
        let (cs, zs) = (&planes.channels, &planes.focal_planes);
        let mut info = ImageInfo::new(self.series.len() as u32, l.width, l.height, pt);
        info.name.clone_from(&im.name);
        info.size_z = zs.len() as u32;
        let rgb_samples = cs.len() == 1 && l.samples_per_pixel > 1 && l.planar != 2;
        info.size_c = cs.len() as u32;
        info.samples_per_pixel = if rgb_samples {
            u32::from(l.samples_per_pixel)
        } else {
            1
        };
        if cs.len() == 1 && l.samples_per_pixel > 1 && l.planar == 2 {
            self.notes.push(format!(
                "SCN image {ii}: planar multi-sample pages are not read"
            ));
            return Ok(());
        }
        let (px, py) = im.pixel_size_um();
        let (rx, ry) = resolution_um(&self.main().ifds[bases[0]]);
        info.physical_size = PhysicalSize::micrometres(px.or(rx), py.or(ry), None);
        info.acquired_at.clone_from(&im.creation_date);
        info.instrument = scn_instrument(im);
        if im.objective.is_some() || im.numerical_aperture.is_some() {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: im.objective,
                lens_na: im.numerical_aperture,
                ..ObjectiveInfo::default()
            });
        }
        info.channels = scn_channels(im, cs);
        info.extra
            .insert("scn".into(), Value::Object(scn_extra(doc, im)));
        info.extra
            .insert("compression".into(), json!(l.compression_name()));
        if l.tiled {
            info.extra
                .insert("tile_size".into(), json!([l.chunk_width, l.chunk_height]));
        }
        let levels = self.scn_levels(ii, im, &planes, bases.len());
        for &p in bases
            .iter()
            .chain(levels.iter().flat_map(|l| l.pages.iter()))
        {
            used[p] = true;
        }
        info.pyramid_levels = 1 + levels.len() as u32;
        let planes = bases
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
            levels,
        });
        Ok(())
    }

    /// The page of each plane at full resolution (`r` 0), in (z, c) order with c fastest; the
    /// problem when a page is missing or not the size the description gives.
    fn scn_full_pages(
        &self,
        im: &ScnImage,
        planes: &ScnPlanes,
    ) -> std::result::Result<Vec<usize>, String> {
        let n_pages = self.main().ifds.len();
        let mut bases = Vec::new();
        let mut problem = None;
        for &z in &planes.focal_planes {
            for &c in &planes.channels {
                match planes.page(im, 0, c, z) {
                    Some(d) if d.ifd < n_pages && self.page_dims(d.ifd) == (d.size_x, d.size_y) => {
                        bases.push(d.ifd);
                    }
                    Some(d) => {
                        problem = Some(format!(
                            "its page {} (c {c}, z {z}) is missing or not {}x{}",
                            d.ifd, d.size_x, d.size_y
                        ));
                    }
                    None => problem = Some(format!("no page for c {c}, z {z}")),
                }
            }
        }
        match problem {
            Some(p) => Err(p),
            None if bases.is_empty() => Err("it lists no full-resolution page".into()),
            None => Ok(bases),
        }
    }

    /// Pyramid levels: every resolution `r` > 0 with a page of one size for each of the
    /// `planes` full-resolution planes; others are skipped with a note.
    fn scn_levels(
        &mut self,
        ii: usize,
        im: &ScnImage,
        planes: &ScnPlanes,
        bases: usize,
    ) -> Vec<Level> {
        let n_pages = self.main().ifds.len();
        let mut rs: Vec<u32> = im
            .dimensions
            .iter()
            .map(|d| d.r)
            .filter(|&r| r > 0)
            .collect();
        rs.sort_unstable();
        rs.dedup();
        let mut levels = Vec::new();
        for r in rs {
            let mut pages = Vec::new();
            let mut dims = None;
            for &z in &planes.focal_planes {
                for &c in &planes.channels {
                    if let Some(d) = planes.page(im, r, c, z)
                        && d.ifd < n_pages
                        && self.page_dims(d.ifd) == (d.size_x, d.size_y)
                        && dims.is_none_or(|x| x == (d.size_x, d.size_y))
                    {
                        dims = Some((d.size_x, d.size_y));
                        pages.push(d.ifd);
                    }
                }
            }
            match dims {
                Some((w, h)) if pages.len() == bases => levels.push(Level {
                    width: w,
                    height: h,
                    page: pages.first().copied(),
                    sub_ifd: None,
                    pages,
                }),
                _ => self.notes.push(format!(
                    "SCN image {ii}: resolution {r} does not list a page for every plane and is skipped"
                )),
            }
        }
        levels
    }
}

/// The channels and focal planes an SCN image has at full resolution, sorted.
struct ScnPlanes {
    channels: Vec<u32>,
    focal_planes: Vec<u32>,
}

impl ScnPlanes {
    fn of(im: &ScnImage) -> Self {
        let full: Vec<&ScnDimension> = im.dimensions.iter().filter(|d| d.r == 0).collect();
        let mut channels: Vec<u32> = full.iter().map(|d| d.c).collect();
        channels.sort_unstable();
        channels.dedup();
        let mut focal_planes: Vec<u32> = full.iter().map(|d| d.z).collect();
        focal_planes.sort_unstable();
        focal_planes.dedup();
        ScnPlanes {
            channels,
            focal_planes,
        }
    }

    /// The page of resolution `r`, channel `c` and focal plane `z`.
    fn page(&self, im: &ScnImage, r: u32, c: u32, z: u32) -> Option<ScnDimension> {
        im.dimensions
            .iter()
            .find(|d| d.r == r && d.c == c && d.z == z)
            .copied()
    }
}

/// The scanner (`device` model and version, first `;`-separated part).
fn scn_instrument(im: &ScnImage) -> Option<InstrumentInfo> {
    let first = |s: &str| s.split(';').next().unwrap_or(s).trim().to_string();
    let model = im.device_model.as_deref().map(first);
    let version = im.device_version.as_deref().map(first);
    (model.is_some() || version.is_some()).then(|| InstrumentInfo {
        manufacturer: Some("Leica".into()),
        model,
        software_version: version,
        ..InstrumentInfo::default()
    })
}

/// One channel per listed channel index, named and banded from the image's channel records.
fn scn_channels(im: &ScnImage, cs: &[u32]) -> Vec<ChannelInfo> {
    cs.iter()
        .enumerate()
        .map(|(i, &c)| {
            let ch = im.channels.iter().find(|k| k.index == c);
            let band = ch
                .and_then(|k| k.suppression_filter.as_deref())
                .and_then(filter_band_nm);
            ChannelInfo {
                index: i as u32,
                name: ch.and_then(|k| k.name.clone()),
                color: ch.and_then(|k| k.rgb.clone()),
                excitation_nm: ch
                    .and_then(|k| k.excitation_filter.as_deref())
                    .and_then(filter_centre_nm),
                emission_band_start_nm: band.map(|b| b.0),
                emission_band_end_nm: band.map(|b| b.1),
                emission_band_center_nm: band.map(|b| f64::midpoint(b.0, b.1)),
                acquisition_mode: im.illumination.clone(),
                ..ChannelInfo::default()
            }
        })
        .collect()
}

/// The image's description values kept under `extra.scn`.
fn scn_extra(doc: &ScnDocument, im: &ScnImage) -> Map<String, Value> {
    let mut scn = Map::new();
    scn.insert("uuid".into(), json!(im.uuid));
    if let Some(v) = im.view_size_nm {
        scn.insert("view_size_nm".into(), json!([v.0, v.1]));
    }
    if let Some(v) = im.view_offset_nm {
        scn.insert("view_offset_nm".into(), json!([v.0, v.1]));
    }
    scn.insert("spacing_z".into(), json!(im.spacing_z));
    scn.insert("illumination".into(), json!(im.illumination));
    scn.insert("device_model".into(), json!(im.device_model));
    scn.insert("device_version".into(), json!(im.device_version));
    scn.insert("covers_slide".into(), json!(im.covers_collection(doc)));
    if !im.channels.is_empty() {
        scn.insert(
            "channels".into(),
            json!(
                im.channels
                    .iter()
                    .map(|k| json!({
                        "index": k.index,
                        "name": k.name,
                        "excitation_filter": k.excitation_filter,
                        "dichroic": k.dichroic,
                        "suppression_filter": k.suppression_filter,
                        "exposure_time_raw": k.exposure_raw,
                        "ccd_gain": k.ccd_gain,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    scn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_images_dimensions_and_channels() {
        let xml = r##"<?xml version="1.0"?>
<scn xmlns="http://www.leica-microsystems.com/scn/2010/10/01" uuid="u0">
  <collection name="c" sizeX="1000" sizeY="2000">
    <barcode>QUJD</barcode>
    <image name="macro"><pixels sizeX="10" sizeY="20"><dimension sizeX="10" sizeY="20" r="0" ifd="0"/></pixels>
      <view sizeX="1000" sizeY="2000" offsetX="0" offsetY="0" spacingZ="0"/></image>
    <image name="main"><creationDate>2012-05-02T14:00:29.07Z</creationDate>
      <device model="Leica SCN400F;Leica SCN" version="1.5"/>
      <pixels sizeX="4" sizeY="2">
        <dimension sizeX="4" sizeY="2" r="0" c="0" ifd="1"/><dimension sizeX="4" sizeY="2" r="0" c="1" ifd="2"/>
        <dimension sizeX="2" sizeY="1" r="1" c="0" ifd="3"/><dimension sizeX="2" sizeY="1" r="1" c="1" ifd="4"/>
      </pixels>
      <view sizeX="2000" sizeY="1000" offsetX="5" offsetY="6" spacingZ="400"/>
      <scanSettings><objectiveSettings><objective>20</objective></objectiveSettings>
        <illuminationSettings><numericalAperture>0.4</numericalAperture><illuminationSource>fluorescence</illuminationSource></illuminationSettings>
        <channelSettings><channel index="0" name="405|Empty" rgb="#0000ff"><fluorescenceCube><excitationFilter>BP 405/60</excitationFilter><suppressionFilter>470/50</suppressionFilter></fluorescenceCube><exposureTime>105000</exposureTime></channel></channelSettings>
      </scanSettings></image>
  </collection>
</scn>"##;
        let d = parse_scn(xml).unwrap();
        assert_eq!(d.images.len(), 2);
        assert_eq!(d.barcode.as_deref(), Some("QUJD"));
        assert!(d.images[0].covers_collection(&d));
        let m = &d.images[1];
        assert!(!m.covers_collection(&d));
        assert_eq!(m.dimensions.len(), 4);
        assert_eq!(m.dimensions[3].ifd, 4);
        assert_eq!(m.pixel_size_um(), (Some(0.5), Some(0.5)));
        assert_eq!(m.objective, Some(20.0));
        assert_eq!(m.channels[0].exposure_raw, Some(105_000.0));
        assert_eq!(filter_centre_nm("BP 405/60"), Some(405.0));
        assert_eq!(filter_band_nm("470/50"), Some((445.0, 495.0)));
        assert!(parse_scn("<scn/>").is_none());
    }
}

//! OME-XML (the open OME data model, schemas 2008-02 … 2016-06) → the subset we normalize.
//! See `docs/formats/tiff.md` § OME-TIFF. Element and attribute names are the OME schema's.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{
    ChannelInfo, ImageInfo, InstrumentInfo, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::Source;
use openreadout_core::source::Fs;
use openreadout_core::xml::{child, children};
use openreadout_core::{Error, PixelType, Result};
use roxmltree::{Document, Node};
use serde_json::{Value, json};

use crate::container::{Ifd, TiffFile};
use crate::dataset::{Flavor, PlaneSrc, Series, TiffDataset, check_plane_count};
use crate::decode::PageLayout;
use crate::files::{FileSetMember, file_name_of, sibling};
use crate::{FORMAT_ID, tags};

/// Namespace of the `MapAnnotation`s in which our OME-XML export keeps normalized fields the OME
/// model cannot carry exactly (`book/src/guides/metadata.md` § OME-XML export). Linked from
/// the `Image` (keys `acquired_at`, `instrument.software`, `instrument.software_version`,
/// `objective.immersion`) or from a `Channel` (key `acquisition_mode`).
pub const NORMALIZED_NS: &str = "openreadout.dev/normalized";

/// The parsed OME document.
#[derive(Debug, Clone, Default)]
pub struct OmeDocument {
    /// `OME/@UUID` — identifies the file whose ImageDescription holds this document.
    pub uuid: Option<String>,
    pub creator: Option<String>,
    /// Schema namespace, e.g. `http://www.openmicroscopy.org/Schemas/OME/2016-06`.
    pub schema: Option<String>,
    pub binary_only: Option<OmeBinaryOnly>,
    pub images: Vec<OmeImage>,
    pub instruments: Vec<OmeInstrument>,
    pub annotations: Vec<OmeAnnotation>,
}

/// `BinaryOnly`: this file holds pixels only; the metadata lives in another file.
#[derive(Debug, Clone)]
pub struct OmeBinaryOnly {
    pub metadata_file: String,
    pub uuid: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct OmeImage {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub acquisition_date: Option<String>,
    pub instrument_ref: Option<String>,
    pub objective_ref: Option<String>,
    pub annotation_refs: Vec<String>,
    pub pixels: OmePixels,
}

#[derive(Debug, Clone, Default)]
pub struct OmePixels {
    /// e.g. `XYZCT`.
    pub dimension_order: String,
    /// OME pixel type name, e.g. `uint16`.
    pub pixel_type: String,
    pub size_x: u32,
    pub size_y: u32,
    pub size_z: u32,
    pub size_c: u32,
    pub size_t: u32,
    /// Micrometres.
    pub physical_size_x: Option<f64>,
    pub physical_size_y: Option<f64>,
    pub physical_size_z: Option<f64>,
    /// Seconds.
    pub time_increment: Option<f64>,
    pub significant_bits: Option<u32>,
    pub big_endian: Option<bool>,
    pub interleaved: Option<bool>,
    pub channels: Vec<OmeChannel>,
    pub tiff_data: Vec<OmeTiffData>,
    pub planes: Vec<OmePlane>,
    /// `MetadataOnly` present (no pixel data anywhere).
    pub metadata_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct OmeChannel {
    pub id: Option<String>,
    pub name: Option<String>,
    pub samples_per_pixel: u32,
    pub fluor: Option<String>,
    /// Nanometres.
    pub excitation_wavelength: Option<f64>,
    pub emission_wavelength: Option<f64>,
    /// Signed 32-bit RGBA as stored.
    pub color: Option<i32>,
    pub acquisition_mode: Option<String>,
    pub illumination_type: Option<String>,
    pub contrast_method: Option<String>,
    /// Micrometres.
    pub pinhole_size: Option<f64>,
    /// `LightPath/EmissionFilterRef/@ID`s.
    pub emission_filter_refs: Vec<String>,
    /// `FilterSetRef/@ID`.
    pub filter_set_ref: Option<String>,
    /// `AnnotationRef/@ID`s of the channel.
    pub annotation_refs: Vec<String>,
}

/// One `TiffData` element: which IFDs hold which planes.
#[derive(Debug, Clone, Default)]
pub struct OmeTiffData {
    pub ifd: Option<u32>,
    pub first_c: u32,
    pub first_z: u32,
    pub first_t: u32,
    pub plane_count: Option<u32>,
    /// `UUID` child text (`urn:uuid:…`), naming the file that holds the IFDs.
    pub uuid: Option<String>,
    /// `UUID/@FileName`.
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct OmePlane {
    pub the_c: u32,
    pub the_z: u32,
    pub the_t: u32,
    /// Seconds since the image's acquisition start.
    pub delta_t: Option<f64>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    /// Micrometres.
    pub position_x: Option<f64>,
    pub position_y: Option<f64>,
    pub position_z: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct OmeInstrument {
    pub id: String,
    pub microscope_manufacturer: Option<String>,
    pub microscope_model: Option<String>,
    pub objectives: Vec<OmeObjective>,
    pub detectors: Vec<OmeDetector>,
    pub filters: Vec<OmeFilter>,
    pub filter_sets: Vec<OmeFilterSet>,
}

/// `Filter` with its `TransmittanceRange` edges (nanometres).
#[derive(Debug, Clone, Default)]
pub struct OmeFilter {
    pub id: String,
    pub cut_in_nm: Option<f64>,
    pub cut_out_nm: Option<f64>,
}

/// `FilterSet`: the emission filters it names.
#[derive(Debug, Clone, Default)]
pub struct OmeFilterSet {
    pub id: String,
    pub emission_filter_refs: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct OmeObjective {
    pub id: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub nominal_magnification: Option<f64>,
    pub lens_na: Option<f64>,
    pub immersion: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct OmeDetector {
    pub id: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub detector_type: Option<String>,
}

/// One entry of `StructuredAnnotations`.
#[derive(Debug, Clone, Default)]
pub struct OmeAnnotation {
    pub id: String,
    /// Element name, e.g. `MapAnnotation`, `CommentAnnotation`, `XMLAnnotation`.
    pub kind: String,
    pub namespace: Option<String>,
    /// Text value; for `MapAnnotation` a JSON object of the `M/@K` pairs.
    pub value: serde_json::Value,
}

fn attr(n: Node<'_, '_>, name: &str) -> Option<String> {
    n.attribute(name)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
fn attr_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    n.attribute(name)?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
}
fn attr_u32(n: Node<'_, '_>, name: &str) -> Option<u32> {
    n.attribute(name)?.trim().parse().ok()
}
fn ref_ids(n: Node<'_, '_>, name: &'static str) -> Vec<String> {
    children(n, name).filter_map(|r| attr(r, "ID")).collect()
}
fn text_of(n: Node<'_, '_>) -> Option<String> {
    let t: String = n
        .descendants()
        .filter(Node::is_text)
        .filter_map(|d| d.text())
        .collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Length unit → factor to micrometres. `None` for non-physical units (`pixel`, ...).
pub fn length_to_um(unit: Option<&str>) -> Option<f64> {
    Some(match unit.map_or("µm", str::trim) {
        "µm" | "um" | "μm" | "micron" | "microns" | "micrometer" | "micrometre" => 1.0,
        "nm" | "nanometer" => 1e-3,
        "pm" => 1e-6,
        "\u{c5}" | "\u{212b}" | "angstrom" => 1e-4,
        "mm" | "millimeter" => 1e3,
        "cm" | "centimeter" => 1e4,
        "dm" => 1e5,
        "m" | "meter" | "metre" => 1e6,
        "km" => 1e9,
        "in" | "inch" => 25_400.0,
        "ft" => 304_800.0,
        _ => return None,
    })
}

/// Time unit → factor to seconds.
pub fn time_to_s(unit: Option<&str>) -> Option<f64> {
    Some(match unit.map_or("s", str::trim) {
        "s" | "sec" | "second" | "seconds" => 1.0,
        "ms" => 1e-3,
        "µs" | "us" | "μs" => 1e-6,
        "ns" => 1e-9,
        "ps" => 1e-12,
        "min" => 60.0,
        "h" => 3600.0,
        "d" => 86_400.0,
        _ => return None,
    })
}

fn length_um(n: Node<'_, '_>, value: &str, unit: &str) -> Option<f64> {
    let v = attr_f64(n, value)?;
    Some(v * length_to_um(n.attribute(unit))?)
}
fn time_s(n: Node<'_, '_>, value: &str, unit: &str) -> Option<f64> {
    let v = attr_f64(n, value)?;
    Some(v * time_to_s(n.attribute(unit))?)
}
fn wavelength_nm(n: Node<'_, '_>, value: &str, unit: &str) -> Option<f64> {
    let v = attr_f64(n, value)?;
    let u = n.attribute(unit).map_or("nm", str::trim);
    Some(
        v * if u == "nm" {
            1.0
        } else {
            length_to_um(Some(u))? * 1e3
        },
    )
}

/// Parse an OME-XML document. Returns `None` when the text is not XML with an `OME` root.
pub fn parse(xml: &str) -> Option<OmeDocument> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, opts).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "OME" {
        return None;
    }
    let mut out = OmeDocument {
        uuid: attr(root, "UUID"),
        creator: attr(root, "Creator"),
        schema: root.tag_name().namespace().map(str::to_string),
        ..Default::default()
    };
    if let Some(b) = child(root, "BinaryOnly") {
        out.binary_only = attr(b, "MetadataFile").map(|metadata_file| OmeBinaryOnly {
            metadata_file,
            uuid: attr(b, "UUID"),
        });
    }
    for inst in children(root, "Instrument") {
        let micro = child(inst, "Microscope");
        out.instruments.push(OmeInstrument {
            id: attr(inst, "ID").unwrap_or_default(),
            microscope_manufacturer: micro.and_then(|m| attr(m, "Manufacturer")),
            microscope_model: micro.and_then(|m| attr(m, "Model")),
            objectives: children(inst, "Objective")
                .map(|o| OmeObjective {
                    id: attr(o, "ID").unwrap_or_default(),
                    manufacturer: attr(o, "Manufacturer"),
                    model: attr(o, "Model"),
                    nominal_magnification: attr_f64(o, "NominalMagnification"),
                    lens_na: attr_f64(o, "LensNA"),
                    immersion: attr(o, "Immersion"),
                })
                .collect(),
            detectors: children(inst, "Detector")
                .map(|d| OmeDetector {
                    id: attr(d, "ID").unwrap_or_default(),
                    manufacturer: attr(d, "Manufacturer"),
                    model: attr(d, "Model"),
                    detector_type: attr(d, "Type"),
                })
                .collect(),
            filters: children(inst, "Filter")
                .map(|f| {
                    let r = child(f, "TransmittanceRange");
                    OmeFilter {
                        id: attr(f, "ID").unwrap_or_default(),
                        cut_in_nm: r.and_then(|r| wavelength_nm(r, "CutIn", "CutInUnit")),
                        cut_out_nm: r.and_then(|r| wavelength_nm(r, "CutOut", "CutOutUnit")),
                    }
                })
                .collect(),
            filter_sets: children(inst, "FilterSet")
                .map(|f| OmeFilterSet {
                    id: attr(f, "ID").unwrap_or_default(),
                    emission_filter_refs: ref_ids(f, "EmissionFilterRef"),
                })
                .collect(),
        });
    }
    for im in children(root, "Image") {
        let Some(px) = child(im, "Pixels") else {
            continue;
        };
        let pixels = OmePixels {
            dimension_order: attr(px, "DimensionOrder").unwrap_or_else(|| "XYZCT".into()),
            pixel_type: attr(px, "Type")
                .or_else(|| attr(px, "PixelType"))
                .unwrap_or_default(),
            size_x: attr_u32(px, "SizeX").unwrap_or(0),
            size_y: attr_u32(px, "SizeY").unwrap_or(0),
            size_z: attr_u32(px, "SizeZ").unwrap_or(1).max(1),
            size_c: attr_u32(px, "SizeC").unwrap_or(1).max(1),
            size_t: attr_u32(px, "SizeT").unwrap_or(1).max(1),
            physical_size_x: length_um(px, "PhysicalSizeX", "PhysicalSizeXUnit"),
            physical_size_y: length_um(px, "PhysicalSizeY", "PhysicalSizeYUnit"),
            physical_size_z: length_um(px, "PhysicalSizeZ", "PhysicalSizeZUnit"),
            time_increment: time_s(px, "TimeIncrement", "TimeIncrementUnit"),
            significant_bits: attr_u32(px, "SignificantBits"),
            big_endian: px.attribute("BigEndian").map(|v| v.trim() == "true"),
            interleaved: px.attribute("Interleaved").map(|v| v.trim() == "true"),
            channels: children(px, "Channel")
                .map(|c| OmeChannel {
                    id: attr(c, "ID"),
                    name: attr(c, "Name"),
                    samples_per_pixel: attr_u32(c, "SamplesPerPixel").unwrap_or(1).max(1),
                    fluor: attr(c, "Fluor"),
                    excitation_wavelength: wavelength_nm(
                        c,
                        "ExcitationWavelength",
                        "ExcitationWavelengthUnit",
                    ),
                    emission_wavelength: wavelength_nm(
                        c,
                        "EmissionWavelength",
                        "EmissionWavelengthUnit",
                    ),
                    color: c.attribute("Color").and_then(|v| {
                        let v = v.trim();
                        v.parse::<i32>()
                            .ok()
                            .or_else(|| v.parse::<u32>().ok().map(|u| u as i32))
                    }),
                    acquisition_mode: attr(c, "AcquisitionMode"),
                    illumination_type: attr(c, "IlluminationType"),
                    contrast_method: attr(c, "ContrastMethod"),
                    pinhole_size: length_um(c, "PinholeSize", "PinholeSizeUnit"),
                    emission_filter_refs: child(c, "LightPath")
                        .map(|l| ref_ids(l, "EmissionFilterRef"))
                        .unwrap_or_default(),
                    filter_set_ref: child(c, "FilterSetRef").and_then(|r| attr(r, "ID")),
                    annotation_refs: ref_ids(c, "AnnotationRef"),
                })
                .collect(),
            tiff_data: children(px, "TiffData")
                .map(|t| {
                    let u = child(t, "UUID");
                    OmeTiffData {
                        ifd: attr_u32(t, "IFD"),
                        first_c: attr_u32(t, "FirstC").unwrap_or(0),
                        first_z: attr_u32(t, "FirstZ").unwrap_or(0),
                        first_t: attr_u32(t, "FirstT").unwrap_or(0),
                        plane_count: attr_u32(t, "PlaneCount").or_else(|| attr_u32(t, "NumPlanes")),
                        uuid: u.and_then(text_of),
                        file_name: u.and_then(|u| attr(u, "FileName")),
                    }
                })
                .collect(),
            planes: children(px, "Plane")
                .map(|p| OmePlane {
                    the_c: attr_u32(p, "TheC").unwrap_or(0),
                    the_z: attr_u32(p, "TheZ").unwrap_or(0),
                    the_t: attr_u32(p, "TheT").unwrap_or(0),
                    delta_t: time_s(p, "DeltaT", "DeltaTUnit"),
                    exposure_time: time_s(p, "ExposureTime", "ExposureTimeUnit"),
                    position_x: length_um(p, "PositionX", "PositionXUnit"),
                    position_y: length_um(p, "PositionY", "PositionYUnit"),
                    position_z: length_um(p, "PositionZ", "PositionZUnit"),
                })
                .collect(),
            metadata_only: child(px, "MetadataOnly").is_some(),
        };
        out.images.push(OmeImage {
            id: attr(im, "ID").unwrap_or_default(),
            name: attr(im, "Name"),
            description: child(im, "Description").and_then(text_of),
            acquisition_date: child(im, "AcquisitionDate")
                .or_else(|| child(im, "AcquiredDate"))
                .and_then(text_of),
            instrument_ref: child(im, "InstrumentRef").and_then(|r| attr(r, "ID")),
            objective_ref: child(im, "ObjectiveSettings").and_then(|r| attr(r, "ID")),
            annotation_refs: ref_ids(im, "AnnotationRef"),
            pixels,
        });
    }
    if let Some(sa) = child(root, "StructuredAnnotations") {
        for a in sa.children().filter(Node::is_element) {
            let value_node = child(a, "Value");
            let value = if a.tag_name().name() == "MapAnnotation" {
                let mut m = serde_json::Map::new();
                if let Some(v) = value_node {
                    for kv in children(v, "M") {
                        m.insert(
                            attr(kv, "K").unwrap_or_default(),
                            serde_json::Value::String(text_of(kv).unwrap_or_default()),
                        );
                    }
                }
                serde_json::Value::Object(m)
            } else {
                value_node
                    .and_then(text_of)
                    .map_or(serde_json::Value::Null, |t| {
                        let mut t = t;
                        if t.len() > 4096 {
                            let mut cut = 4096;
                            while !t.is_char_boundary(cut) {
                                cut -= 1;
                            }
                            t.truncate(cut);
                            t.push('…');
                        }
                        serde_json::Value::String(t)
                    })
            };
            out.annotations.push(OmeAnnotation {
                id: attr(a, "ID").unwrap_or_default(),
                kind: a.tag_name().name().to_string(),
                namespace: attr(a, "Namespace"),
                value,
            });
        }
    }
    Some(out)
}

impl OmeDocument {
    /// The `Instrument` an image refers to (else the first one).
    pub fn instrument_of(&self, img: &OmeImage) -> Option<&OmeInstrument> {
        img.instrument_ref
            .as_deref()
            .and_then(|r| self.instruments.iter().find(|i| i.id == r))
            .or_else(|| self.instruments.first())
    }

    /// Value of `key` in the first `MapAnnotation` of namespace [`NORMALIZED_NS`] among `refs`.
    pub fn normalized_value(&self, refs: &[String], key: &str) -> Option<String> {
        self.annotations
            .iter()
            .filter(|a| {
                a.kind == "MapAnnotation"
                    && a.namespace.as_deref() == Some(NORMALIZED_NS)
                    && refs.contains(&a.id)
            })
            .find_map(|a| a.value.get(key)?.as_str().map(str::to_string))
            .filter(|v| !v.trim().is_empty())
    }

    /// `AcquisitionDate`, or the full-precision time our export keeps in the image's annotation
    /// when it agrees with `AcquisitionDate` to the second.
    pub fn acquired_at(&self, img: &OmeImage) -> Option<String> {
        let date = img.acquisition_date.clone();
        let exact = self.normalized_value(&img.annotation_refs, "acquired_at");
        match (date, exact) {
            (Some(d), Some(e)) if d.get(..19).is_some() && d.get(..19) == e.get(..19) => Some(e),
            (None, Some(e)) => Some(e),
            (d, _) => d,
        }
    }

    /// The objective and instrument an image refers to, in the normalized model. Software and
    /// the exact immersion text come from our export's image annotation when it has one.
    pub fn objective_and_instrument(
        &self,
        img: &OmeImage,
    ) -> (Option<ObjectiveInfo>, Option<InstrumentInfo>) {
        let refs = &img.annotation_refs;
        let inst = self.instrument_of(img);
        let objective = inst
            .and_then(|inst| {
                img.objective_ref
                    .as_deref()
                    .and_then(|r| inst.objectives.iter().find(|o| o.id == r))
                    .or_else(|| inst.objectives.first())
            })
            .map(|o| {
                let immersion = self
                    .normalized_value(refs, "objective.immersion")
                    .or_else(|| o.immersion.clone());
                ObjectiveInfo {
                    model: o.model.clone(),
                    nominal_magnification: o.nominal_magnification,
                    lens_na: o.lens_na,
                    immersion,
                }
            });
        let software = self.normalized_value(refs, "instrument.software");
        let software_version = self.normalized_value(refs, "instrument.software_version");
        let instrument = InstrumentInfo {
            manufacturer: inst.and_then(|i| i.microscope_manufacturer.clone()),
            model: inst.and_then(|i| i.microscope_model.clone()),
            software,
            software_version,
            detector: inst.and_then(|i| {
                i.detectors
                    .first()
                    .and_then(|d| d.model.clone().or_else(|| d.detector_type.clone()))
            }),
        };
        let empty = instrument == InstrumentInfo::default();
        (objective, (!empty).then_some(instrument))
    }

    /// A channel's acquisition mode as a readable label: our export's own annotation when
    /// present, else `AcquisitionMode` and `ContrastMethod`
    /// (`openreadout_core::acquisition_mode::from_ome`).
    pub fn channel_mode(&self, ch: &OmeChannel) -> Option<String> {
        self.normalized_value(&ch.annotation_refs, "acquisition_mode")
            .or_else(|| {
                openreadout_core::acquisition_mode::from_ome(
                    ch.acquisition_mode.as_deref(),
                    ch.contrast_method.as_deref(),
                )
            })
    }

    /// A channel's detection band `[start, end]` in nm: the pass band its emission filters
    /// share (`LightPath/EmissionFilterRef`, else those of its `FilterSetRef`), i.e. the largest
    /// cut-in and the smallest cut-out. `None` unless every such filter has both edges and the
    /// band is not empty.
    pub fn channel_band(&self, inst: Option<&OmeInstrument>, ch: &OmeChannel) -> Option<[f64; 2]> {
        let inst = inst?;
        let refs = if ch.emission_filter_refs.is_empty() {
            ch.filter_set_ref
                .as_deref()
                .and_then(|r| inst.filter_sets.iter().find(|s| s.id == r))
                .map(|s| s.emission_filter_refs.as_slice())
                .unwrap_or_default()
        } else {
            ch.emission_filter_refs.as_slice()
        };
        if refs.is_empty() {
            return None;
        }
        let mut band = [f64::NEG_INFINITY, f64::INFINITY];
        for r in refs {
            let f = inst.filters.iter().find(|f| f.id == *r)?;
            band[0] = band[0].max(f.cut_in_nm?);
            band[1] = band[1].min(f.cut_out_nm?);
        }
        (band[0].is_finite() && band[1].is_finite() && band[0] > 0.0 && band[0] <= band[1])
            .then_some(band)
    }
}

impl OmePixels {
    /// Samples per pixel of the first channel (RGB channels declare 3).
    pub fn samples_per_pixel(&self) -> u32 {
        self.channels
            .first()
            .map_or(1, |c| c.samples_per_pixel.max(1))
    }
    /// `SizeC` counts samples; this is the number of channel entries (planes along C).
    pub fn effective_size_c(&self) -> u32 {
        let spp = self.samples_per_pixel();
        if spp > 1 && self.size_c.is_multiple_of(spp) {
            self.size_c / spp
        } else {
            self.size_c
        }
    }
    /// The three non-XY axes of `DimensionOrder`, fastest first (e.g. `ZCT`).
    pub fn plane_axes(&self) -> [char; 3] {
        let order: Vec<char> = self
            .dimension_order
            .chars()
            .filter(|c| matches!(c, 'Z' | 'C' | 'T'))
            .collect();
        if order.len() == 3 {
            [order[0], order[1], order[2]]
        } else {
            ['Z', 'C', 'T']
        }
    }
    /// Linear plane index (rasterisation order of `DimensionOrder`) → (c, z, t).
    pub fn plane_at(&self, linear: u64) -> (u32, u32, u32) {
        let (mut c, mut z, mut t) = (0, 0, 0);
        let mut rest = linear;
        for ax in self.plane_axes() {
            let size = u64::from(match ax {
                'C' => self.effective_size_c(),
                'Z' => self.size_z,
                _ => self.size_t,
            })
            .max(1);
            let v = (rest % size) as u32;
            rest /= size;
            match ax {
                'C' => c = v,
                'Z' => z = v,
                _ => t = v,
            }
        }
        (c, z, t)
    }
    /// (c, z, t) → linear plane index.
    pub fn linear_of(&self, c: u32, z: u32, t: u32) -> u64 {
        let mut idx = 0u64;
        let mut stride = 1u64;
        for ax in self.plane_axes() {
            let (v, size) = match ax {
                'C' => (c, self.effective_size_c()),
                'Z' => (z, self.size_z),
                _ => (t, self.size_t),
            };
            idx = idx.saturating_add(u64::from(v).saturating_mul(stride));
            stride = stride.saturating_mul(u64::from(size.max(1)));
        }
        idx
    }
    pub fn plane_count(&self) -> u64 {
        u64::from(self.effective_size_c())
            .saturating_mul(u64::from(self.size_z))
            .saturating_mul(u64::from(self.size_t))
    }
}

// ---------------------------------------------------------------- OME-TIFF data sets

/// How the samples of an OME image's pages map onto channels.
struct OmeSamples {
    /// Samples per stored pixel (the page's when OME says 1 but the page holds more).
    per_pixel: u32,
    /// Samples stored as separate planes (PlanarConfiguration 2): each sample is a channel.
    planar: bool,
    pixel_type: PixelType,
    size_c: u32,
    samples_per_pixel: u32,
}

impl OmeSamples {
    /// From the OME `Pixels` and the layout of the first page the image uses (when it opens).
    fn of(px: &OmePixels, layout: Option<&PageLayout>) -> Result<Self> {
        let spp = px.samples_per_pixel();
        let c_eff = px.effective_size_c();
        let page_spp = layout.map_or(spp, |l| u32::from(l.samples_per_pixel));
        let planar = layout.is_some_and(|l| l.planar == 2);
        let pixel_type = match layout.map(PageLayout::pixel_type) {
            Some(Ok(pt)) => pt,
            Some(Err(e)) => return Err(e),
            None => ome_pixel_type(&px.pixel_type)?,
        };
        let per_pixel =
            if page_spp > 1 && spp == 1 && px.size_c.is_multiple_of(page_spp) && c_eff == px.size_c
            {
                page_spp
            } else {
                spp
            };
        let (size_c, samples_per_pixel) = if per_pixel > 1 && planar {
            (px.effective_size_c() * per_pixel, 1)
        } else if per_pixel > 1 {
            (px.size_c / per_pixel, per_pixel)
        } else {
            (c_eff, 1)
        };
        Ok(OmeSamples {
            per_pixel,
            planar,
            pixel_type,
            size_c: size_c.max(1),
            samples_per_pixel,
        })
    }

    /// Each stored sample is its own channel.
    fn sample_channels(&self) -> bool {
        self.per_pixel > 1 && self.planar
    }
}

impl TiffDataset {
    /// OME-XML in page 0's `ImageDescription`: builds the OME images (from this file set, or
    /// from the metadata file a `BinaryOnly` element names). `false` when the XML does not
    /// parse or describes no image with pixel data here (the pages are then read as plain TIFF).
    pub(crate) fn try_ome(&mut self, page0: &Ifd, desc: &str) -> Result<bool> {
        let Some(doc) = parse(desc.trim_start_matches('\u{feff}')) else {
            self.notes.push(
                "ImageDescription looks like OME-XML but does not parse; showing pages as plain TIFF"
                    .into(),
            );
            return Ok(false);
        };
        let offset = page0
            .field(tags::IMAGE_DESCRIPTION)
            .and_then(|f| f.value_offset);
        self.files[0].uuid = doc.uuid.clone();
        if let Some(bo) = &doc.binary_only {
            return match self.load_binary_only(&bo.metadata_file) {
                Ok((meta_doc, meta_xml)) => {
                    self.flavor = Flavor::OmeTiff;
                    self.notes.push(format!(
                        "OME-TIFF BinaryOnly file: metadata read from '{}'",
                        bo.metadata_file
                    ));
                    let owner = self.files.len() - 1;
                    self.ome_xml = Some((owner, None, meta_xml));
                    self.build_ome(&meta_doc, Some(owner))?;
                    Ok(true)
                }
                Err(e) => {
                    self.notes.push(format!(
                        "OME-TIFF BinaryOnly file, but its metadata file '{}' could not be read ({e}); showing the pixels as plain TIFF",
                        bo.metadata_file
                    ));
                    Ok(false)
                }
            };
        }
        if doc.images.is_empty() {
            return Ok(false);
        }
        self.flavor = Flavor::OmeTiff;
        self.ome_xml = Some((0, offset, desc.to_string()));
        self.build_ome(&doc, Some(0))?;
        if self.series.is_empty() {
            self.notes.push("OME-XML describes no image with pixel data in this file set; showing pages as plain TIFF".into());
            return Ok(false);
        }
        Ok(true)
    }

    /// `BinaryOnly`: read the OME-XML from the metadata file (a companion XML or another
    /// OME-TIFF) and register it as a file-set member.
    fn load_binary_only(&mut self, name: &str) -> Result<(OmeDocument, String)> {
        let p = sibling(&self.path, name);
        if !self.fs.exists(&p) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("metadata file {} is missing", p.display()),
            ));
        }
        let (xml, mut member) = if crate::file_starts_like_tiff(&self.fs, &p)? {
            let (t, src) = TiffFile::open_in(&self.fs, &p)?;
            let xml = t
                .ifds
                .first()
                .and_then(|i| i.text(tags::IMAGE_DESCRIPTION))
                .unwrap_or_default()
                .to_string();
            (
                xml,
                FileSetMember::tiff(p.clone(), name.to_string(), (t, src)),
            )
        } else {
            let xml = read_companion(&self.fs, &p)?;
            let mut m = FileSetMember::pending(p.clone(), name.to_string(), None);
            m.metadata_only = true;
            (xml, m)
        };
        let doc =
            parse(&xml).ok_or_else(|| Error::corrupt(FORMAT_ID, "metadata file is not OME-XML"))?;
        member.uuid = doc.uuid.clone();
        self.files.push(member);
        Ok((doc, xml))
    }

    /// The images of an OME document whose pixels are in this file set (`xml_owner`: the
    /// member the XML came from, which a `TiffData` without a file reference points to).
    pub(crate) fn build_ome(&mut self, doc: &OmeDocument, xml_owner: Option<usize>) -> Result<()> {
        self.ome_schema = schema_name(doc);
        self.ome_parsed = true;
        if let Some(owner) = xml_owner {
            self.adopt_renamed_file(doc, owner);
        }
        let mut skipped = 0;
        for img in &doc.images {
            if !self.push_ome_image(doc, img, xml_owner)? {
                skipped += 1;
            }
        }
        if skipped > 0 {
            self.notes.push(format!(
                "{skipped} OME image(s) without pixel data in this file set were skipped"
            ));
        }
        if !doc.annotations.is_empty() {
            self.notes.push(format!(
                "{} OME structured annotation(s); image-linked ones are under images[].extra.annotations, all are in `info --view full`",
                doc.annotations.len()
            ));
        }
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::Spec),
            ("images[].size_c", Source::Spec),
            ("images[].size_t", Source::Spec),
            ("images[].dimension_order", Source::Spec),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::Spec),
            ("images[].time_increment_s", Source::Spec),
            ("images[].channels[]", Source::Spec),
            ("images[].objective", Source::Spec),
            ("images[].instrument", Source::Spec),
            ("images[].acquired_at", Source::Spec),
            ("images[].pyramid_levels", Source::Spec),
        ]);
        Ok(())
    }

    /// A single-file data set whose file was renamed after acquisition: the OME-XML has no
    /// root UUID and every `TiffData` names one file that does not exist next to us. That
    /// file's UUID is given to the file the XML came from.
    fn adopt_renamed_file(&mut self, doc: &OmeDocument, owner: usize) {
        if doc.uuid.is_some() {
            return;
        }
        let mut refs: Vec<(Option<&str>, Option<&str>)> = doc
            .images
            .iter()
            .flat_map(|i| i.pixels.tiff_data.iter())
            .map(|t| (t.uuid.as_deref(), t.file_name.as_deref()))
            .collect();
        refs.sort_unstable();
        refs.dedup();
        if let [(Some(uuid), Some(name))] = refs.as_slice()
            && *name != self.files[owner].name
            && !self.fs.exists(&sibling(&self.path, name))
        {
            self.files[owner].uuid = Some((*uuid).to_string());
            self.notes.push(format!(
                "the OME-XML names this file '{name}'; it was renamed to '{}' (planes are read from this file)",
                self.files[owner].name
            ));
        }
    }

    /// Adds one OME image as a series; `false` when it has no pixel data in this file set.
    fn push_ome_image(
        &mut self,
        doc: &OmeDocument,
        img: &OmeImage,
        xml_owner: Option<usize>,
    ) -> Result<bool> {
        let px = &img.pixels;
        if px.metadata_only || px.tiff_data.is_empty() {
            return Ok(false);
        }
        let pages = self.ome_pages(px, xml_owner)?;
        let Some((f0, p0)) = pages.iter().flatten().next().copied() else {
            return Ok(false);
        };
        let layout = if self.files[f0].ensure_open(&self.fs).is_ok() {
            self.layout(f0, p0).ok()
        } else {
            None
        };
        let samples = OmeSamples::of(px, layout.as_ref())?;
        let index = self.series.len() as u32;
        let mut info = ome_image_info(index, doc, img, &samples);
        let planes = ome_plane_table(px, &info, &samples, &pages)?;
        let missing = planes.iter().filter(|p| p.is_none()).count();
        if missing > 0 {
            self.notes.push(format!(
                "image {index}: {missing} of {} planes have no TiffData entry (reads of them fail; see `check`)",
                planes.len()
            ));
        }
        let files = self.member_names(&planes);
        if files.len() > 1 || files.first().is_some_and(|f| *f != self.files[0].name) {
            info.extra.insert("files".into(), json!(files));
        }
        let levels = self.sub_ifd_levels(f0, p0);
        info.pyramid_levels = 1 + levels.len() as u32;
        self.series.push(Series {
            info: info.finish(),
            planes,
            levels,
        });
        Ok(true)
    }

    /// The (member, page) of each plane in OME order (`Pixels` linear index), from the
    /// `TiffData` elements.
    fn ome_pages(
        &mut self,
        px: &OmePixels,
        xml_owner: Option<usize>,
    ) -> Result<Vec<Option<(usize, usize)>>> {
        let total = px.plane_count();
        check_plane_count(total)?;
        let mut pages: Vec<Option<(usize, usize)>> = vec![None; total as usize];
        for td in &px.tiff_data {
            let Some(file) =
                self.resolve_member(td.uuid.as_deref(), td.file_name.as_deref(), xml_owner)
            else {
                continue;
            };
            let ifd0 = td.ifd.unwrap_or(0) as usize;
            let count = match (td.plane_count, td.ifd) {
                (Some(n), _) => n as usize,
                (None, Some(_)) => 1,
                (None, None) => self.files[file]
                    .page_count(&self.fs)
                    .map_or(total as usize, |n| n.saturating_sub(ifd0)),
            };
            let first = px.linear_of(td.first_c, td.first_z, td.first_t);
            for i in 0..count {
                let l = first + i as u64;
                if l >= total {
                    break;
                }
                pages[l as usize] = Some((file, ifd0 + i));
            }
        }
        Ok(pages)
    }

    /// File-set member for an OME `TiffData`: UUID match first, then file name.
    fn resolve_member(
        &mut self,
        uuid: Option<&str>,
        name: Option<&str>,
        xml_owner: Option<usize>,
    ) -> Option<usize> {
        if let Some(u) = uuid
            && let Some(i) = self
                .files
                .iter()
                .position(|f| f.uuid.as_deref() == Some(u) && !f.metadata_only)
        {
            return Some(i);
        }
        if let Some(n) = name {
            let leaf = Path::new(n)
                .file_name()
                .map_or_else(|| n.to_string(), |s| s.to_string_lossy().to_string());
            if let Some(i) = self
                .files
                .iter()
                .position(|f| !f.metadata_only && file_name_of(&f.path) == leaf)
            {
                if self.files[i].uuid.is_none() {
                    self.files[i].uuid = uuid.map(str::to_string);
                }
                return Some(i);
            }
            self.files.push(FileSetMember::pending(
                sibling(&self.path, n),
                n.to_string(),
                uuid.map(str::to_string),
            ));
            return Some(self.files.len() - 1);
        }
        if uuid.is_some() {
            return None; // a UUID without a FileName that is not ours: unresolvable
        }
        xml_owner.filter(|&i| !self.files[i].metadata_only)
    }

    /// Names of the members the planes are read from, in member order.
    fn member_names(&self, planes: &[Option<PlaneSrc>]) -> Vec<String> {
        let mut v: Vec<usize> = planes
            .iter()
            .flatten()
            .map(|p| match p {
                PlaneSrc::Page { file, .. }
                | PlaneSrc::Contiguous { file, .. }
                | PlaneSrc::Member { file, .. } => *file,
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v.iter().map(|&i| self.files[i].name.clone()).collect()
    }

    pub(crate) fn ome_schema(&self) -> Option<String> {
        if self.ome_parsed {
            return self.ome_schema.clone();
        }
        let (_, _, xml) = self.ome_xml.as_ref()?;
        schema_name(&parse(xml.trim_start_matches('\u{feff}'))?)
    }
}

/// The normalized image of an OME `Image`: sizes, calibration, channels, optics and extras
/// (planes and pyramid levels are added by the caller).
fn ome_image_info(
    index: u32,
    doc: &OmeDocument,
    img: &OmeImage,
    samples: &OmeSamples,
) -> ImageInfo {
    let px = &img.pixels;
    let mut info = ImageInfo::new(index, px.size_x, px.size_y, samples.pixel_type);
    info.name = img.name.clone();
    info.size_z = px.size_z;
    info.size_t = px.size_t;
    info.size_c = samples.size_c;
    info.samples_per_pixel = samples.samples_per_pixel;
    info.dimension_order = if px.dimension_order.len() == 5 {
        px.dimension_order.clone()
    } else {
        "XYZCT".into()
    };
    info.physical_size =
        PhysicalSize::micrometres(px.physical_size_x, px.physical_size_y, px.physical_size_z);
    info.time_increment_s = px.time_increment.or_else(|| plane_time_increment(px));
    info.acquired_at = doc.acquired_at(img);
    info.channels = ome_channels(doc, img, samples, info.size_c);
    let (objective, instrument) = doc.objective_and_instrument(img);
    info.objective = objective;
    info.instrument = instrument;
    ome_extras(&mut info, img, doc);
    info
}

/// The time increment from the first and last `Plane` `DeltaT` of channel 0, Z 0.
fn plane_time_increment(px: &OmePixels) -> Option<f64> {
    let mut dts: Vec<(u32, f64)> = px
        .planes
        .iter()
        .filter(|p| p.the_c == 0 && p.the_z == 0)
        .filter_map(|p| p.delta_t.map(|d| (p.the_t, d)))
        .collect();
    dts.sort_by_key(|x| x.0);
    dts.dedup_by_key(|x| x.0);
    (dts.len() >= 2 && px.size_t > 1).then(|| {
        let (a, b) = (dts[0], dts[dts.len() - 1]);
        (b.1 - a.1) / f64::from(b.0 - a.0)
    })
}

/// One channel per plane-channel; for planar multi-sample pages each sample is a channel.
fn ome_channels(
    doc: &OmeDocument,
    img: &OmeImage,
    samples: &OmeSamples,
    size_c: u32,
) -> Vec<ChannelInfo> {
    let px = &img.pixels;
    let inst = doc.instrument_of(img);
    (0..size_c)
        .map(|ci| {
            let oc = if samples.sample_channels() {
                px.channels.get((ci / samples.per_pixel) as usize)
            } else {
                px.channels.get(ci as usize)
            };
            let exposure = px
                .planes
                .iter()
                .find(|p| p.the_c == ci && p.exposure_time.is_some())
                .and_then(|p| p.exposure_time)
                .map(|s| s * 1e3);
            let mut ch = ChannelInfo {
                index: ci,
                name: oc.and_then(|c| c.name.clone()),
                fluorophore: oc.and_then(|c| c.fluor.clone()),
                excitation_nm: oc.and_then(|c| c.excitation_wavelength),
                emission_nm: oc.and_then(|c| c.emission_wavelength),
                emission_range_nm: None,
                color: oc.and_then(|c| c.color).map(ome_color),
                acquisition_mode: oc.and_then(|c| doc.channel_mode(c)),
                exposure_ms: exposure,
                ..ChannelInfo::default()
            };
            if let Some([a, b]) = oc.and_then(|c| doc.channel_band(inst, c)) {
                ch.set_band(a, b);
            }
            ch
        })
        .collect()
}

/// The plane table in our (c, z, t) order, from the pages in OME order.
fn ome_plane_table(
    px: &OmePixels,
    info: &ImageInfo,
    samples: &OmeSamples,
    pages: &[Option<(usize, usize)>],
) -> Result<Vec<Option<PlaneSrc>>> {
    let n = u64::from(info.size_c)
        .saturating_mul(u64::from(info.size_z))
        .saturating_mul(u64::from(info.size_t));
    check_plane_count(n)?;
    let mut planes = vec![None; n as usize];
    for t in 0..info.size_t {
        for z in 0..info.size_z {
            for c in 0..info.size_c {
                let (c_ome, sample) = if samples.sample_channels() {
                    (c / samples.per_pixel, Some((c % samples.per_pixel) as u16))
                } else {
                    (c, None)
                };
                let l = px.linear_of(c_ome, z, t) as usize;
                let slot = ((t as usize * info.size_z as usize) + z as usize)
                    * info.size_c as usize
                    + c as usize;
                planes[slot] = pages
                    .get(l)
                    .copied()
                    .flatten()
                    .map(|(file, page)| PlaneSrc::Page { file, page, sample });
            }
        }
    }
    Ok(planes)
}

fn ome_color(c: i32) -> String {
    let v = c as u32;
    format!(
        "#{:02X}{:02X}{:02X}",
        (v >> 24) & 0xFF,
        (v >> 16) & 0xFF,
        (v >> 8) & 0xFF
    )
}

fn ome_pixel_type(s: &str) -> Result<PixelType> {
    Ok(match s {
        "int8" => PixelType::Int8,
        "int16" => PixelType::Int16,
        "int32" => PixelType::Int32,
        "uint8" => PixelType::Uint8,
        "uint16" => PixelType::Uint16,
        "uint32" => PixelType::Uint32,
        "float" => PixelType::Float,
        "double" => PixelType::Double,
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("OME pixel type '{other}'"),
                "bit, complex and double-complex pixel types are not decoded.",
            ));
        }
    })
}

fn ome_extras(info: &mut ImageInfo, img: &OmeImage, doc: &OmeDocument) {
    if !img.id.is_empty() {
        info.extra.insert("ome_image_id".into(), json!(img.id));
    }
    if let Some(d) = &img.description {
        info.extra.insert("description".into(), json!(d));
    }
    let px = &img.pixels;
    if let Some(b) = px.significant_bits {
        info.extra.insert("significant_bits".into(), json!(b));
    }
    if let Some(p) = px
        .planes
        .iter()
        .find(|p| p.position_x.is_some() || p.position_y.is_some())
    {
        info.extra.insert(
            "stage_position_um".into(),
            json!({"x": p.position_x, "y": p.position_y, "z": p.position_z}),
        );
    }
    let mut dts: BTreeMap<u32, f64> = BTreeMap::new();
    for p in px.planes.iter().filter(|p| p.the_c == 0 && p.the_z == 0) {
        if let Some(d) = p.delta_t {
            dts.entry(p.the_t).or_insert(d);
        }
    }
    if dts.len() > 1 && dts.len() <= 10_000 {
        info.extra
            .insert("delta_t_s".into(), json!(dts.values().collect::<Vec<_>>()));
    }
    let anns: Vec<Value> = doc
        .annotations
        .iter()
        .filter(|a| img.annotation_refs.contains(&a.id))
        // our export's normalized fields are read back into the model, not listed again
        .filter(|a| a.namespace.as_deref() != Some(NORMALIZED_NS))
        .map(|a| json!({"id": a.id, "kind": a.kind, "namespace": a.namespace, "value": a.value}))
        .collect();
    if !anns.is_empty() {
        info.extra.insert("annotations".into(), Value::Array(anns));
    }
    if let Some(c) = &doc.creator {
        info.extra.insert("ome_creator".into(), json!(c));
    }
}

/// The schema file name of an OME-XML document (`2016-06/ome.xsd` style URIs cut to the last
/// segment).
fn schema_name(doc: &OmeDocument) -> Option<String> {
    doc.schema
        .as_ref()
        .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
}

/// Largest companion OME-XML document read (the same bound as an in-file `ImageDescription`).
const MAX_COMPANION_XML: u64 = 256 << 20;

/// Read a companion OME-XML file (`.companion.ome`), refusing one above [`MAX_COMPANION_XML`].
pub(crate) fn read_companion(fs: &Fs, path: &Path) -> Result<String> {
    let bytes = openreadout_core::bytes::read_file_capped_in(
        fs,
        path,
        MAX_COMPANION_XML,
        FORMAT_ID,
        "companion OME-XML file",
    )?;
    String::from_utf8(bytes).map_err(|e| {
        Error::io(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06" UUID="urn:uuid:a">
 <Instrument ID="Instrument:0"><Objective ID="Objective:0" NominalMagnification="63" LensNA="1.4" Immersion="Oil"/></Instrument>
 <Image ID="Image:0" Name="x"><AcquisitionDate>2020-01-02T03:04:05</AcquisitionDate>
  <ObjectiveSettings ID="Objective:0"/>
  <Pixels DimensionOrder="XYCZT" Type="uint16" SizeX="4" SizeY="3" SizeZ="2" SizeC="2" SizeT="1"
          PhysicalSizeX="100" PhysicalSizeXUnit="nm" PhysicalSizeZ="0.5" TimeIncrement="20" TimeIncrementUnit="ms">
   <Channel ID="Channel:0:0" Name="DAPI" EmissionWavelength="461" Color="-16776961"/>
   <Channel ID="Channel:0:1" Name="GFP" ExcitationWavelength="0.488" ExcitationWavelengthUnit="µm"/>
   <TiffData IFD="0" PlaneCount="4"><UUID FileName="x.ome.tif">urn:uuid:a</UUID></TiffData>
   <Plane TheC="0" TheZ="0" TheT="0" DeltaT="1.5" ExposureTime="10" ExposureTimeUnit="ms"/>
  </Pixels></Image>
 <StructuredAnnotations><MapAnnotation ID="Annotation:0" Namespace="n"><Value><M K="a">1</M></Value></MapAnnotation></StructuredAnnotations>
</OME>"#;

    #[test]
    fn parses_core_fields_and_units() {
        let d = parse(DOC).unwrap();
        assert_eq!(d.uuid.as_deref(), Some("urn:uuid:a"));
        let p = &d.images[0].pixels;
        assert_eq!(
            (p.size_x, p.size_y, p.size_z, p.size_c, p.size_t),
            (4, 3, 2, 2, 1)
        );
        assert!((p.physical_size_x.unwrap() - 0.1).abs() < 1e-12);
        assert_eq!(p.physical_size_z, Some(0.5));
        assert!((p.time_increment.unwrap() - 0.02).abs() < 1e-12);
        assert_eq!(p.channels[1].excitation_wavelength, Some(488.0));
        assert_eq!(p.tiff_data[0].file_name.as_deref(), Some("x.ome.tif"));
        assert_eq!(p.planes[0].exposure_time, Some(0.01));
        assert_eq!(d.annotations[0].value["a"], "1");
        assert_eq!(d.instruments[0].objectives[0].lens_na, Some(1.4));
    }

    /// Filters, filter sets, OME mode enumerations and our normalized annotations.
    const OPTICS: &str = r#"<?xml version="1.0"?>
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
 <Instrument ID="Instrument:0">
  <Microscope Manufacturer="Acme"/>
  <Objective ID="Objective:0" Model="40x" Immersion="Other"/>
  <FilterSet ID="FilterSet:0"><EmissionFilterRef ID="Filter:em"/></FilterSet>
  <Filter ID="Filter:em"><TransmittanceRange CutIn="0.5" CutInUnit="µm" CutOut="550"/></Filter>
  <Filter ID="Filter:lp"><TransmittanceRange CutIn="510" CutOut="700"/></Filter>
  <Filter ID="Filter:open"><TransmittanceRange CutIn="400"/></Filter>
 </Instrument>
 <Image ID="Image:0"><AcquisitionDate>2019-06-27T18:39:25.807Z</AcquisitionDate>
  <InstrumentRef ID="Instrument:0"/>
  <Pixels DimensionOrder="XYCZT" Type="uint8" SizeX="1" SizeY="1" SizeZ="1" SizeC="4" SizeT="1">
   <Channel ID="Channel:0:0" AcquisitionMode="LaserScanningConfocalMicroscopy" ContrastMethod="Fluorescence"><FilterSetRef ID="FilterSet:0"/></Channel>
   <Channel ID="Channel:0:1" AcquisitionMode="WideField" ContrastMethod="Phase">
    <LightPath><EmissionFilterRef ID="Filter:em"/><EmissionFilterRef ID="Filter:lp"/></LightPath></Channel>
   <Channel ID="Channel:0:2" AcquisitionMode="BrightField"><AnnotationRef ID="Annotation:c2"/>
    <LightPath><EmissionFilterRef ID="Filter:open"/></LightPath></Channel>
   <Channel ID="Channel:0:3"/>
  </Pixels>
  <AnnotationRef ID="Annotation:i0"/></Image>
 <StructuredAnnotations>
  <MapAnnotation ID="Annotation:i0" Namespace="openreadout.dev/normalized"><Value>
   <M K="acquired_at">2019-06-27T18:39:25.8078869Z</M><M K="instrument.software">ZEN</M>
   <M K="objective.immersion">Silicone</M></Value></MapAnnotation>
  <MapAnnotation ID="Annotation:c2" Namespace="openreadout.dev/normalized"><Value>
   <M K="acquisition_mode">Brightfield (RGB)</M></Value></MapAnnotation>
 </StructuredAnnotations>
</OME>"#;

    #[test]
    fn optics_and_normalized_annotations() {
        let d = parse(OPTICS).unwrap();
        let img = &d.images[0];
        let inst = d.instrument_of(img);
        let ch = &img.pixels.channels;
        // filter set → its emission filter; µm converted
        assert_eq!(d.channel_band(inst, &ch[0]), Some([500.0, 550.0]));
        // two filters in the light path: the band both pass
        assert_eq!(d.channel_band(inst, &ch[1]), Some([510.0, 550.0]));
        // a filter without a cut-out gives no band; no filter, no band
        assert_eq!(d.channel_band(inst, &ch[2]), None);
        assert_eq!(d.channel_band(inst, &ch[3]), None);
        assert_eq!(
            d.channel_mode(&ch[0]).as_deref(),
            Some("Laser Scanning Confocal Fluorescence")
        );
        assert_eq!(d.channel_mode(&ch[1]).as_deref(), Some("Phase Contrast"));
        assert_eq!(d.channel_mode(&ch[2]).as_deref(), Some("Brightfield (RGB)"));
        assert_eq!(d.channel_mode(&ch[3]), None);
        assert_eq!(
            d.acquired_at(img).as_deref(),
            Some("2019-06-27T18:39:25.8078869Z")
        );
        let (o, i) = d.objective_and_instrument(img);
        assert_eq!(o.unwrap().immersion.as_deref(), Some("Silicone"));
        let i = i.unwrap();
        assert_eq!(i.software.as_deref(), Some("ZEN"));
        assert_eq!(i.manufacturer.as_deref(), Some("Acme"));
        // an annotation that disagrees with AcquisitionDate is not used
        let mut other = img.clone();
        other.acquisition_date = Some("2020-01-01T00:00:00Z".into());
        assert_eq!(
            d.acquired_at(&other).as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
    }

    #[test]
    fn rasterisation_order() {
        let d = parse(DOC).unwrap();
        let p = &d.images[0].pixels;
        // XYCZT: C fastest, then Z.
        assert_eq!(p.plane_at(0), (0, 0, 0));
        assert_eq!(p.plane_at(1), (1, 0, 0));
        assert_eq!(p.plane_at(2), (0, 1, 0));
        assert_eq!(p.linear_of(1, 1, 0), 3);
    }
}

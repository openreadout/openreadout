//! The XML header: element tree → image nodes. See `docs/formats/lif.md` § XML model.

use openreadout_core::{Error, Result};
use roxmltree::{Document, Node};

use crate::FORMAT_ID;
use crate::xlef::{FrameRef, memory_frames};

/// Dimension identities from `DimensionDescription/@DimID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
    T,
    Lambda,
    Rotation,
    XtSlices,
    TSlices,
    Mosaic,
    Other(u32),
}

impl Axis {
    pub fn from_dim_id(id: u32) -> Self {
        match id {
            1 => Axis::X,
            2 => Axis::Y,
            3 => Axis::Z,
            4 => Axis::T,
            5 => Axis::Lambda,
            6 => Axis::Rotation,
            7 => Axis::XtSlices,
            8 => Axis::TSlices,
            10 => Axis::Mosaic,
            other => Axis::Other(other),
        }
    }
    pub fn name(self) -> String {
        match self {
            Axis::X => "x".into(),
            Axis::Y => "y".into(),
            Axis::Z => "z".into(),
            Axis::T => "t".into(),
            Axis::Lambda => "lambda".into(),
            Axis::Rotation => "rotation".into(),
            Axis::XtSlices => "xt_slices".into(),
            Axis::TSlices => "t_slices".into(),
            Axis::Mosaic => "mosaic".into(),
            Axis::Other(9) => "excitation_lambda".into(),
            Axis::Other(11) => "loop".into(),
            Axis::Other(n) => format!("dim{n}"),
        }
    }
}

/// One `DimensionDescription`.
#[derive(Debug, Clone)]
pub struct DimensionDesc {
    pub dim_id: u32,
    pub axis: Axis,
    pub count: u32,
    pub origin: f64,
    /// Total extent in `unit` (metres for space, seconds for time).
    pub length: f64,
    pub unit: String,
    /// Byte stride between consecutive elements along this axis.
    pub bytes_inc: u64,
}

impl DimensionDesc {
    /// Step between elements in the dimension's unit.
    pub fn step(&self) -> Option<f64> {
        if self.count > 1 && self.length.is_finite() {
            Some((self.length / f64::from(self.count - 1)).abs())
        } else {
            None
        }
    }
    /// Coordinate of element `i`: `origin + i * length / (count - 1)`.
    pub fn coordinate(&self, i: u32) -> f64 {
        if self.count > 1 {
            self.origin + self.length * f64::from(i) / f64::from(self.count - 1)
        } else {
            self.origin
        }
    }
}

/// One `ChannelDescription`.
#[derive(Debug, Clone)]
pub struct ChannelDesc {
    /// 0 = integer samples, 1 = floating point.
    pub data_type: u32,
    /// 0 gray, 1 red, 2 green, 3 blue.
    pub channel_tag: u32,
    pub resolution_bits: u32,
    /// Byte offset of this channel's first sample from the pixel origin.
    pub bytes_inc: u64,
    pub lut_name: String,
    pub min: f64,
    pub max: f64,
    /// Detection band `[start, end]` in nm from `ChannelAttachment`, if present.
    pub band_nm: Option<[f64; 2]>,
}

/// Acquisition settings found in the hardware-setting attachment.
#[derive(Debug, Clone, Default)]
pub struct HardwareInfo {
    pub objective_name: Option<String>,
    pub magnification: Option<f64>,
    pub numerical_aperture: Option<f64>,
    pub immersion: Option<String>,
    pub software: Option<String>,
    pub system_type_name: Option<String>,
    pub microscope_model: Option<String>,
    /// `Laser Scanning Confocal` or `Widefield`, from the kind of setting the attachment holds.
    pub acquisition_mode: Option<String>,
    /// Spectral detection windows of a λ scan (`LambdaDefinition/LambdaEmission`).
    pub lambda_windows: Option<LambdaWindows>,
}

/// `LambdaDefinition/LambdaEmission` of a λ scan: window `i` detects
/// `[begin_nm + i * step_nm, begin_nm + i * step_nm + bandwidth_nm]`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LambdaWindows {
    /// `LambdaDetectionBegin`: start of the first window, nm.
    pub begin_nm: f64,
    /// `LambdaDetectionStepSize`: offset from one window to the next, nm.
    pub step_nm: f64,
    /// `LambdaDetectionBandWidth`: width of every window, nm.
    pub bandwidth_nm: f64,
    /// `LambdaDetectionStepCount`: number of windows.
    pub step_count: u32,
}

/// The first valid `LambdaEmission` element under a hardware-setting attachment.
fn lambda_windows(att: Node<'_, '_>) -> Option<LambdaWindows> {
    let n = att.descendants().find(|n| {
        n.has_tag_name("LambdaEmission")
            && n.attribute("ValidLambdaDetectionDefinition") != Some("0")
    })?;
    let w = LambdaWindows {
        begin_nm: attr_f64(n, "LambdaDetectionBegin")?,
        step_nm: attr_f64(n, "LambdaDetectionStepSize")?,
        bandwidth_nm: attr_f64(n, "LambdaDetectionBandWidth")?,
        step_count: attr_u32(n, "LambdaDetectionStepCount")?,
    };
    (w.begin_nm.is_finite()
        && w.begin_nm > 0.0
        && w.step_nm.is_finite()
        && w.bandwidth_nm.is_finite()
        && w.bandwidth_nm > 0.0)
        .then_some(w)
}

/// One tile of a tile scan.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TilePosition {
    pub field_x: i64,
    pub field_y: i64,
    /// Stage position in metres.
    pub pos_x: f64,
    pub pos_y: f64,
    pub pos_z: f64,
}

/// `Attachment[@Name="TileScanInfo"]`: stage orientation flags and one entry per tile.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TileScan {
    /// Stage X runs against image X.
    pub flip_x: bool,
    /// Stage Y runs against image Y.
    pub flip_y: bool,
    /// Stage X maps to image Y and vice versa.
    pub swap_xy: bool,
    pub tiles: Vec<TilePosition>,
}

/// A FALCON FLIM/TCSPC element (`Data/SingleMoleculeDetection[@IsImage="true"]`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlimInfo {
    /// `RawData/Format`, e.g. `LMSCOMPRESSED`.
    pub raw_format: String,
    /// `RawData/Dimensions`: identifier and size, in file order.
    pub raw_dims: Vec<(String, u32)>,
    /// `RawData/VoxelSizeX`, `VoxelSizeY`, `VoxelSizeZ` in metres.
    pub voxel_size_m: [Option<f64>; 3],
    pub laser_pulse_frequency_hz: Option<f64>,
    pub clock_period_s: Option<f64>,
    pub pixel_time_s: Option<f64>,
}

impl FlimInfo {
    /// Size of a raw dimension (1 when absent).
    pub fn size(&self, id: &str) -> u32 {
        self.raw_dims
            .iter()
            .find(|(d, _)| d == id)
            .map_or(1, |(_, n)| (*n).max(1))
    }
    /// TCSPC histogram bins in one laser period: floor(1 / frequency / clock period).
    pub fn histogram_bins(&self) -> Option<u64> {
        let f = self.laser_pulse_frequency_hz?;
        let c = self.clock_period_s?;
        if f <= 0.0 || c <= 0.0 {
            return None;
        }
        let bins = (1.0 / f / c).floor();
        (bins.is_finite() && (1.0..1e12).contains(&bins)).then_some(bins as u64)
    }
}

/// An image element with everything we need to address its samples.
#[derive(Debug, Clone)]
pub struct ImageNode {
    pub name: String,
    /// Element names from the root to this image, joined with `/`.
    pub path: String,
    pub unique_id: Option<String>,
    pub memory_block_id: String,
    pub memory_size: u64,
    /// Channels in storage order (sorted by `BytesInc`).
    pub channels: Vec<ChannelDesc>,
    pub dimensions: Vec<DimensionDesc>,
    /// FILETIME values from `TimeStampList`, in file order.
    pub timestamps: Vec<u64>,
    pub hardware: HardwareInfo,
    pub tile_scan: Option<TileScan>,
    /// Set for FALCON FLIM/TCSPC elements; their raw photon data is not decoded.
    pub flim: Option<FlimInfo>,
    /// LIFEXT only: memory block id of the parent image in the matching `.lif`.
    pub parent_block_id: Option<String>,
    /// XLIF only: files holding the frames of this image's memory block.
    pub frames: Vec<FrameRef>,
}

impl ImageNode {
    pub fn dim(&self, axis: Axis) -> Option<&DimensionDesc> {
        self.dimensions.iter().find(|d| d.axis == axis)
    }
    pub fn count(&self, axis: Axis) -> u32 {
        self.dim(axis).map_or(1, |d| d.count.max(1))
    }
    pub fn inc(&self, axis: Axis) -> u64 {
        self.dim(axis).map_or(0, |d| d.bytes_inc)
    }
}

/// The acquisition mode a hardware-setting attachment records: a confocal setting definition
/// (LAS X) or a scanner setting with a pinhole (LAS AF) is laser-scanning confocal, a camera
/// setting definition widefield.
fn acquisition_mode(att: Node<'_, '_>) -> Option<&'static str> {
    let mut camera = false;
    for n in att.descendants() {
        if n.has_tag_name("ATLConfocalSettingDefinition")
            || (n.has_tag_name("ScannerSettingRecord")
                && n.attribute("Identifier") == Some("dblPinhole"))
        {
            return Some("Laser Scanning Confocal");
        }
        camera |= n.has_tag_name("ATLCameraSettingDefinition");
    }
    camera.then_some("Widefield")
}

/// LAS AF `HardwareSettingList` rows: `ScannerSettingRecord[@Identifier="SystemType"]` (the
/// system name) and `FilterSettingRecord[@Attribute="Objective" | "NumericalAperture"]`.
fn legacy_hardware(att: Node<'_, '_>, hw: &mut HardwareInfo) {
    let variant = |n: Node<'_, '_>| {
        n.attribute("Variant")
            .map(|v| v.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|v| !v.is_empty())
    };
    for n in att.descendants() {
        if n.has_tag_name("ScannerSettingRecord")
            && n.attribute("Identifier") == Some("SystemType")
            && hw.system_type_name.is_none()
        {
            hw.system_type_name = variant(n);
        } else if n.has_tag_name("FilterSettingRecord") {
            match n.attribute("Attribute") {
                Some("Objective") if hw.objective_name.is_none() => {
                    hw.objective_name = variant(n);
                    hw.magnification = hw.objective_name.as_deref().and_then(magnification_of);
                }
                Some("NumericalAperture") if hw.numerical_aperture.is_none() => {
                    hw.numerical_aperture = attr_f64(n, "Variant").filter(|v| *v > 0.0);
                }
                _ => {}
            }
        }
    }
}

/// `HCX PL APO lambda blue 63.0x1.40 OIL UV` → 63.
fn magnification_of(name: &str) -> Option<f64> {
    name.split_whitespace().find_map(|w| {
        w.split_once(['x', 'X'])
            .and_then(|(m, _)| m.parse::<f64>().ok())
            .filter(|m| *m > 0.0)
    })
}

fn attr_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    n.attribute(name).and_then(|v| v.trim().parse().ok())
}
fn attr_u32(n: Node<'_, '_>, name: &str) -> Option<u32> {
    n.attribute(name).and_then(|v| v.trim().parse().ok())
}
fn attr_u64(n: Node<'_, '_>, name: &str) -> Option<u64> {
    n.attribute(name).and_then(|v| v.trim().parse().ok())
}

/// Parse the XML header and collect every image node in document order.
pub fn parse_images(xml: &str) -> Result<Vec<ImageNode>> {
    let doc = Document::parse(xml)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("XML header does not parse: {e}")))?;
    let root = doc.root_element();
    let mut out = Vec::new();
    for el in root
        .children()
        .filter(|n| n.is_element() && n.has_tag_name("Element"))
    {
        walk_element(el, "", None, &mut out);
    }
    // LIFEXT: `ChildrenOf/@MemoryBlockID` names the parent image's memory block in the `.lif`.
    for group in root.children().filter(|n| n.has_tag_name("ChildrenOf")) {
        let parent = group.attribute("MemoryBlockID").unwrap_or("");
        for el in group
            .children()
            .filter(|n| n.is_element() && n.has_tag_name("Element"))
        {
            walk_element(el, parent, Some(parent), &mut out);
        }
    }
    Ok(out)
}

fn walk_element(
    el: Node<'_, '_>,
    parent_path: &str,
    parent_block: Option<&str>,
    out: &mut Vec<ImageNode>,
) {
    let name = el.attribute("Name").unwrap_or("").to_string();
    let path = if parent_path.is_empty() {
        name.clone()
    } else {
        format!("{parent_path}/{name}")
    };
    let memory = el.children().find(|n| n.has_tag_name("Memory"));
    let data = el.children().find(|n| n.has_tag_name("Data"));
    let uid = el.attribute("UniqueID");
    if let Some(image) = data.and_then(|d| d.children().find(|n| n.has_tag_name("Image"))) {
        if let Some(mut node) = parse_image(image, &name, &path, uid, memory) {
            node.parent_block_id = parent_block.map(str::to_string);
            out.push(node);
        }
    } else if let Some(smd) = data.and_then(|d| {
        d.children().find(|n| {
            n.has_tag_name("SingleMoleculeDetection") && n.attribute("IsImage") == Some("true")
        })
    }) {
        let (memory_block_id, memory_size) = memory_attrs(memory);
        out.push(ImageNode {
            name: name.clone(),
            path: path.clone(),
            unique_id: uid.map(str::to_string),
            memory_block_id,
            memory_size,
            channels: Vec::new(),
            dimensions: Vec::new(),
            timestamps: Vec::new(),
            hardware: HardwareInfo::default(),
            tile_scan: None,
            flim: Some(parse_flim(smd)),
            parent_block_id: parent_block.map(str::to_string),
            frames: Vec::new(),
        });
    }
    if let Some(children) = el.children().find(|n| n.has_tag_name("Children")) {
        for child in children
            .children()
            .filter(|n| n.is_element() && n.has_tag_name("Element"))
        {
            walk_element(child, &path, parent_block, out);
        }
    }
}

fn memory_attrs(memory: Option<Node<'_, '_>>) -> (String, u64) {
    memory
        .map(|m| {
            (
                m.attribute("MemoryBlockID").unwrap_or("").to_string(),
                attr_u64(m, "Size").unwrap_or(0),
            )
        })
        .unwrap_or_default()
}

fn child_text<'a>(n: Node<'a, '_>, tag: &str) -> Option<&'a str> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .map(str::trim)
}

fn child_text_f64(n: Node<'_, '_>, tag: &str) -> Option<f64> {
    child_text(n, tag).and_then(|t| t.parse().ok())
}

fn parse_flim(smd: Node<'_, '_>) -> FlimInfo {
    let raw = smd
        .children()
        .find(|n| n.has_tag_name("Dataset"))
        .and_then(|d| d.children().find(|n| n.has_tag_name("RawData")));
    let Some(raw) = raw else {
        return FlimInfo::default();
    };
    let raw_dims = raw
        .children()
        .find(|n| n.has_tag_name("Dimensions"))
        .map(|d| {
            d.children()
                .filter(|n| n.has_tag_name("Dimension"))
                .filter_map(|dim| {
                    let id = child_text(dim, "DimensionIdentifier")?.to_string();
                    let size = child_text(dim, "Size")?.parse().ok()?;
                    Some((id, size))
                })
                .collect()
        })
        .unwrap_or_default();
    FlimInfo {
        raw_format: child_text(raw, "Format").unwrap_or("").to_string(),
        raw_dims,
        voxel_size_m: [
            child_text_f64(raw, "VoxelSizeX"),
            child_text_f64(raw, "VoxelSizeY"),
            child_text_f64(raw, "VoxelSizeZ"),
        ],
        laser_pulse_frequency_hz: child_text_f64(raw, "LaserPulseFrequency"),
        clock_period_s: child_text_f64(raw, "ClockPeriod"),
        pixel_time_s: child_text_f64(raw, "PixelTime"),
    }
}

fn parse_tile_scan(att: Node<'_, '_>) -> TileScan {
    let flag = |k: &str| att.attribute(k).is_some_and(|v| v.trim() == "1");
    TileScan {
        flip_x: flag("FlipX"),
        flip_y: flag("FlipY"),
        swap_xy: flag("SwapXY"),
        tiles: att
            .children()
            .filter(|n| n.has_tag_name("Tile"))
            .map(|t| TilePosition {
                field_x: t
                    .attribute("FieldX")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0),
                field_y: t
                    .attribute("FieldY")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0),
                pos_x: attr_f64(t, "PosX").unwrap_or(0.0),
                pos_y: attr_f64(t, "PosY").unwrap_or(0.0),
                pos_z: attr_f64(t, "PosZ").unwrap_or(0.0),
            })
            .collect(),
    }
}

fn parse_image(
    image: Node<'_, '_>,
    name: &str,
    path: &str,
    unique_id: Option<&str>,
    memory: Option<Node<'_, '_>>,
) -> Option<ImageNode> {
    let desc = image
        .children()
        .find(|n| n.has_tag_name("ImageDescription"))?;
    let mut channels: Vec<ChannelDesc> = desc
        .children()
        .find(|n| n.has_tag_name("Channels"))
        .map(|c| {
            c.children()
                .filter(|n| n.has_tag_name("ChannelDescription"))
                .map(|n| ChannelDesc {
                    data_type: attr_u32(n, "DataType").unwrap_or(0),
                    channel_tag: attr_u32(n, "ChannelTag").unwrap_or(0),
                    resolution_bits: attr_u32(n, "Resolution").unwrap_or(8),
                    bytes_inc: attr_u64(n, "BytesInc").unwrap_or(0),
                    lut_name: n.attribute("LUTName").unwrap_or("").to_string(),
                    min: attr_f64(n, "Min").unwrap_or(0.0),
                    max: attr_f64(n, "Max").unwrap_or(0.0),
                    band_nm: None,
                })
                .collect()
        })
        .unwrap_or_default();
    let dimensions: Vec<DimensionDesc> = desc
        .children()
        .find(|n| n.has_tag_name("Dimensions"))
        .map(|d| {
            d.children()
                .filter(|n| n.has_tag_name("DimensionDescription"))
                .map(|n| {
                    let dim_id = attr_u32(n, "DimID").unwrap_or(0);
                    DimensionDesc {
                        dim_id,
                        axis: Axis::from_dim_id(dim_id),
                        count: attr_u32(n, "NumberOfElements").unwrap_or(1),
                        origin: attr_f64(n, "Origin").unwrap_or(0.0),
                        length: attr_f64(n, "Length").unwrap_or(0.0),
                        unit: n.attribute("Unit").unwrap_or("").to_string(),
                        bytes_inc: attr_u64(n, "BytesInc").unwrap_or(0),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if channels.is_empty() || dimensions.is_empty() {
        return None;
    }

    // Detection bands: ChannelAttachment/Band/Quantity (first two values are start/end in metres).
    if let Some(att) = image
        .children()
        .find(|n| n.has_tag_name("Attachment") && n.attribute("Name") == Some("ChannelAttachment"))
    {
        for (i, band) in att
            .children()
            .filter(|n| n.has_tag_name("Band"))
            .enumerate()
        {
            let q: Vec<f64> = band
                .children()
                .filter(|n| n.has_tag_name("Quantity") && n.attribute("Unit") == Some("m"))
                .filter_map(|n| attr_f64(n, "Value"))
                .collect();
            if q.len() >= 2
                && let Some(ch) = channels.get_mut(i)
            {
                ch.band_nm = Some([q[0] * 1e9, q[1] * 1e9]);
            }
        }
    }
    // Channels in storage order (by `BytesInc`), as they are laid out in the memory block.
    channels.sort_by_key(|c| c.bytes_inc);

    // `TimeStampList` holds hex FILETIMEs as text (LAS X) or `TimeStamp` elements with the two
    // 32-bit halves as `HighInteger` / `LowInteger` attributes (LAS AF).
    let list = image.children().find(|n| n.has_tag_name("TimeStampList"));
    let mut timestamps: Vec<u64> = list
        .and_then(|n| n.text())
        .map(|t| {
            t.split_whitespace()
                .filter_map(|h| u64::from_str_radix(h, 16).ok())
                .collect()
        })
        .unwrap_or_default();
    if timestamps.is_empty()
        && let Some(list) = list
    {
        timestamps = list
            .children()
            .filter(|n| n.has_tag_name("TimeStamp"))
            .filter_map(|n| {
                let hi = u64::from(attr_u32(n, "HighInteger")?);
                let lo = u64::from(attr_u32(n, "LowInteger")?);
                Some((hi << 32) | lo)
            })
            .collect();
    }

    let mut hardware = HardwareInfo::default();
    if let Some(att) = image.children().find(|n| {
        n.has_tag_name("Attachment")
            && matches!(
                n.attribute("Name"),
                Some("HardwareSetting" | "HardwareSettingList")
            )
    }) {
        hardware.acquisition_mode = acquisition_mode(att).map(str::to_string);
        legacy_hardware(att, &mut hardware);
    }
    if let Some(att) = image
        .children()
        .find(|n| n.has_tag_name("Attachment") && n.attribute("Name") == Some("HardwareSetting"))
    {
        hardware.software = att.attribute("Software").map(str::to_string);
        hardware.lambda_windows = lambda_windows(att);
        hardware.system_type_name = att
            .attribute("SystemTypeName")
            .map(str::to_string)
            .or(hardware.system_type_name.take());
        if let Some(obj) = att.descendants().find(|n| n.has_attribute("ObjectiveName")) {
            hardware.objective_name = obj
                .attribute("ObjectiveName")
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            hardware.magnification = attr_f64(obj, "Magnification");
            hardware.numerical_aperture = attr_f64(obj, "NumericalAperture");
            hardware.immersion = obj
                .attribute("Immersion")
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            hardware.microscope_model = obj
                .attribute("MicroscopeModel")
                .map(str::to_string)
                .filter(|s| !s.is_empty());
        }
    }

    let tile_scan = image
        .children()
        .find(|n| n.has_tag_name("Attachment") && n.attribute("Name") == Some("TileScanInfo"))
        .map(parse_tile_scan)
        .filter(|s| !s.tiles.is_empty());

    let (memory_block_id, memory_size) = memory_attrs(memory);
    let frames = memory.map(memory_frames).unwrap_or_default();

    Some(ImageNode {
        name: name.to_string(),
        path: path.to_string(),
        unique_id: unique_id.map(str::to_string),
        memory_block_id,
        memory_size,
        channels,
        dimensions,
        timestamps,
        hardware,
        tile_scan,
        flim: None,
        parent_block_id: None,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<LMSDataContainerHeader Version="2"><Element Name="proj.lif">
      <Data><Experiment Path="x"/></Data><Memory Size="0" MemoryBlockID="MemBlock_1"/>
      <Children>
        <Element Name="Spectral" UniqueID="u1"><Data><Image><ImageDescription>
          <Channels>
            <ChannelDescription DataType="0" ChannelTag="0" Resolution="16" BytesInc="16" LUTName="Green"/>
            <ChannelDescription DataType="0" ChannelTag="0" Resolution="16" BytesInc="0" LUTName="Red"/>
          </Channels>
          <Dimensions>
            <DimensionDescription DimID="1" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="2"/>
            <DimensionDescription DimID="2" NumberOfElements="2" Origin="0" Length="1e-6" Unit="m" BytesInc="4"/>
            <DimensionDescription DimID="5" NumberOfElements="3" Origin="4.2e-7" Length="2e-8" Unit="m" BytesInc="32"/>
          </Dimensions></ImageDescription>
          <Attachment Name="TileScanInfo" FlipX="1" FlipY="0" SwapXY="1">
            <Tile FieldX="0" FieldY="1" PosX="0.5" PosY="0.25" PosZ="0"/>
          </Attachment>
        </Image></Data><Memory Size="96" MemoryBlockID="MemBlock_2"/></Element>
        <Element Name="Flim"><Data><SingleMoleculeDetection IsImage="true"><Dataset><RawData>
          <Format>LMSCOMPRESSED</Format><VoxelSizeX>2e-7</VoxelSizeX><VoxelSizeY>2e-7</VoxelSizeY>
          <Dimensions><Dimension><DimensionIdentifier>X</DimensionIdentifier><Size>64</Size></Dimension>
          <Dimension><DimensionIdentifier>Y</DimensionIdentifier><Size>32</Size></Dimension></Dimensions>
          <ClockPeriod>9.696969697e-11</ClockPeriod><LaserPulseFrequency>19505000</LaserPulseFrequency>
          </RawData></Dataset></SingleMoleculeDetection></Data><Memory Size="10" MemoryBlockID="MemBlock_3"/></Element>
      </Children></Element></LMSDataContainerHeader>"#;

    #[test]
    fn parses_images_flim_and_tiles() {
        let nodes = parse_images(XML).unwrap();
        assert_eq!(nodes.len(), 2);
        let s = &nodes[0];
        assert_eq!(s.path, "proj.lif/Spectral");
        assert_eq!(s.channels[0].lut_name, "Red", "channels sorted by BytesInc");
        let l = s.dim(Axis::Lambda).unwrap();
        assert_eq!(l.count, 3);
        assert!((l.coordinate(2) - 4.4e-7).abs() < 1e-15);
        let ts = s.tile_scan.as_ref().unwrap();
        assert!(ts.flip_x && !ts.flip_y && ts.swap_xy);
        assert_eq!(ts.tiles[0].field_y, 1);
        assert!((ts.tiles[0].pos_x - 0.5).abs() < 1e-12);
        let f = nodes[1].flim.as_ref().unwrap();
        assert_eq!(f.raw_format, "LMSCOMPRESSED");
        assert_eq!((f.size("X"), f.size("Y"), f.size("Z")), (64, 32, 1));
        assert_eq!(f.histogram_bins(), Some(528));
        assert_eq!(nodes[1].memory_block_id, "MemBlock_3");
    }

    #[test]
    fn hardware_settings_mode_and_legacy_objective() {
        let image = |att: &str| {
            format!(
                r#"<LMSDataContainerHeader Version="2"><Element Name="p"><Data><Image><ImageDescription>
                <Channels><ChannelDescription DataType="0" Resolution="8" BytesInc="0"/></Channels>
                <Dimensions><DimensionDescription DimID="1" NumberOfElements="2" BytesInc="1"/>
                <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="2"/></Dimensions>
                </ImageDescription>{att}</Image></Data><Memory Size="4" MemoryBlockID="MemBlock_1"/></Element>
                </LMSDataContainerHeader>"#
            )
        };
        let legacy = image(
            r#"<Attachment Name="HardwareSettingList"><HardwareSetting Name="default"><ScannerSetting>
            <ScannerSettingRecord Identifier="SystemType" Variant="TCS SP5"/>
            <ScannerSettingRecord Identifier="dblPinhole" Variant="5.5E-05"/></ScannerSetting>
            <FilterSetting><FilterSettingRecord Attribute="NumericalAperture" Variant="1.4"/>
            <FilterSettingRecord Attribute="Objective" Variant="HCX PL APO lambda blue  63.0x1.40 OIL  UV"/>
            </FilterSetting></HardwareSetting></Attachment>"#,
        );
        let hw = &parse_images(&legacy).unwrap()[0].hardware;
        assert_eq!(
            hw.acquisition_mode.as_deref(),
            Some("Laser Scanning Confocal")
        );
        assert_eq!(hw.system_type_name.as_deref(), Some("TCS SP5"));
        assert_eq!(
            hw.objective_name.as_deref(),
            Some("HCX PL APO lambda blue 63.0x1.40 OIL UV")
        );
        assert_eq!(hw.magnification, Some(63.0));
        assert_eq!(hw.numerical_aperture, Some(1.4));
        let camera = image(
            r#"<Attachment Name="HardwareSetting" SystemTypeName="AF 6000LX"><ATLCameraSettingDefinition/></Attachment>"#,
        );
        let hw = &parse_images(&camera).unwrap()[0].hardware;
        assert_eq!(hw.acquisition_mode.as_deref(), Some("Widefield"));
        assert_eq!(hw.system_type_name.as_deref(), Some("AF 6000LX"));
        let lambda = image(
            r#"<Attachment Name="HardwareSetting"><ATLConfocalSettingDefinition><LambdaDefinition>
            <LambdaEmission ValidLambdaDetectionDefinition="1" LambdaDetectionBegin="420"
             LambdaDetectionEnd="720" LambdaDetectionStepSize="10" LambdaDetectionStepCount="29"
             LambdaDetectionBandWidth="20"/></LambdaDefinition></ATLConfocalSettingDefinition></Attachment>"#,
        );
        let hw = &parse_images(&lambda).unwrap()[0].hardware;
        assert_eq!(
            hw.lambda_windows,
            Some(LambdaWindows {
                begin_nm: 420.0,
                step_nm: 10.0,
                bandwidth_nm: 20.0,
                step_count: 29
            })
        );
        let confocal = image(
            r#"<Attachment Name="HardwareSetting"><ATLConfocalSettingDefinition/></Attachment>"#,
        );
        let hw = &parse_images(&confocal).unwrap()[0].hardware;
        assert_eq!(
            hw.acquisition_mode.as_deref(),
            Some("Laser Scanning Confocal")
        );
        assert_eq!(hw.lambda_windows, None);
    }

    #[test]
    fn lifext_children_of() {
        let xml = r#"<LMSDataContainerEnhancedHeader Version="1"><ChildrenOf MemoryBlockID="MemBlock_16">
          <Element Name="R 1_pmd_0"><Data><Image><ImageDescription>
            <Channels><ChannelDescription DataType="0" Resolution="8" BytesInc="0" LUTName="Gray"/></Channels>
            <Dimensions><DimensionDescription DimID="1" NumberOfElements="4" BytesInc="1"/>
            <DimensionDescription DimID="2" NumberOfElements="4" BytesInc="4"/></Dimensions>
          </ImageDescription></Image></Data><Memory Size="16" MemoryBlockID="MemBlock_17"/>
          <Children><Element Name="R 1_pmd_1"><Data><Image><ImageDescription>
            <Channels><ChannelDescription DataType="0" Resolution="8" BytesInc="0"/></Channels>
            <Dimensions><DimensionDescription DimID="1" NumberOfElements="2" BytesInc="1"/>
            <DimensionDescription DimID="2" NumberOfElements="2" BytesInc="2"/></Dimensions>
          </ImageDescription></Image></Data><Memory Size="4" MemoryBlockID="MemBlock_18"/></Element></Children>
          </Element></ChildrenOf></LMSDataContainerEnhancedHeader>"#;
        let nodes = parse_images(xml).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].path, "MemBlock_16/R 1_pmd_0");
        assert_eq!(nodes[0].parent_block_id.as_deref(), Some("MemBlock_16"));
        assert_eq!(nodes[1].path, "MemBlock_16/R 1_pmd_0/R 1_pmd_1");
        assert_eq!(nodes[1].parent_block_id.as_deref(), Some("MemBlock_16"));
    }

    #[test]
    fn timestamps_as_hex_text_or_filetime_halves() {
        let image = |list: &str| {
            format!(
                r#"<LMSDataContainerHeader Version="2"><Element Name="p"><Data><Image><ImageDescription>
                <Channels><ChannelDescription DataType="0" Resolution="8" BytesInc="0"/></Channels>
                <Dimensions><DimensionDescription DimID="1" NumberOfElements="2" BytesInc="1"/></Dimensions>
                </ImageDescription>{list}</Image></Data><Memory Size="2" MemoryBlockID="MemBlock_1"/></Element>
                </LMSDataContainerHeader>"#
            )
        };
        let hex = image("<TimeStampList>1d6f94d749e3e98 1d6f94d749e3e99</TimeStampList>");
        assert_eq!(
            parse_images(&hex).unwrap()[0].timestamps,
            vec![0x01d6_f94d_749e_3e98, 0x01d6_f94d_749e_3e99]
        );
        let halves = image(
            r#"<TimeStampList><TimeStamp HighInteger="30865858" LowInteger="1956527768"/></TimeStampList>"#,
        );
        assert_eq!(
            parse_images(&halves).unwrap()[0].timestamps,
            vec![(0x01d6_f9c2u64 << 32) | 0x749e_3e98]
        );
    }

    #[test]
    fn malformed_xml_is_corrupt() {
        assert!(matches!(
            parse_images("<LMSDataContainerHeader"),
            Err(Error::Corrupt { .. })
        ));
    }
}

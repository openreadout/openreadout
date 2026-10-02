//! Standalone Opera `.flex` files: a multi-page TIFF whose first IFD carries tag 65200, an XML
//! document describing the plate, the well and every page (`Well/Images/Image@BufferNo`).
//! A measurement folder of `.flex` files is one plate; one file opened on its own is one well.
//! Layout (our derivation): `docs/formats/opera-harmony.md`, *Standalone Opera .flex*.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use openreadout_core::model::{ChannelInfo, InstrumentInfo, ObjectiveInfo};
use openreadout_core::{Error, Fs, Result};
use openreadout_tiff::TiffFile;
use roxmltree::{Document, Node};
use serde_json::json;

use crate::HARMONY_FORMAT_ID;
use crate::model::{HcsPlate, PlaneFile, PlaneSlot, PlateBuilder, RawPlane};

/// TIFF tag holding the Flex XML document.
pub const TAG_FLEX_XML: u16 = 65200;
/// Largest number of `.flex` files one measurement folder may hold.
const MAX_FLEX_FILES: usize = 20_000;

/// Is `path` a `.flex` file name?
pub fn is_flex_name(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("flex"))
}

/// The `.flex` files of a folder, sorted by name.
pub fn flex_files(fs: &Fs, dir: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = fs
        .read_dir(dir)
        .map(|rd| {
            rd.filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| is_flex_name(p) && fs.is_file(p))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// One `Image` of a well: one page of the file.
#[derive(Debug, Clone, Default)]
struct FlexImage {
    page: u32,
    exposure_no: u32,
    stack: u32,
    sublayout: u32,
    array_name: Option<String>,
    exposure_s: Option<f64>,
    position_m: [Option<f64>; 3],
    date: Option<String>,
    resolution_m: [Option<f64>; 2],
    width: Option<u32>,
    height: Option<u32>,
    objective: Option<String>,
    light: Option<String>,
    binning: Option<String>,
}

fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text_of(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    text_of(n, name)
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn uint(n: Node<'_, '_>, name: &str) -> Option<u32> {
    text_of(n, name).and_then(|t| t.parse::<u32>().ok())
}

/// The Flex XML of a file (tag 65200 of its first page).
pub fn flex_xml(fs: &Fs, path: &Path) -> Result<String> {
    let (tf, _) = TiffFile::open_in(fs, path)?;
    let ifd = tf.ifds.first().ok_or_else(|| {
        Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("{} has no pages", path.display()),
        )
    })?;
    let bytes: Vec<u8> = match (ifd.text(TAG_FLEX_XML), ifd.bytes(TAG_FLEX_XML)) {
        (Some(t), _) => t.as_bytes().to_vec(),
        (None, Some(b)) => b.to_vec(),
        _ => {
            return Err(Error::unsupported(
                HARMONY_FORMAT_ID,
                format!("{} (a TIFF without the Flex XML tag 65200)", path.display()),
                "Only Opera .flex files carry their plate description in tag 65200; open this file with the TIFF reader (`--format tiff`).",
            ));
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.trim_end_matches('\0').to_string())
}

/// Magnification, numerical aperture and immersion refractive index of an objective.
type ObjectiveValues = (Option<f64>, Option<f64>, Option<f64>);

/// Everything one file says.
#[derive(Debug, Default)]
struct FlexFile {
    rel: String,
    row: u32,
    col: u32,
    images: Vec<FlexImage>,
    plate_name: Option<String>,
    barcode: Option<String>,
    rows: Option<u32>,
    columns: Option<u32>,
    start: Option<String>,
    version: Option<String>,
    device: Option<String>,
    lights: HashMap<String, f64>,
    combos: HashMap<String, Vec<String>>,
    objectives: HashMap<String, ObjectiveValues>,
    fields: HashMap<u32, (Option<f64>, Option<f64>)>,
}

fn parse_file(xml: &str, rel: String, path: &Path) -> Result<FlexFile> {
    let doc = Document::parse(xml).map_err(|e| {
        Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("{}: Flex XML: {e}", path.display()),
        )
    })?;
    let root = doc.root_element();
    let flex = child(root, "FLEX").ok_or_else(|| {
        Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("{}: the Flex XML has no FLEX element", path.display()),
        )
    })?;
    let mut f = FlexFile {
        rel,
        version: flex.attribute("version").map(str::to_string),
        device: flex.attribute("OperaDevice").map(str::to_string),
        ..FlexFile::default()
    };
    let arrays: Vec<Node> = child(root, "Arrays")
        .map(|a| a.children().filter(|c| c.has_tag_name("Array")).collect())
        .unwrap_or_default();
    if let Some(ls) = child(flex, "LightSources") {
        for l in ls.children().filter(|c| c.has_tag_name("LightSource")) {
            if let (Some(id), Some(w)) = (l.attribute("ID"), num(l, "Wavelength")) {
                f.lights.insert(id.to_string(), w);
            }
        }
    }
    if let Some(lc) = child(flex, "LightSourceCombinations") {
        for c in lc
            .children()
            .filter(|c| c.has_tag_name("LightSourceCombination"))
        {
            let refs = c
                .children()
                .filter(|r| r.has_tag_name("LightSourceRef"))
                .filter_map(|r| r.attribute("ID").map(str::to_string))
                .collect();
            if let Some(id) = c.attribute("ID") {
                f.combos.insert(id.to_string(), refs);
            }
        }
    }
    if let Some(os) = child(flex, "Objectives") {
        for o in os.children().filter(|c| c.has_tag_name("Objective")) {
            if let Some(id) = o.attribute("ID") {
                f.objectives.insert(
                    id.to_string(),
                    (
                        num(o, "Magnification"),
                        num(o, "NumAperture"),
                        num(o, "Immersion"),
                    ),
                );
            }
        }
    }
    if let Some(sl) = child(flex, "Sublayouts") {
        for s in sl.children().filter(|c| c.has_tag_name("Sublayout")) {
            for fld in s.children().filter(|c| c.has_tag_name("Field")) {
                if let Some(no) = fld.attribute("No").and_then(|v| v.parse().ok()) {
                    f.fields
                        .insert(no, (num(fld, "OffsetX"), num(fld, "OffsetY")));
                }
            }
        }
    }
    if let Some(p) = child(flex, "Plate") {
        f.plate_name = text_of(p, "PlateName");
        f.barcode = text_of(p, "Barcode");
        f.rows = uint(p, "XSize");
        f.columns = uint(p, "YSize");
        f.start = text_of(p, "StartTime");
    }
    let well = child(flex, "Well").ok_or_else(|| {
        Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("{}: the Flex XML has no Well element", path.display()),
        )
    })?;
    let wc = child(well, "WellCoordinate");
    f.row = wc
        .and_then(|w| w.attribute("Row"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    f.col = wc
        .and_then(|w| w.attribute("Col"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if f.row == 0 || f.col == 0 {
        return Err(Error::corrupt(
            HARMONY_FORMAT_ID,
            format!(
                "{}: the Flex XML has no well coordinate (WellCoordinate@Row/@Col, 1-based)",
                path.display()
            ),
        ));
    }
    if let Some(imgs) = child(well, "Images") {
        for im in imgs.children().filter(|c| c.has_tag_name("Image")) {
            let Some(page) = im.attribute("BufferNo").and_then(|v| v.parse::<u32>().ok()) else {
                continue;
            };
            let bin = match (uint(im, "CameraBinningX"), uint(im, "CameraBinningY")) {
                (Some(x), Some(y)) => Some(format!("{x}x{y}")),
                _ => None,
            };
            f.images.push(FlexImage {
                page,
                exposure_no: uint(im, "ExposureNo").unwrap_or(1),
                stack: uint(im, "Stack").unwrap_or(1),
                sublayout: uint(im, "Sublayout").unwrap_or(1),
                array_name: arrays
                    .get(page as usize)
                    .and_then(|a| a.attribute("Name"))
                    .map(str::to_string),
                exposure_s: num(im, "CameraExposureTime"),
                position_m: [
                    num(im, "PositionX"),
                    num(im, "PositionY"),
                    num(im, "PositionZ"),
                ],
                date: text_of(im, "DateTime"),
                resolution_m: [num(im, "ImageResolutionX"), num(im, "ImageResolutionY")],
                width: uint(im, "ImageWidth"),
                height: uint(im, "ImageHeight"),
                objective: text_of(im, "ObjectiveRef"),
                light: text_of(im, "LightSourceCombinationRef"),
                binning: bin,
            });
        }
    }
    if f.images.is_empty() {
        return Err(Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("{}: the Flex XML lists no images", path.display()),
        ));
    }
    Ok(f)
}

fn um(m: Option<f64>) -> Option<f64> {
    m.map(|v| v * 1e6)
}

/// Parse a `.flex` file (one well) or a folder of `.flex` files (the plate).
pub fn parse(fs: &Fs, path: &Path) -> Result<HcsPlate> {
    let (root, files) = if fs.is_dir(path) {
        (path.to_path_buf(), flex_files(fs, path))
    } else {
        (
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
            vec![path.to_path_buf()],
        )
    };
    if files.is_empty() {
        return Err(Error::unsupported(
            HARMONY_FORMAT_ID,
            format!("{} (no .flex files)", path.display()),
            "Open an Opera .flex file or the measurement folder that holds them.",
        ));
    }
    if files.len() > MAX_FLEX_FILES {
        return Err(Error::unsupported(
            HARMONY_FORMAT_ID,
            format!("a folder of {} .flex files", files.len()),
            "Open the .flex files of one well at a time.",
        ));
    }
    let mut parsed = Vec::with_capacity(files.len());
    let mut index_bytes = 0u64;
    for p in &files {
        let xml = flex_xml(fs, p)?;
        index_bytes += xml.len() as u64;
        let rel = p
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().to_string());
        parsed.push(parse_file(&xml, rel, p)?);
    }
    let first = &parsed[0];
    // channels: every ExposureNo, described by its first image
    let mut chans: BTreeMap<u32, FlexImage> = BTreeMap::new();
    for f in &parsed {
        for im in &f.images {
            chans.entry(im.exposure_no).or_insert_with(|| im.clone());
        }
    }
    let chan_index: HashMap<u32, usize> = chans.keys().enumerate().map(|(i, &e)| (e, i)).collect();
    let mut plate = HcsPlate {
        format_id: HARMONY_FORMAT_ID,
        source: path.to_path_buf(),
        root,
        index_bytes,
        format_version: first.version.as_ref().map(|v| format!("FLEX {v}")),
        id: first.barcode.clone(),
        plate_type: first.plate_name.clone(),
        started_at: first.start.clone(),
        ..HcsPlate::default()
    };
    for (i, (no, im)) in chans.iter().enumerate() {
        let wavelengths: Vec<f64> = im
            .light
            .as_ref()
            .and_then(|l| first.combos.get(l))
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| first.lights.get(id))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        plate.channels.push(ChannelInfo {
            index: i as u32,
            name: im.array_name.clone(),
            excitation_nm: (wavelengths.len() == 1).then(|| wavelengths[0]),
            exposure_ms: im.exposure_s.filter(|v| *v > 0.0).map(|s| s * 1000.0),
            ..ChannelInfo::default()
        });
        let mut ex = BTreeMap::new();
        ex.insert("exposure_no".into(), json!(no));
        if wavelengths.len() > 1 {
            ex.insert("light_source_wavelengths_nm".into(), json!(wavelengths));
        }
        if let Some(b) = &im.binning {
            ex.insert("binning".into(), json!(b));
        }
        plate.channel_extra.push(ex);
    }
    let im0 = &first.images[0];
    plate.size_x = im0.width.unwrap_or(0);
    plate.size_y = im0.height.unwrap_or(0);
    plate.pixel_size_um = [
        um(im0.resolution_m[0]).filter(|v| *v > 0.0),
        um(im0.resolution_m[1]).filter(|v| *v > 0.0),
    ];
    if let Some((mag, na, ri)) = im0.objective.as_ref().and_then(|o| first.objectives.get(o)) {
        plate.objective = Some(ObjectiveInfo {
            model: im0.objective.clone(),
            nominal_magnification: mag.filter(|v| *v > 0.0),
            lens_na: na.filter(|v| *v > 0.0),
            immersion: None,
        });
        if let Some(ri) = ri {
            plate
                .plate_extra
                .insert("objective_immersion_refractive_index".into(), json!(ri));
        }
    }
    plate.instrument = InstrumentInfo {
        manufacturer: Some("PerkinElmer".into()),
        model: first.device.clone(),
        software: Some("Opera (FLEX)".into()),
        software_version: first.version.clone(),
        detector: None,
    };
    let max_row = parsed.iter().map(|f| f.row).max().unwrap_or(1);
    let max_col = parsed.iter().map(|f| f.col).max().unwrap_or(1);
    let (rows, cols) = match (first.rows, first.columns) {
        (Some(r), Some(c)) if r >= max_row && c >= max_col && r > 0 && c > 0 => (r, c),
        _ => openreadout_core::plate::standard_dimensions(max_row, max_col),
    };
    if rows > crate::model::MAX_PLATE_ROWS || cols > crate::model::MAX_PLATE_COLUMNS {
        return Err(Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("plate geometry {rows} x {cols} is implausible"),
        ));
    }
    plate.rows = rows;
    plate.columns = cols;
    // Z step: the Z positions of the first field's first channel
    let mut zs: Vec<(u32, f64)> = first
        .images
        .iter()
        .filter(|i| i.exposure_no == im0.exposure_no && i.sublayout == im0.sublayout)
        .filter_map(|i| Some((i.stack, i.position_m[2]?)))
        .collect();
    zs.sort_by_key(|z| z.0);
    if zs.len() > 1 {
        plate.z_step_um = um(Some((zs[1].1 - zs[0].1).abs())).filter(|v| *v > 0.0);
    }
    let mut b = PlateBuilder::new(HARMONY_FORMAT_ID);
    for f in &parsed {
        for im in &f.images {
            let field = im.sublayout;
            let offs = f.fields.get(&field).copied().unwrap_or((None, None));
            b.push(RawPlane {
                row: f.row - 1,
                column: f.col - 1,
                field,
                channel: chan_index[&im.exposure_no],
                z: im.stack,
                t: 0,
                slot: PlaneSlot::File(PlaneFile {
                    name: f.rel.clone(),
                    page: im.page,
                    acquired_at: im.date.clone(),
                    position_um: [
                        um(im.position_m[0]),
                        um(im.position_m[1]),
                        um(im.position_m[2]),
                    ],
                    exposure_ms: im.exposure_s.map(|s| s * 1000.0),
                }),
                field_position_um: [um(offs.0), um(offs.1)],
            })?;
        }
    }
    plate.declared_wells = parsed.iter().map(|f| (f.row - 1, f.col - 1)).collect();
    b.finish(&mut plate)?;
    plate.notes.push(format!(
        "standalone Opera .flex: {} file(s), one well each; pages addressed by the Flex XML (tag 65200) Image@BufferNo",
        parsed.len()
    ));
    plate.vendor = json!({
        "flex_version": first.version,
        "opera_device": first.device,
        "files": parsed.iter().map(|f| json!({"file": f.rel, "row": f.row, "column": f.col, "images": f.images.len()})).collect::<Vec<_>>(),
    });
    Ok(plate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<Root><Arrays><Array Name="Exp1Cam1" Width="4" Height="2"/><Array Name="Exp2Cam1" Width="4" Height="2"/></Arrays>
<FLEX version="1.8.1.0" OperaDevice="OPERA5013"><LightSources><LightSource ID="L1"><Wavelength>488</Wavelength></LightSource></LightSources>
<LightSourceCombinations><LightSourceCombination ID="C1"><LightSourceRef ID="L1"/></LightSourceCombination></LightSourceCombinations>
<Plate><PlateName>96</PlateName><XSize>8</XSize><YSize>12</YSize><Barcode>B1</Barcode></Plate>
<Well><WellCoordinate Row="2" Col="3"/><Images>
<Image BufferNo="0"><ExposureNo>1</ExposureNo><Stack>1</Stack><Sublayout>1</Sublayout><LightSourceCombinationRef>C1</LightSourceCombinationRef><CameraExposureTime>0.1</CameraExposureTime><ImageResolutionX>1.0e-7</ImageResolutionX></Image>
<Image BufferNo="1"><ExposureNo>2</ExposureNo><Stack>1</Stack><Sublayout>1</Sublayout></Image>
</Images></Well></FLEX></Root>"#;

    #[test]
    fn parses_a_well() {
        let f = parse_file(XML, "x.flex".into(), Path::new("x.flex")).unwrap();
        assert_eq!((f.row, f.col), (2, 3));
        assert_eq!(f.images.len(), 2);
        assert_eq!(f.images[1].page, 1);
        assert_eq!(f.images[1].array_name.as_deref(), Some("Exp2Cam1"));
        assert_eq!(f.lights.get("L1"), Some(&488.0));
        assert_eq!(f.rows, Some(8));
        assert!(parse_file("<Root/>", "x".into(), Path::new("x")).is_err());
    }
}

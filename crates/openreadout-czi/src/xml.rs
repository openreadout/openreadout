//! The `ImageDocument` metadata XML and per-subblock `<METADATA>`: the subset we normalize.
//! See `docs/formats/czi.md`.

use std::collections::BTreeMap;

use openreadout_core::xml::{child, children, path};
use roxmltree::{Document, Node};

/// One `Information/Image/Dimensions/Channels/Channel`.
#[derive(Debug, Clone, Default)]
pub struct ChannelXml {
    pub id: Option<String>,
    pub channel_name: Option<String>,
    pub excitation_nm: Option<f64>,
    pub emission_nm: Option<f64>,
    /// `#AARRGGBB` as written.
    pub color_argb: Option<String>,
    pub fluor: Option<String>,
    /// Exposure in nanoseconds as written by ZEN.
    pub exposure_ns: Option<f64>,
    pub acquisition_mode: Option<String>,
    pub illumination_type: Option<String>,
    pub contrast_method: Option<String>,
    /// `DetectionWavelength/Ranges` as `[start_nm, end_nm]` (first range when several are listed).
    pub detection_range_nm: Option<[f64; 2]>,
    /// `DisplaySetting/Channels/Channel/DyeName` of the display channel with the same `Id`.
    pub dye_name: Option<String>,
    /// `DetectorSettings/Detector/@Id`: the `Information/Instrument/Detectors` entry used.
    pub detector_id: Option<String>,
}

/// One `Information/Image/Dimensions/S/Scenes/Scene`.
#[derive(Debug, Clone, Default)]
pub struct SceneXml {
    pub scene_name: Option<String>,
    /// `CenterPosition` ("x,y"), stage coordinates in micrometres.
    pub center_x_um: Option<f64>,
    pub center_y_um: Option<f64>,
    /// `ContourSize` ("w,h") in micrometres.
    pub contour_width_um: Option<f64>,
    pub contour_height_um: Option<f64>,
    /// `Shape/@Name` (the well name on multiwell plates, e.g. `B2`).
    pub well_name: Option<String>,
    /// `Shape/@Id` (e.g. `2-2`).
    pub well_id: Option<String>,
    /// `Shape/RowIndex`, `Shape/ColumnIndex` as written.
    pub row_index: Option<u32>,
    pub column_index: Option<u32>,
}

/// Summary of `Metadata/Experiment` (the acquisition protocol).
#[derive(Debug, Clone, Default)]
pub struct ExperimentXml {
    /// `Experiment/@Version`.
    pub experiment_version: Option<String>,
    /// Number of `ExperimentBlocks/AcquisitionBlock` elements.
    pub block_count: u32,
    /// Element names of activated `*Setup` elements under the first acquisition block's
    /// `SubDimensionSetups` (nested setups included), e.g. `TimeSeriesSetup`, `ZStackSetup`.
    pub active_setups: Vec<String>,
    /// `TimeSeriesSetup/Duration/Cycles`.
    pub time_series_cycles: Option<u32>,
    /// `TimeSeriesSetup/Interval/TimeSpan/Value` converted to seconds using `DefaultUnitFormat`.
    pub time_series_interval_s: Option<f64>,
}

/// Tags from one subblock's `<METADATA><Tags>` block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubBlockTags {
    /// `AcquisitionTime`, ISO-8601 as written.
    pub acquisition_time: Option<String>,
    /// `StageXPosition`, `StageYPosition`, `FocusPosition` in micrometres.
    pub stage_x_um: Option<f64>,
    pub stage_y_um: Option<f64>,
    pub focus_um: Option<f64>,
    /// `ExposureTime` inside the escaped `DetectorState` fragment, nanoseconds.
    pub exposure_ns: Option<f64>,
}

/// `Information/Instrument/Objectives/Objective`.
#[derive(Debug, Clone, Default)]
pub struct ObjectiveXml {
    pub objective_name: Option<String>,
    pub lens_na: Option<f64>,
    pub nominal_magnification: Option<f64>,
    pub immersion: Option<String>,
}

/// `Scaling/Items/Distance` values in micrometres.
#[derive(Debug, Clone, Default)]
pub struct ScalingXml {
    pub distance_x: Option<f64>,
    pub distance_y: Option<f64>,
    pub distance_z: Option<f64>,
}

/// Everything normalized from the XML.
#[derive(Debug, Clone, Default)]
pub struct ImageXml {
    pub size: BTreeMap<char, u32>,
    pub pixel_type: Option<String>,
    pub component_bit_count: Option<u32>,
    pub acquisition_time: Option<String>,
    pub channels: Vec<ChannelXml>,
    /// scene index → name
    pub scene_names: BTreeMap<u32, String>,
    /// scene index → everything read about that scene
    pub scenes: BTreeMap<u32, SceneXml>,
    pub objective: ObjectiveXml,
    pub microscope_name: Option<String>,
    /// `Information/Instrument/Detectors/Detector`: id → label (`Manufacturer/Model`, else
    /// `@Name`, else `Type`).
    pub detectors: BTreeMap<String, String>,
    /// `Information/Document/UserName`, else `Information/User/DisplayName`.
    pub user_name: Option<String>,
    pub application_name: Option<String>,
    pub application_version: Option<String>,
    pub scaling: ScalingXml,
    /// Time between frames, if `Dimensions/T/Positions/Interval/Increment` is present (seconds).
    pub t_increment_s: Option<f64>,
    pub experiment: Option<ExperimentXml>,
}

fn text(n: Option<Node<'_, '_>>) -> Option<String> {
    n.and_then(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
fn num(n: Option<Node<'_, '_>>) -> Option<f64> {
    text(n).and_then(|s| s.parse().ok())
}

/// A wavelength in nm; ZEN writes 0 for "not set".
fn wavelength(n: Option<Node<'_, '_>>) -> Option<f64> {
    num(n).filter(|v| v.is_finite() && *v > 0.0)
}

/// Parse what we need; tolerant of missing pieces (returns defaults).
pub fn parse_image_xml(xml: &str) -> Option<ImageXml> {
    let doc = Document::parse(xml).ok()?;
    let root = doc.root_element();
    let meta = child(root, "Metadata")?;
    let mut out = ImageXml::default();
    if let Some(img) = path(meta, &["Information", "Image"]) {
        for d in ['X', 'Y', 'Z', 'C', 'T', 'S', 'M', 'H', 'B', 'R', 'I', 'V'] {
            if let Some(v) = num(child(img, &format!("Size{d}"))) {
                out.size.insert(d, v as u32);
            }
        }
        out.pixel_type = text(child(img, "PixelType"));
        out.component_bit_count = num(child(img, "ComponentBitCount")).map(|v| v as u32);
        out.acquisition_time = text(child(img, "AcquisitionDateAndTime"));
        if let Some(chs) = path(img, &["Dimensions", "Channels"]) {
            for c in children(chs, "Channel") {
                out.channels.push(ChannelXml {
                    id: c.attribute("Id").map(str::to_string),
                    channel_name: c.attribute("Name").map(str::to_string),
                    excitation_nm: wavelength(child(c, "ExcitationWavelength")),
                    emission_nm: wavelength(child(c, "EmissionWavelength")),
                    color_argb: text(child(c, "Color")),
                    fluor: text(child(c, "Fluor")),
                    exposure_ns: num(child(c, "ExposureTime")),
                    acquisition_mode: text(child(c, "AcquisitionMode")),
                    illumination_type: text(child(c, "IlluminationType")),
                    contrast_method: text(child(c, "ContrastMethod")),
                    detection_range_nm: text(path(c, &["DetectionWavelength", "Ranges"]))
                        .and_then(|r| parse_range(&r)),
                    dye_name: None,
                    detector_id: path(c, &["DetectorSettings", "Detector"])
                        .and_then(|d| d.attribute("Id"))
                        .map(str::to_string),
                });
            }
        }
        if let Some(scenes) = path(img, &["Dimensions", "S", "Scenes"]) {
            for s in children(scenes, "Scene") {
                let Some(i) = s.attribute("Index").and_then(|v| v.parse::<u32>().ok()) else {
                    continue;
                };
                if let Some(n) = s.attribute("Name") {
                    out.scene_names.insert(i, n.to_string());
                }
                out.scenes.insert(i, parse_scene(s));
            }
        }
        out.t_increment_s = num(path(
            img,
            &["Dimensions", "T", "Positions", "Interval", "Increment"],
        ));
    }
    if let Some(obj) = path(
        meta,
        &["Information", "Instrument", "Objectives", "Objective"],
    ) {
        out.objective = ObjectiveXml {
            objective_name: obj
                .attribute("Name")
                .map(str::to_string)
                .or_else(|| text(path(obj, &["Manufacturer", "Model"]))),
            lens_na: num(child(obj, "LensNA")),
            nominal_magnification: num(child(obj, "NominalMagnification")),
            immersion: text(child(obj, "Immersion")),
        };
    }
    out.microscope_name = path(
        meta,
        &["Information", "Instrument", "Microscopes", "Microscope"],
    )
    .and_then(|m| {
        m.attribute("Name")
            .map(str::to_string)
            .filter(|n| !n.trim().is_empty())
            .or_else(|| text(child(m, "System")))
    });
    if let Some(dets) = path(meta, &["Information", "Instrument", "Detectors"]) {
        for d in children(dets, "Detector") {
            let label = text(path(d, &["Manufacturer", "Model"]))
                .or_else(|| {
                    d.attribute("Name")
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string)
                })
                .or_else(|| text(child(d, "Type")));
            if let (Some(id), Some(label)) = (d.attribute("Id"), label) {
                out.detectors.insert(id.to_string(), label);
            }
        }
    }
    out.user_name = text(path(meta, &["Information", "Document", "UserName"]))
        .or_else(|| text(path(meta, &["Information", "User", "DisplayName"])));
    out.application_name = text(path(meta, &["Information", "Application", "Name"]));
    out.application_version = text(path(meta, &["Information", "Application", "Version"]));
    if let Some(items) = path(meta, &["Scaling", "Items"]) {
        for d in children(items, "Distance") {
            let v = num(child(d, "Value")).map(|m| m * 1e6);
            match d.attribute("Id") {
                Some("X") => out.scaling.distance_x = v,
                Some("Y") => out.scaling.distance_y = v,
                Some("Z") => out.scaling.distance_z = v,
                _ => {}
            }
        }
    }
    if let Some(ds) = path(meta, &["DisplaySetting", "Channels"]) {
        // Some writers (ZEN black test builds, older ZEN 2011, RGB snaps) list channels only
        // under DisplaySetting: take them from there, in `StartC` order.
        if out.channels.is_empty() {
            let mut display: Vec<(Option<u32>, usize, Option<String>)> = children(ds, "Channel")
                .enumerate()
                .map(|(i, d)| {
                    (
                        d.attribute("StartC").and_then(|v| v.trim().parse().ok()),
                        i,
                        d.attribute("Id").map(str::to_string),
                    )
                })
                .collect();
            // Reordering needs ids, since display entries are matched to channels by id.
            if display.iter().all(|d| d.0.is_some() && d.2.is_some()) {
                display.sort_by_key(|d| (d.0, d.1));
            }
            out.channels = display
                .into_iter()
                .map(|(_, _, id)| ChannelXml {
                    id,
                    ..ChannelXml::default()
                })
                .collect();
        }
        // A document can list fewer image channels than it stores (a PALM rendering's
        // widefield channel appears only under DisplaySetting): display channels whose `Id`
        // matches none of the image channels follow them, in document order.
        let ids: Vec<Option<String>> = out.channels.iter().map(|c| c.id.clone()).collect();
        if !ids.is_empty() && ids.iter().all(Option::is_some) {
            let extra: Vec<String> = children(ds, "Channel")
                .filter_map(|d| d.attribute("Id"))
                .filter(|id| !ids.iter().any(|x| x.as_deref() == Some(*id)))
                .map(str::to_string)
                .collect();
            let matched = children(ds, "Channel")
                .filter_map(|d| d.attribute("Id"))
                .any(|id| ids.iter().any(|x| x.as_deref() == Some(id)));
            if matched {
                out.channels.extend(extra.into_iter().map(|id| ChannelXml {
                    id: Some(id),
                    ..ChannelXml::default()
                }));
            }
        }
        apply_display_setting(ds, &mut out.channels);
    }
    out.experiment = child(meta, "Experiment").map(parse_experiment);
    Some(out)
}

fn parse_scene(s: Node<'_, '_>) -> SceneXml {
    let center = text(child(s, "CenterPosition")).and_then(|v| parse_pair(&v));
    let contour = text(child(s, "ContourSize")).and_then(|v| parse_pair(&v));
    let shape = child(s, "Shape");
    SceneXml {
        scene_name: s.attribute("Name").map(str::to_string),
        center_x_um: center.map(|p| p[0]),
        center_y_um: center.map(|p| p[1]),
        contour_width_um: contour.map(|p| p[0]),
        contour_height_um: contour.map(|p| p[1]),
        well_name: shape.and_then(|n| n.attribute("Name")).map(str::to_string),
        well_id: shape.and_then(|n| n.attribute("Id")).map(str::to_string),
        row_index: shape
            .and_then(|n| num(child(n, "RowIndex")))
            .map(|v| v as u32),
        column_index: shape
            .and_then(|n| num(child(n, "ColumnIndex")))
            .map(|v| v as u32),
    }
}

/// `DisplaySetting/Channels/Channel`: matched to image channels by `Id` when any id matches,
/// by position otherwise. Fills colour, name, dye and wavelengths the image channel lacks.
fn apply_display_setting(ds: Node<'_, '_>, channels: &mut [ChannelXml]) {
    let display: Vec<Node<'_, '_>> = children(ds, "Channel").collect();
    let by_id = display.iter().any(|d| {
        d.attribute("Id")
            .is_some_and(|id| channels.iter().any(|c| c.id.as_deref() == Some(id)))
    });
    for (i, d) in display.iter().enumerate() {
        let target = if by_id {
            d.attribute("Id")
                .and_then(|id| channels.iter().position(|c| c.id.as_deref() == Some(id)))
        } else {
            Some(i)
        };
        let Some(ch) = target.and_then(|t| channels.get_mut(t)) else {
            continue;
        };
        if ch.color_argb.is_none() {
            ch.color_argb = text(child(*d, "Color"));
        }
        if ch.channel_name.is_none() {
            ch.channel_name = d.attribute("Name").map(str::to_string);
        }
        if ch.dye_name.is_none() {
            ch.dye_name = text(child(*d, "DyeName"));
        }
        if ch.emission_nm.is_none() {
            ch.emission_nm = wavelength(child(*d, "DyeMaxEmission"));
        }
        if ch.excitation_nm.is_none() {
            ch.excitation_nm = wavelength(child(*d, "DyeMaxExcitation"));
        }
        if ch.illumination_type.is_none() {
            ch.illumination_type = text(child(*d, "IlluminationType"));
        }
    }
}

fn parse_experiment(e: Node<'_, '_>) -> ExperimentXml {
    let blocks: Vec<Node<'_, '_>> = child(e, "ExperimentBlocks")
        .map(|b| children(b, "AcquisitionBlock").collect())
        .unwrap_or_default();
    let mut out = ExperimentXml {
        experiment_version: e.attribute("Version").map(str::to_string),
        block_count: blocks.len() as u32,
        ..ExperimentXml::default()
    };
    if let Some(setups) = blocks.first().and_then(|b| child(*b, "SubDimensionSetups")) {
        collect_active_setups(setups, &mut out, 0);
    }
    out
}

/// Walk nested `*Setup` elements (setups can contain other setups) and record activated ones.
fn collect_active_setups(n: Node<'_, '_>, out: &mut ExperimentXml, depth: u32) {
    if depth > 16 {
        return;
    }
    for c in n.children().filter(Node::is_element) {
        let name = c.tag_name().name();
        if name.ends_with("Setup") && c.attribute("IsActivated") == Some("true") {
            out.active_setups.push(name.to_string());
            if name == "TimeSeriesSetup" {
                out.time_series_cycles = num(path(c, &["Duration", "Cycles"])).map(|v| v as u32);
                if let Some(ts) = path(c, &["Interval", "TimeSpan"]) {
                    let unit = text(child(ts, "DefaultUnitFormat"));
                    out.time_series_interval_s = num(child(ts, "Value")).and_then(|v| {
                        unit_to_seconds(unit.as_deref().unwrap_or("s")).map(|f| v * f)
                    });
                }
            }
        }
        if let Some(sub) = child(c, "SubDimensionSetups") {
            collect_active_setups(sub, out, depth + 1);
        }
    }
}

fn unit_to_seconds(unit: &str) -> Option<f64> {
    Some(match unit {
        "s" => 1.0,
        "ms" => 1e-3,
        "µs" | "us" => 1e-6,
        "ns" => 1e-9,
        "min" => 60.0,
        "h" => 3600.0,
        _ => return None,
    })
}

/// `"a,b"` → `[a, b]`.
fn parse_pair(s: &str) -> Option<[f64; 2]> {
    let mut it = s.split(',').map(|v| v.trim().parse::<f64>());
    match (it.next(), it.next(), it.next()) {
        (Some(Ok(a)), Some(Ok(b)), None) => Some([a, b]),
        _ => None,
    }
}

/// `"415.0-735"` (optionally several comma-separated ranges) → first range.
fn parse_range(s: &str) -> Option<[f64; 2]> {
    let first = s.split(',').next()?.trim();
    let (a, b) = first.split_once('-')?;
    Some([a.trim().parse().ok()?, b.trim().parse().ok()?])
}

/// Parse a subblock's `<METADATA>` XML. Tolerant: missing or malformed tags are `None`.
pub fn parse_subblock_tags(xml: &str) -> SubBlockTags {
    let Ok(doc) = Document::parse(xml) else {
        return SubBlockTags::default();
    };
    let Some(tags) = child(doc.root_element(), "Tags") else {
        return SubBlockTags::default();
    };
    // DetectorState holds an escaped XML fragment; roxmltree has already unescaped it.
    let exposure_ns = text(child(tags, "DetectorState")).and_then(|inner| {
        Document::parse(&inner).ok().and_then(|d| {
            d.descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "ExposureTime")
                .and_then(|n| n.text())
                .and_then(|t| t.trim().parse().ok())
        })
    });
    SubBlockTags {
        acquisition_time: text(child(tags, "AcquisitionTime")),
        stage_x_um: num(child(tags, "StageXPosition")),
        stage_y_um: num(child(tags, "StageYPosition")),
        focus_um: num(child(tags, "FocusPosition")),
        exposure_ns,
    }
}

/// `#AARRGGBB` or `#RRGGBB` → `#RRGGBB`.
pub fn argb_to_rgb(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    match h.len() {
        8 => Some(format!("#{}", h[2..].to_uppercase())),
        6 => Some(format!("#{}", h.to_uppercase())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subblock_tags() {
        let x = "<METADATA><Tags><AcquisitionTime>2022-08-22T06:38:37.6951779Z</AcquisitionTime><DetectorState>&lt;CameraState&gt;&lt;ExposureTime&gt;10004210.5&lt;/ExposureTime&gt;&lt;/CameraState&gt;</DetectorState><StageXPosition>+000000049500.0000</StageXPosition><StageYPosition>-000000035500.5000</StageYPosition><FocusPosition>+000000000111.8420</FocusPosition></Tags></METADATA>";
        let t = parse_subblock_tags(x);
        assert_eq!(
            t.acquisition_time.as_deref(),
            Some("2022-08-22T06:38:37.6951779Z")
        );
        assert_eq!(t.stage_x_um, Some(49500.0));
        assert_eq!(t.stage_y_um, Some(-35500.5));
        assert_eq!(t.focus_um, Some(111.842));
        assert_eq!(t.exposure_ns, Some(10_004_210.5));
        assert_eq!(parse_subblock_tags("<not xml"), SubBlockTags::default());
    }

    #[test]
    fn scenes_wells_display_and_experiment() {
        let doc = r#"<ImageDocument><Metadata><Information><Image>
          <Dimensions><Channels><Channel Id="Channel:0" Name="EGFP"><ExposureTime>150000000</ExposureTime>
            <DetectionWavelength><Ranges>415-735</Ranges></DetectionWavelength></Channel></Channels>
          <S><Scenes><Scene Index="0" Name="B2"><CenterPosition>13500,17500</CenterPosition><ContourSize>1024,1344</ContourSize>
            <Shape Name="B2" Id="2-2"><ColumnIndex>2</ColumnIndex><RowIndex>1</RowIndex></Shape></Scene></Scenes></S>
          </Dimensions></Image></Information>
          <Experiment Version="1.2"><ExperimentBlocks><AcquisitionBlock><SubDimensionSetups>
            <TimeSeriesSetup IsActivated="true"><Duration><Cycles>3</Cycles></Duration>
              <Interval><TimeSpan><Value>500</Value><DefaultUnitFormat>ms</DefaultUnitFormat></TimeSpan></Interval>
              <SubDimensionSetups><ZStackSetup IsActivated="false"/><TilesSetup IsActivated="true"/></SubDimensionSetups>
            </TimeSeriesSetup></SubDimensionSetups></AcquisitionBlock></ExperimentBlocks></Experiment>
          <DisplaySetting><Channels><Channel Id="Channel:0" Name="EGFP"><Color>#FF00FF5B</Color><DyeName>EGFP dye</DyeName>
            <DyeMaxEmission>509</DyeMaxEmission><DyeMaxExcitation>488</DyeMaxExcitation></Channel></Channels></DisplaySetting>
          </Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        let scene = &meta.scenes[&0];
        assert_eq!(scene.well_name.as_deref(), Some("B2"));
        assert_eq!(scene.well_id.as_deref(), Some("2-2"));
        assert_eq!((scene.row_index, scene.column_index), (Some(1), Some(2)));
        assert_eq!(
            (scene.center_x_um, scene.center_y_um),
            (Some(13500.0), Some(17500.0))
        );
        assert_eq!(scene.contour_height_um, Some(1344.0));
        let ch = &meta.channels[0];
        assert_eq!(ch.color_argb.as_deref(), Some("#FF00FF5B"));
        assert_eq!(ch.dye_name.as_deref(), Some("EGFP dye"));
        assert_eq!(
            (ch.excitation_nm, ch.emission_nm),
            (Some(488.0), Some(509.0))
        );
        assert_eq!(ch.detection_range_nm, Some([415.0, 735.0]));
        let exp = meta.experiment.unwrap();
        assert_eq!(exp.experiment_version.as_deref(), Some("1.2"));
        assert_eq!(exp.block_count, 1);
        assert_eq!(exp.active_setups, ["TimeSeriesSetup", "TilesSetup"]);
        assert_eq!(exp.time_series_cycles, Some(3));
        assert_eq!(exp.time_series_interval_s, Some(0.5));
    }

    #[test]
    fn channels_from_display_setting_alone() {
        let doc = r#"<ImageDocument><Metadata><Information><Image><SizeC>2</SizeC></Image></Information>
          <DisplaySetting><Channels>
            <Channel Id="B" StartC="1" Name="SWF 1"><Color>#FF00FF00</Color></Channel>
            <Channel Id="A" StartC="0" Name="HR 1"/>
          </Channels></DisplaySetting></Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        let names: Vec<_> = meta
            .channels
            .iter()
            .map(|c| c.channel_name.as_deref())
            .collect();
        assert_eq!(names, [Some("HR 1"), Some("SWF 1")]);
        assert_eq!(meta.channels[1].color_argb.as_deref(), Some("#FF00FF00"));
    }

    #[test]
    fn display_channels_missing_from_the_image_list_follow_it() {
        let doc = r#"<ImageDocument><Metadata><Information><Image><Dimensions><Channels>
            <Channel Id="A" Name="HR 1"/></Channels></Dimensions></Image></Information>
          <DisplaySetting><Channels><Channel Id="A" Name="HR 1"/><Channel Id="B" Name="SWF 1"/>
          </Channels></DisplaySetting></Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        let names: Vec<_> = meta
            .channels
            .iter()
            .map(|c| c.channel_name.as_deref())
            .collect();
        assert_eq!(names, [Some("HR 1"), Some("SWF 1")]);
    }

    #[test]
    fn detectors_by_channel() {
        let doc = r#"<ImageDocument><Metadata><Information><Image><Dimensions><Channels>
            <Channel Id="Channel:0"><DetectorSettings><Detector Id="Detector:705c"/></DetectorSettings></Channel>
            <Channel Id="Channel:1"><DetectorSettings><Detector Id="Detector:0:0"/></DetectorSettings></Channel>
          </Channels></Dimensions></Image>
          <Instrument><Detectors>
            <Detector Id="Detector:705c" Name="705c"><Manufacturer><Model>Axiocam705c</Model></Manufacturer></Detector>
            <Detector Id="Detector:0:0"><Type>PMT</Type></Detector>
          </Detectors></Instrument></Information></Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        assert_eq!(
            meta.channels[0].detector_id.as_deref(),
            Some("Detector:705c")
        );
        assert_eq!(meta.detectors["Detector:705c"], "Axiocam705c");
        assert_eq!(meta.detectors["Detector:0:0"], "PMT");
    }

    #[test]
    fn user_name_and_microscope_system() {
        let doc = r#"<ImageDocument><Metadata><Information>
            <User Id="0"><DisplayName>m1psc</DisplayName></User>
            <Document><UserName>m1psc-login</UserName></Document>
            <Instrument><Microscopes><Microscope Id="Microscope:1"><System>LSM 710, AxioObserver</System></Microscope></Microscopes></Instrument>
            </Information></Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        assert_eq!(meta.user_name.as_deref(), Some("m1psc-login"));
        assert_eq!(
            meta.microscope_name.as_deref(),
            Some("LSM 710, AxioObserver")
        );
        let doc = r#"<ImageDocument><Metadata><Information>
            <User Id="0"><DisplayName>LSM User</DisplayName></User>
            <Instrument><Microscopes><Microscope Name="Axio Observer.Z1 / 7"><System>ignored</System></Microscope></Microscopes></Instrument>
            </Information></Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        assert_eq!(meta.user_name.as_deref(), Some("LSM User"));
        assert_eq!(
            meta.microscope_name.as_deref(),
            Some("Axio Observer.Z1 / 7")
        );
    }

    #[test]
    fn display_setting_by_position_when_ids_differ() {
        let doc = r#"<ImageDocument><Metadata><Information><Image><Dimensions><Channels>
            <Channel Id="A" Name="Ch1"/></Channels></Dimensions></Image></Information>
            <DisplaySetting><Channels><Channel Id="Z"><Color>#FFFFFF</Color></Channel></Channels></DisplaySetting>
            </Metadata></ImageDocument>"#;
        let meta = parse_image_xml(doc).unwrap();
        assert_eq!(meta.channels[0].color_argb.as_deref(), Some("#FFFFFF"));
        assert_eq!(argb_to_rgb("#00ffffff").as_deref(), Some("#FFFFFF"));
        assert_eq!(argb_to_rgb("#é00000"), None);
    }
}

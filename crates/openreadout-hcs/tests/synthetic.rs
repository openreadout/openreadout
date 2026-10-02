//! Synthetic plates written to a temporary folder: every reader against planes whose pixels are
//! known, partial copies (missing files), planes the index lists without a file, channels
//! acquired at fewer Z planes, per-well statistics, and damaged indexes (clean errors, no panic).

use std::path::Path;

use openreadout_core::parallel::ReadContext;
use openreadout_core::plate::{WellStatsRequest, plate_layout, well_stats};
use openreadout_core::reader::PlaneIndex;
use openreadout_core::stats::{StatsRequest, compute_stats};
use openreadout_core::{FormatReader, Registry};
use openreadout_hcs::{CellVoyagerReader, HarmonyReader, ImageXpressReader};

/// A minimal little-endian uncompressed uint16 TIFF: `w` x `h` samples all equal to `value`.
fn tiff(w: u16, h: u16, value: u16) -> Vec<u8> {
    tiff_pages(w, h, &[value])
}

/// A multi-page uncompressed uint16 TIFF: page `i` holds `w` x `h` samples equal to `values[i]`.
fn tiff_pages(w: u16, h: u16, values: &[u16]) -> Vec<u8> {
    let n = u32::from(w) * u32::from(h);
    let mut b = Vec::new();
    b.extend_from_slice(b"II*\0");
    b.extend_from_slice(&0u32.to_le_bytes());
    let mut link = 4usize; // where the offset of the next IFD goes
    for &value in values {
        let data_off = u32::try_from(b.len()).unwrap();
        for _ in 0..n {
            b.extend_from_slice(&value.to_le_bytes());
        }
        let ifd_off = u32::try_from(b.len()).unwrap();
        b[link..link + 4].copy_from_slice(&ifd_off.to_le_bytes());
        let entries: [(u16, u16, u32, u32); 8] = [
            (256, 3, 1, u32::from(w)),
            (257, 3, 1, u32::from(h)),
            (258, 3, 1, 16),
            (259, 3, 1, 1),
            (262, 3, 1, 1),
            (273, 4, 1, data_off),
            (278, 3, 1, u32::from(h)),
            (279, 4, 1, n * 2),
        ];
        b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, typ, count, val) in entries {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            if typ == 3 {
                b.extend_from_slice(&(val as u16).to_le_bytes());
                b.extend_from_slice(&[0, 0]);
            } else {
                b.extend_from_slice(&val.to_le_bytes());
            }
        }
        link = b.len();
        b.extend_from_slice(&0u32.to_le_bytes());
    }
    b
}

/// A one-page uint16 TIFF like [`tiff`] with an ImageDescription (tag 270), e.g. the MetaSeries
/// XML MetaXpress writes into every plane.
fn tiff_described(w: u16, h: u16, value: u16, description: &str) -> Vec<u8> {
    let n = u32::from(w) * u32::from(h);
    let mut b = Vec::new();
    b.extend_from_slice(b"II*\0");
    b.extend_from_slice(&0u32.to_le_bytes());
    let data_off = u32::try_from(b.len()).unwrap();
    for _ in 0..n {
        b.extend_from_slice(&value.to_le_bytes());
    }
    let desc_off = u32::try_from(b.len()).unwrap();
    b.extend_from_slice(description.as_bytes());
    b.push(0);
    if b.len() % 2 == 1 {
        b.push(0);
    }
    let ifd_off = u32::try_from(b.len()).unwrap();
    b[4..8].copy_from_slice(&ifd_off.to_le_bytes());
    let desc_len = u32::try_from(description.len() + 1).unwrap();
    let entries: [(u16, u16, u32, u32); 9] = [
        (256, 3, 1, u32::from(w)),
        (257, 3, 1, u32::from(h)),
        (258, 3, 1, 16),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (270, 2, desc_len, desc_off),
        (273, 4, 1, data_off),
        (278, 3, 1, u32::from(h)),
        (279, 4, 1, n * 2),
    ];
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, typ, count, val) in entries {
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&typ.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        if typ == 3 {
            b.extend_from_slice(&(val as u16).to_le_bytes());
            b.extend_from_slice(&[0, 0]);
        } else {
            b.extend_from_slice(&val.to_le_bytes());
        }
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    b
}

/// MetaSeries XML as MetaXpress writes it into a plane's ImageDescription (trimmed).
const METASERIES: &str = "<MetaData>\r\n<prop id=\"Description\" type=\"string\" value=\"Plate Name: P7&#13;&#10;Exposure: 45 ms\"/>\r\n<prop id=\"image-name\" type=\"string\" value=\"DAPI\"/>\r\n</MetaData>";

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(HarmonyReader))
        .with(Box::new(ImageXpressReader))
        .with(Box::new(CellVoyagerReader))
}

fn harmony_image(row: u32, col: u32, field: u32, ch: u32, url: &str) -> String {
    format!(
        r#"<Image Version="1"><id>x</id><State>Ok</State><URL>{url}</URL><Row>{row}</Row><Col>{col}</Col>
<FieldID>{field}</FieldID><PlaneID>1</PlaneID><TimepointID>0</TimepointID><ChannelID>{ch}</ChannelID><FlimID>1</FlimID>
<ChannelName>{name}</ChannelName><ChannelType>Fluorescence</ChannelType>
<ImageResolutionX Unit="m">6.5E-07</ImageResolutionX><ImageResolutionY Unit="m">6.5E-07</ImageResolutionY>
<ImageSizeX>8</ImageSizeX><ImageSizeY>4</ImageSizeY><PositionX Unit="m">0.0001</PositionX><PositionY Unit="m">-0.0002</PositionY>
<PositionZ Unit="m">1E-06</PositionZ><AbsTime>2021-01-01T10:0{field}:0{ch}+00:00</AbsTime>
<MainExcitationWavelength Unit="nm">375</MainExcitationWavelength><MainEmissionWavelength Unit="nm">456</MainEmissionWavelength>
<ObjectiveMagnification Unit="">20</ObjectiveMagnification><ObjectiveNA Unit="">1</ObjectiveNA><ExposureTime Unit="s">0.02</ExposureTime></Image>"#,
        name = if ch == 1 { "DAPI" } else { "GFP" }
    )
}

/// Wells B03 (fields 1, 2) and D12 (field 1) x 2 channels. B03 field 2 channel 2 has no file on
/// disk; D12 field 1 channel 2 is listed without a file name.
fn harmony_plate(dir: &Path) -> std::path::PathBuf {
    let img = dir.join("M1").join("Images");
    std::fs::create_dir_all(&img).unwrap();
    let mut images = String::new();
    for (row, col, field, ch, url, value) in [
        (2, 3, 1, 1, "r02c03f01p01-ch1sk1fk1fl1.tiff", Some(100)),
        (2, 3, 1, 2, "r02c03f01p01-ch2sk1fk1fl1.tiff", Some(200)),
        (2, 3, 2, 1, "r02c03f02p01-ch1sk1fk1fl1.tiff", Some(300)),
        (2, 3, 2, 2, "r02c03f02p01-ch2sk1fk1fl1.tiff", None),
        (4, 12, 1, 1, "r04c12f01p01-ch1sk1fk1fl1.tiff", Some(500)),
        (4, 12, 1, 2, "", None),
    ] {
        images.push_str(&harmony_image(row, col, field, ch, url));
        if let Some(v) = value {
            std::fs::write(img.join(url), tiff(8, 4, v)).unwrap();
        }
    }
    let index = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<EvaluationInputData Version="1" xmlns="http://www.perkinelmer.com/PEHH/HarmonyV5">
<User>tester</User><InstrumentType>Phenix</InstrumentType>
<Plates><Plate><PlateID>BAR123</PlateID><MeasurementID>m</MeasurementID><MeasurementStartTime>2021-01-01T09:59:00+00:00</MeasurementStartTime>
<Name>BAR123</Name><PlateTypeName>96 test</PlateTypeName><PlateRows>8</PlateRows><PlateColumns>12</PlateColumns>
<Well id="0203" /><Well id="0412" /><Well id="0101" /></Plate></Plates>
<Images>{images}</Images></EvaluationInputData>"#
    );
    let p = img.join("Index.idx.xml");
    std::fs::write(&p, index).unwrap();
    // a plane-like file the index does not name
    std::fs::write(img.join("r08c08f01p01-ch1sk1fk1fl1.tiff"), tiff(8, 4, 1)).unwrap();
    p
}

#[test]
fn harmony_plate_with_missing_and_unrecorded_planes() {
    let dir = tempfile::tempdir().unwrap();
    let index = harmony_plate(dir.path());
    let reg = registry();
    for open in [
        index.clone(),
        dir.path().join("M1"),
        index.parent().unwrap().to_path_buf(),
    ] {
        let (det, mut ds) = reg.open(&open).unwrap();
        assert_eq!(det.format_id, "opera-harmony");
        let info = ds.info().unwrap();
        assert_eq!(info.images.len(), 3);
        let im = &info.images[0];
        assert_eq!(im.name.as_deref(), Some("B03 field 1"));
        assert_eq!((im.size_x, im.size_y, im.size_c), (8, 4, 2));
        assert_eq!(im.channels[1].name.as_deref(), Some("GFP"));
        assert_eq!(im.physical_size.x, Some(0.65));
        assert_eq!(im.extra["well"], "B03");
        assert_eq!(im.extra["position_x_um"], 100.0);
        let plate = ds.plate().unwrap();
        assert_eq!(plate.id.as_deref(), Some("BAR123"));
        assert_eq!(plate.wells.len(), 2);
        assert_eq!(plate.planes_missing, 1);
        assert_eq!(plate.planes_absent, 1);
        assert!(!plate.complete);
        assert_eq!(
            plate.extra["selected_wells_without_images"],
            serde_json::json!(["A01"])
        );
        assert_eq!(plate.extra["unindexed_files"], 1);
        // pixels
        let p = ds.read_plane(1, PlaneIndex { c: 0, z: 0, t: 0 }).unwrap();
        assert_eq!(&p.data[..2], &300u16.to_le_bytes());
        let e = ds
            .read_plane(1, PlaneIndex { c: 1, z: 0, t: 0 })
            .unwrap_err();
        assert_eq!(e.exit_code(), 5, "{e}");
        assert!(e.to_string().contains("r02c03f02p01-ch2"), "{e}");
        let blank = ds.read_plane(2, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
        assert!(blank.data.iter().all(|&b| b == 0));
        assert_eq!(
            info.images[2].extra["absent_planes"],
            serde_json::json!([[1, 0, 0]])
        );
        assert_eq!(
            info.images[1].extra["missing_planes"],
            serde_json::json!([[1, 0, 0]])
        );
        // check
        let r = ds.check().unwrap();
        assert!(!r.ok);
        let codes: Vec<&str> = r.findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"missing_plane_files"), "{codes:?}");
        assert!(codes.contains(&"planes_not_recorded"), "{codes:?}");
        assert!(codes.contains(&"unindexed_plane_files"), "{codes:?}");
        // experiment
        let e = ds.experiment().unwrap();
        assert_eq!(e.sample.unwrap().barcode.as_deref(), Some("BAR123"));
        assert_eq!(ds.frames(0, None).unwrap().0, 2);
    }
}

#[test]
fn well_stats_merge_fields_and_skip_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let index = harmony_plate(dir.path());
    let (_, mut ds) = registry().open(&index).unwrap();
    let info = ds.info().unwrap();
    let out = well_stats(
        ds.as_mut(),
        &info,
        &WellStatsRequest::default(),
        &ReadContext::default(),
    )
    .unwrap();
    let b03_c0 = out
        .rows
        .iter()
        .find(|r| r.well == "B03" && r.c == 0)
        .unwrap();
    assert_eq!(b03_c0.mean, Some(200.0)); // fields at 100 and 300
    assert_eq!(b03_c0.fields, 2);
    let b03_c1 = out
        .rows
        .iter()
        .find(|r| r.well == "B03" && r.c == 1)
        .unwrap();
    assert_eq!(
        (b03_c1.mean, b03_c1.planes, b03_c1.planes_missing),
        (Some(200.0), 1, 1)
    );
    let d12_c1 = out
        .rows
        .iter()
        .find(|r| r.well == "D12" && r.c == 1)
        .unwrap();
    assert_eq!((d12_c1.mean, d12_c1.planes), (None, 0)); // not recorded: left out
    assert_eq!(out.plate.as_deref(), Some("BAR123"));
    let mut req = WellStatsRequest::default();
    req.per_field = true;
    req.wells = vec!["b3".into()];
    let out = well_stats(ds.as_mut(), &info, &req, &ReadContext::default()).unwrap();
    assert_eq!(out.rows.len(), 4);
    assert_eq!(out.rows[2].field, Some(2));
    assert!(
        openreadout_core::plate::well_stats_csv(&out)
            .starts_with("plate,well,row,column,image,field")
    );
    let mut req = WellStatsRequest::default();
    req.wells = vec!["H1".into()];
    assert_eq!(
        well_stats(ds.as_mut(), &info, &req, &ReadContext::default())
            .unwrap_err()
            .exit_code(),
        2
    );
}

const MRF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<bts:MeasurementDetail bts:Version="1.0" bts:OperatorName="op" bts:Title="T" bts:BeginTime="2021-06-14T09:00:00+01:00" bts:EndTime="2021-06-14T10:00:00+01:00" bts:MeasurementSettingFileName="S.mes" bts:ColumnCount="24" bts:RowCount="16" bts:TargetSystem="CV8000" bts:ReleaseNumber="R2" xmlns:bts="http://www.yokogawa.co.jp/BTS/BTSSchema/1.0">
  <bts:MeasurementSamplePlate bts:Name="PL1" bts:WellPlateFileName="PL1.wpi" bts:WellPlateProductFileName="x.wpp" />
  <bts:MeasurementChannel bts:Ch="1" bts:HorizontalPixelDimension="0.65" bts:VerticalPixelDimension="0.65" bts:InputBitDepth="16" bts:HorizontalPixels="8" bts:VerticalPixels="4" />
  <bts:MeasurementChannel bts:Ch="2" bts:HorizontalPixelDimension="0.65" bts:VerticalPixelDimension="0.65" bts:InputBitDepth="16" bts:HorizontalPixels="8" bts:VerticalPixels="4" />
</bts:MeasurementDetail>"#;

#[test]
fn cellvoyager_channel_at_fewer_z_planes_is_left_out_of_statistics() {
    let dir = tempfile::tempdir().unwrap();
    let m = dir.path().join("meas");
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(m.join("MeasurementDetail.mrf"), MRF).unwrap();
    let mut recs = String::new();
    for (ch, z, value) in [(1, 1, 10u16), (2, 1, 20), (2, 2, 30), (2, 3, 40)] {
        let name = format!("PL1_A01_T0001F001L01A01Z0{z}C0{ch}.tif");
        std::fs::write(m.join(&name), tiff(8, 4, value)).unwrap();
        recs.push_str(&format!(
            r#"<bts:MeasurementRecord bts:Type="IMG" bts:Time="2021-06-14T09:00:0{z}+01:00" bts:Column="1" bts:Row="1" bts:TimePoint="1" bts:FieldIndex="1" bts:ZIndex="{z}" bts:X="1" bts:Y="2" bts:Z="{zp}" bts:Ch="{ch}">{name}</bts:MeasurementRecord>"#,
            zp = f64::from(z) * 5.0
        ));
    }
    std::fs::write(
        m.join("MeasurementData.mlf"),
        format!(r#"<?xml version="1.0" encoding="utf-8"?><bts:MeasurementData xmlns:bts="http://www.yokogawa.co.jp/BTS/BTSSchema/1.0">{recs}</bts:MeasurementData>"#),
    )
    .unwrap();
    for open in [
        m.join("MeasurementData.mlf"),
        m.clone(),
        m.join("MeasurementDetail.mrf"),
    ] {
        let (det, mut ds) = registry().open(&open).unwrap();
        assert_eq!(det.format_id, "cellvoyager");
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!((im.size_c, im.size_z), (2, 3));
        assert_eq!(im.physical_size.z, Some(5.0));
        assert_eq!(
            im.extra["absent_planes"],
            serde_json::json!([[0, 1, 0], [0, 2, 0]])
        );
        let mut req = StatsRequest::default();
        req.select = vec!["c=0".into()];
        let s = compute_stats(ds.as_mut(), &info, &req, &ReadContext::default()).unwrap();
        assert_eq!(s.channels[0].planes, 1);
        assert_eq!(s.channels[0].stats.mean, Some(10.0)); // not diluted by blank planes
        let plate = plate_layout(ds.as_ref(), &info).unwrap();
        assert!(plate.complete);
        assert_eq!(plate.planes_absent, 2);
        assert!(ds.check().unwrap().ok);
    }
}

#[test]
fn imagexpress_htd_and_folder_without_htd() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("plate");
    std::fs::create_dir_all(&p).unwrap();
    let htd = "\"HTSInfoFile\", Version 1.0\r\n\"Description\", \"test plate\"\r\n\"TimePoints\", 1\r\n\"XWells\", 12\r\n\"YWells\", 8\r\n\"WellsSelection1\", TRUE, TRUE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE\r\n\"Sites\", TRUE\r\n\"XSites\", 2\r\n\"YSites\", 1\r\n\"SiteSelection1\", TRUE, TRUE\r\n\"Waves\", TRUE\r\n\"NWavelengths\", 2\r\n\"WaveName1\", \"DAPI\"\r\n\"WaveName2\", \"FITC\"\r\n\"EndFile\"\r\n";
    std::fs::write(p.join("P7.HTD"), htd).unwrap();
    for (well, site, wave, v) in [
        ("A01", 1, 1, 11u16),
        ("A01", 1, 2, 12),
        ("A01", 2, 1, 13),
        ("A01", 2, 2, 14),
        ("A02", 1, 1, 21),
    ] {
        std::fs::write(
            p.join(format!(
                "P7_{well}_s{site}_w{wave}4996E914-A4B6-49AF-8081-5B58A8653201.tif"
            )),
            tiff_described(8, 4, v, METASERIES),
        )
        .unwrap();
    }
    let (det, mut ds) = registry().open(&p.join("P7.HTD")).unwrap();
    assert_eq!(det.format_id, "imagexpress");
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 4);
    assert_eq!(info.images[0].channels[1].name.as_deref(), Some("FITC"));
    let plate = ds.plate().unwrap();
    assert_eq!(plate.planes_missing, 3); // A02 s1 w2, s2 w1, s2 w2
    assert_eq!(plate.id.as_deref(), Some("P7"));
    let px = ds.read_plane(1, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    assert_eq!(&px.data[..2], &14u16.to_le_bytes());
    assert_eq!(
        ds.read_plane(3, PlaneIndex { c: 0, z: 0, t: 0 })
            .unwrap_err()
            .exit_code(),
        5
    );
    // the folder without its HTD: layout from the file names only
    std::fs::remove_file(p.join("P7.HTD")).unwrap();
    let (det, ds) = registry().open(&p).unwrap();
    assert_eq!(det.format_id, "imagexpress");
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 3);
    assert_eq!(ds.plate().unwrap().planes_missing, 0);
    assert!(info.notes.iter().any(|n| n.contains("no HTD")));
}

/// A plate folder without its HTD is claimed only on positive evidence; folders other readers
/// own, and folders of TIFFs whose names merely look like wells, are not.
#[test]
fn imagexpress_folder_detection_needs_positive_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry();
    let write = |d: &Path, names: &[&str], desc: Option<&str>| {
        std::fs::create_dir_all(d).unwrap();
        for (i, n) in names.iter().enumerate() {
            let v = u16::try_from(i).unwrap();
            let bytes = desc.map_or_else(|| tiff(4, 4, v), |s| tiff_described(4, 4, v, s));
            std::fs::write(d.join(n), bytes).unwrap();
        }
    };
    let detected = |d: &Path| reg.detect(d).ok().map(|(_, det)| det.format_id);

    // MetaXpress names with MetaSeries headers: a plate (likely, no HTD).
    let plate = dir.path().join("plate");
    write(
        &plate,
        &[
            "HTS_A01_s1_w1.tif",
            "HTS_A01_s1_w2.tif",
            "HTS_B03_s1_w1.tif",
        ],
        Some(METASERIES),
    );
    let (_, det) = reg.detect(&plate).unwrap();
    assert_eq!(det.format_id, "imagexpress");
    assert_eq!(
        det.confidence,
        openreadout_core::model::DetectConfidence::Likely
    );
    // One well name and plain TIFFs: not enough.
    write(
        &dir.path().join("one"),
        &["HTS_A01_w1.tif"],
        Some(METASERIES),
    );
    assert_eq!(detected(&dir.path().join("one")), None);
    // The same names without MetaMorph headers are not claimed.
    let plain = dir.path().join("plain");
    write(&plain, &["HTS_A01_w1.tif", "HTS_A01_w2.tif"], None);
    assert_eq!(detected(&plain), None);
    // Olympus FluoView `.oif.files`: `s_C001.tif` parses as prefix `s`, well C1; never a plate.
    let oif = dir.path().join("50x.oif.files");
    write(&oif, &["s_C001.tif", "s_C002.tif"], Some(METASERIES));
    assert_eq!(detected(&oif), None);
    // Three-digit columns are not MetaXpress well names.
    let olympus_like = dir.path().join("channels");
    write(
        &olympus_like,
        &["s_C001.tif", "s_C002.tif"],
        Some(METASERIES),
    );
    assert_eq!(detected(&olympus_like), None);
    // A MetaMorph `.nd` series (owned by the TIFF reader), a Micro-Manager folder and
    // instrument `.d` folders are not claimed even with MetaXpress-like names and headers.
    for (folder, marker) in [
        ("nd-series", Some("exp.nd")),
        ("mm", Some("metadata.txt")),
        ("run.d", None),
        ("fid.fid", None),
    ] {
        let d = dir.path().join(folder);
        write(&d, &["exp_A01_w1.TIF", "exp_A02_w1.TIF"], Some(METASERIES));
        if let Some(m) = marker {
            std::fs::write(d.join(m), "x").unwrap();
        }
        assert_eq!(detected(&d), None, "{folder}");
    }
    // An HTD is definite whatever the plane files look like.
    std::fs::write(
        plain.join("HTS.HTD"),
        "\"HTSInfoFile\", Version 1.0\r\n\"XWells\", 12\r\n\"YWells\", 8\r\n\"EndFile\"\r\n",
    )
    .unwrap();
    let (_, det) = reg.detect(&plain).unwrap();
    assert_eq!(det.format_id, "imagexpress");
    assert_eq!(
        det.confidence,
        openreadout_core::model::DetectConfidence::Definite
    );
}

#[test]
fn damaged_indexes_are_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let index = harmony_plate(dir.path());
    let good = std::fs::read(&index).unwrap();
    let mut seed = 0x9e37_79b9_u32;
    for i in 0..300 {
        let mut b = good.clone();
        match i % 3 {
            0 => b.truncate((seed as usize) % b.len()),
            1 => {
                for _ in 0..8 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    let k = (seed as usize) % b.len();
                    b[k] = (seed >> 16) as u8;
                }
            }
            _ => {
                let k = (seed as usize) % b.len();
                b.splice(
                    k..k,
                    b"<Image><Row>99999999999</Row><URL>../../x</URL></Image>"
                        .iter()
                        .copied(),
                );
            }
        }
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        std::fs::write(&index, &b).unwrap();
        if let Ok(mut ds) = HarmonyReader.open(&index) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_plane(0, PlaneIndex::default());
        }
    }
}

/// A Columbus export: one multi-page TIFF per well, planes addressed by `URL@BufferNo`, channel
/// colours as ARGB numbers, internal (non-`RRCC`) well ids.
#[test]
fn columbus_index_with_multi_page_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut images = String::new();
    let mut n = 0;
    for (row, col, file) in [(1, 1, "001001-1.tif"), (2, 1, "002001-1.tif")] {
        let mut values = Vec::new();
        for z in 1..=2u32 {
            for ch in 1..=2u32 {
                let page = values.len();
                let value = u16::try_from(row * 1000 + z * 10 + ch).unwrap();
                values.push(value);
                let color = if ch == 1 {
                    4_287_795_858u32
                } else {
                    4_278_255_360
                };
                images.push_str(&format!(
                    r#"<Image Version="1"><id>i{n}</id><State>Ok</State><URL BufferNo="{page}">{file}</URL><Row>{row}</Row><Col>{col}</Col>
<FieldID>1</FieldID><PlaneID>{z}</PlaneID><TimepointID>1</TimepointID><ChannelID>{ch}</ChannelID>
<ChannelName>{name}</ChannelName><ChannelColor>{color}</ChannelColor><ChannelType>Fluorescence</ChannelType>
<ImageResolutionX Unit="m">1E-06</ImageResolutionX><ImageResolutionY Unit="m">1E-06</ImageResolutionY>
<ImageSizeX>8</ImageSizeX><ImageSizeY>4</ImageSizeY><PositionZ Unit="m">{pz}E-06</PositionZ><AbsTime>2022-02-27T07:45:2{z}Z</AbsTime></Image>"#,
                    name = if ch == 1 { "Brightfield" } else { "Alexa 488" },
                    pz = z * 5,
                ));
                n += 1;
            }
        }
        std::fs::write(dir.path().join(file), tiff_pages(8, 4, &values)).unwrap();
    }
    let index = dir.path().join("ImageIndex.ColumbusIDX.xml");
    std::fs::write(
        &index,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" ?><EvaluationInputData xmlns="http://www.perkinelmer.com/Columbus" Version="1">
<User>Undefined</User><Plates><Plate><PlateID>col-1</PlateID><PlateTypeName>6 well</PlateTypeName><PlateRows>2</PlateRows><PlateColumns>3</PlateColumns>
<Well id="227003" /><Well id="227004" /></Plate></Plates>
<Wells><Well><id>227003</id><Row>1</Row><Col>1</Col><Image id="i0" /></Well></Wells>
<Images>{images}</Images></EvaluationInputData>"#
        ),
    )
    .unwrap();
    let reg = registry();
    for open in [index.clone(), dir.path().to_path_buf()] {
        let (det, mut ds) = reg.open(&open).unwrap();
        assert_eq!(det.format_id, "opera-harmony");
        let info = ds.info().unwrap();
        assert_eq!(info.images.len(), 2);
        let im = &info.images[1];
        assert_eq!(im.extra["well"], "B01");
        assert_eq!((im.size_x, im.size_y, im.size_z, im.size_c), (8, 4, 2, 2));
        assert_eq!(im.channels[0].name.as_deref(), Some("Brightfield"));
        assert_eq!(im.channels[0].color.as_deref(), Some("#929292"));
        assert_eq!(im.channels[1].color.as_deref(), Some("#00FF00"));
        assert_eq!(im.physical_size.z, Some(5.0));
        for z in 0..2u32 {
            for c in 0..2u32 {
                let p = ds.read_plane(1, PlaneIndex { c, z, t: 0 }).unwrap();
                let want = u16::try_from(2000 + (z + 1) * 10 + c + 1).unwrap();
                assert_eq!(&p.data[..2], &want.to_le_bytes(), "z{z} c{c}");
            }
        }
        let plate = ds.plate().unwrap();
        assert_eq!(plate.id.as_deref(), Some("col-1"));
        assert!(plate.complete, "{plate:?}");
        assert!(ds.check().unwrap().ok);
    }
}

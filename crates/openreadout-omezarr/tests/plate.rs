//! OME-NGFF HCS plate export: a synthetic plate (3 fields in 2 wells, one field with a missing
//! plane file) written as a plate store and read back with the OME-Zarr reader.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::plate::{PlateSummary, PlateWell};
use openreadout_core::reader::PlaneIndex;
use openreadout_core::{
    Confidence, Dataset, Error, FormatReader, PixelType, Plane, ProvenanceMap, Result,
};
use openreadout_omezarr::{ZarrExportOptions, export_ome_zarr};
use serde_json::json;

struct Plate {
    images: Vec<ImageInfo>,
}

fn image(i: u32, well: &str, row: &str, column: u32, field: u32, missing: bool) -> ImageInfo {
    let mut im = ImageInfo::new(i, 6, 4, PixelType::Uint16);
    im.size_c = 2;
    let mut im = im.finish();
    im.extra.insert("well".into(), json!(well));
    im.extra.insert("row".into(), json!(row));
    im.extra.insert("column".into(), json!(column));
    im.extra.insert("field".into(), json!(field));
    if missing {
        im.extra.insert("planes_missing".into(), json!(1));
        im.extra.insert("missing_planes".into(), json!([[1, 0, 0]]));
    }
    im
}

impl Dataset for Plate {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: "plate".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "test-plate".into(),
                name: "Test plate".into(),
                vendor: String::new(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: self.images.clone(),
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            plane_count: 6,
            notes: vec![],
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if image == 2 && index.c == 1 {
            return Err(Error::io(
                "missing.tif",
                std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
            ));
        }
        let v = (image * 10 + index.c) as u16;
        Ok(Plane {
            width: 6,
            height: 4,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data: (0..24u16)
                .flat_map(|k| (v * 100 + k).to_le_bytes())
                .collect(),
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("plate", "test-plate"))
    }
    fn plate(&self) -> Option<PlateSummary> {
        let well = |name: &str, row: &str, ri: u32, col: u32, images: Vec<u32>| PlateWell {
            well: name.into(),
            row: row.into(),
            column: col,
            row_index: ri,
            column_index: col - 1,
            images,
            planes_missing: 0,
        };
        Some(PlateSummary {
            id: Some("P1".into()),
            rows: 8,
            columns: 12,
            wells: vec![
                well("A02", "A", 0, 2, vec![0, 1]),
                well("C05", "C", 2, 5, vec![2]),
            ],
            field_count: 2,
            planes_expected: 6,
            planes_missing: 1,
            complete: false,
            extra: BTreeMap::new(),
            ..PlateSummary::default()
        })
    }
}

fn plate() -> Plate {
    Plate {
        images: vec![
            image(0, "A02", "A", 2, 1, false),
            image(1, "A02", "A", 2, 2, false),
            image(2, "C05", "C", 5, 1, true),
        ],
    }
}

fn read_json(p: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn partial_plates_are_refused_unless_incomplete_fields_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("p.ome.zarr");
    let mut ds = plate();
    let e = export_ome_zarr(
        &mut ds,
        Path::new("plate"),
        &out,
        &ZarrExportOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    assert!(e.hint().unwrap().contains("--skip-incomplete"));
    // selecting only channel 0 needs no missing file
    let mut o = ZarrExportOptions::default();
    o.select = vec!["c=0".into()];
    let r = export_ome_zarr(&mut ds, Path::new("plate"), &out, &o).unwrap();
    assert_eq!(r.images_written, 3);
    assert_eq!(r.layout.as_deref(), Some("plate"));

    let mut o = ZarrExportOptions::default();
    o.skip_incomplete = true;
    o.overwrite = true;
    let r = export_ome_zarr(&mut ds, Path::new("plate"), &out, &o).unwrap();
    assert_eq!(r.images_written, 2);
    assert_eq!(r.images_skipped, vec![2]);
}

#[test]
fn plate_layout_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("p.ome.zarr");
    let mut ds = plate();
    let mut o = ZarrExportOptions::default();
    o.select = vec!["c=0".into()];
    export_ome_zarr(&mut ds, Path::new("plate"), &out, &o).unwrap();
    let root = read_json(&out.join("zarr.json"));
    let p = &root["attributes"]["ome"]["plate"];
    assert_eq!(p["name"], "P1");
    assert_eq!(p["rows"].as_array().unwrap().len(), 8);
    assert_eq!(p["columns"].as_array().unwrap().len(), 12);
    assert_eq!(p["field_count"], 2);
    assert_eq!(
        p["wells"],
        json!([
            {"path": "A/2", "rowIndex": 0, "columnIndex": 1},
            {"path": "C/5", "rowIndex": 2, "columnIndex": 4}
        ])
    );
    let well = read_json(&out.join("A/2/zarr.json"));
    assert_eq!(
        well["attributes"]["ome"]["well"]["images"],
        json!([{"path": "0"}, {"path": "1"}])
    );
    assert!(out.join("A/2/1/zarr.json").is_file());
    // the OME-Zarr reader sees a plate with the same wells and fields
    let mut back = openreadout_zarr::ZarrReader.open(&out).unwrap();
    let info = back.info().unwrap();
    assert_eq!(info.images.len(), 3);
    let layout = openreadout_core::plate::plate_layout(back.as_ref(), &info).unwrap();
    let names: Vec<&str> = layout.wells.iter().map(|w| w.well.as_str()).collect();
    assert_eq!(names, ["A02", "C05"]);
    assert_eq!(layout.wells[0].images.len(), 2);
    let plane = back.read_plane(1, PlaneIndex::default()).unwrap();
    let orig = ds.read_plane(1, PlaneIndex::default()).unwrap();
    assert_eq!(plane.data, orig.data);
}

#[test]
fn wells_select_images_and_no_plate_writes_a_collection() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("p.ome.zarr");
    let mut ds = plate();
    let mut o = ZarrExportOptions::default();
    o.wells = vec!["a2".into()];
    let r = export_ome_zarr(&mut ds, Path::new("plate"), &out, &o).unwrap();
    assert_eq!(r.images_written, 2);
    let mut o = ZarrExportOptions::default();
    o.wells = vec!["B01".into()];
    let e = export_ome_zarr(&mut ds, Path::new("plate"), &out, &o).unwrap_err();
    assert_eq!(e.exit_code(), 2);
    let mut o = ZarrExportOptions::default();
    o.plate = false;
    o.select = vec!["c=0".into()];
    o.overwrite = true;
    let r = export_ome_zarr(&mut ds, Path::new("plate"), &out, &o).unwrap();
    assert_eq!(r.layout, None);
    let root = read_json(&out.join("zarr.json"));
    assert_eq!(root["attributes"]["ome"]["bioformats2raw.layout"], 3);
}

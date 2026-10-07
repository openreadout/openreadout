//! Signed integer planes (MRC micrographs and segmentations are int16 and int8) export to
//! OME-TIFF with `SampleFormat` 2 and read back unchanged. Until 2026-10-06 the export refused
//! them with exit 6 (found on `zenodo15871573-carbon-ctf` and `zenodo10814409-chlamy-seg`).

use std::path::Path;

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::{
    Confidence, Dataset, FormatReader, PixelType, Plane, PlaneIndex, ProvenanceMap, Result,
};
use openreadout_ometiff::{ExportOptions, export_ome_tiff};

struct Signed {
    pixel_type: PixelType,
}

impl Signed {
    fn plane_bytes(&self) -> Vec<u8> {
        let n: i32 = 8 * 4;
        match self.pixel_type {
            PixelType::Int8 => (0..n).map(|i| (i as i8 - 16) as u8).collect(),
            PixelType::Int16 => (0..n)
                .flat_map(|i| (i as i16 * 1000 - 16_000).to_le_bytes())
                .collect(),
            _ => (0..n)
                .flat_map(|i| (i * 100_000 - 1_600_000).to_le_bytes())
                .collect(),
        }
    }
}

impl Dataset for Signed {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: "synthetic".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                vendor: String::new(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            plane_count: 1,
            images: vec![ImageInfo::new(0, 8, 4, self.pixel_type).finish()],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
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
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Ok(Plane {
            width: 8,
            height: 4,
            pixel_type: self.pixel_type,
            samples_per_pixel: 1,
            data: self.plane_bytes(),
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synthetic", "synthetic"))
    }
}

#[test]
fn signed_integer_planes_round_trip() {
    for pt in [PixelType::Int8, PixelType::Int16, PixelType::Int32] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("s.ome.tiff");
        let mut ds = Signed { pixel_type: pt };
        let r = export_ome_tiff(
            &mut ds,
            Path::new("synthetic"),
            &out,
            &ExportOptions::default(),
        )
        .unwrap();
        assert!(r.verified, "{pt:?}");
        let mut back = openreadout_tiff::TiffReader
            .open_input(&openreadout_core::Input::local(&out))
            .unwrap();
        assert_eq!(back.info().unwrap().images[0].pixel_type, pt);
        let p = back.read_plane(0, PlaneIndex::default()).unwrap();
        assert_eq!(p.data, ds.plane_bytes(), "{pt:?}");
    }
}

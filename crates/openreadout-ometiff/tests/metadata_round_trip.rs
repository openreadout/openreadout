//! Normalized metadata survives an OME-TIFF round trip: a synthetic source with the fields the
//! 2026-09-25 audit found lost (detection bands of a λ scan, acquisition modes, objective model
//! and immersion, software, sub-millisecond acquisition time, the instrument's detector next
//! to per-channel detectors, an unnamed image) is exported and read back with OpenReadout's
//! TIFF reader; `check --against`'s metadata diff must be empty.

use std::path::Path;

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, FormatDescriptor, ImageInfo, InstrumentInfo, LsEntry,
    ObjectiveInfo,
};
use openreadout_core::{
    Confidence, Dataset, FormatReader, PixelType, Plane, PlaneIndex, ProvenanceMap, Result,
};
use openreadout_ometiff::{ExportOptions, export_ome_tiff};
use openreadout_ops::compare::{DEFAULT_IGNORES, json_diff};
use serde_json::json;

struct Synthetic {
    images: Vec<ImageInfo>,
}

fn lambda_scan() -> ImageInfo {
    let mut im = ImageInfo::new(0, 8, 4, PixelType::Uint16);
    im.name = Some("Lambda".into());
    im.size_c = 3;
    im.acquired_at = Some("2025-02-03T04:05:06.1234567Z".into());
    im.channels = (0..3u32)
        .map(|k| {
            let start = 420.0 + 10.0 * f64::from(k);
            let mut c = ChannelInfo {
                index: k,
                name: Some(format!("Gray {start:.0} nm")),
                emission_nm: Some(start),
                acquisition_mode: Some("Laser Scanning Confocal".into()),
                ..ChannelInfo::default()
            };
            c.set_band(start, start + 20.0);
            c
        })
        .collect();
    im.objective = Some(ObjectiveInfo {
        // trailing blank as LAS X writes it; the model trims it
        model: Some("HC PL APO CS2    10x/0.40 DRY ".into()),
        nominal_magnification: Some(10.0),
        lens_na: Some(0.4),
        immersion: Some("DRY".into()),
    });
    im.instrument = Some(InstrumentInfo {
        manufacturer: Some("Leica Microsystems".into()),
        model: Some("STELLARIS 8".into()),
        software: Some("LAS X".into()),
        software_version: Some("4.8.1.29271".into()),
        detector: None,
    });
    im.finish()
}

fn widefield() -> ImageInfo {
    let mut im = ImageInfo::new(1, 8, 4, PixelType::Uint16);
    // no name: the export must not invent one
    im.size_c = 3;
    im.acquired_at = Some("2019-06-27T18:39:25.8078869Z".into());
    im.channels = vec![
        ChannelInfo {
            index: 0,
            name: Some("EGFP".into()),
            excitation_nm: Some(488.0),
            emission_nm: Some(509.0),
            // OME tokens as CZI stores them: the model turns them into a label
            acquisition_mode: Some("WideField".into()),
            ..ChannelInfo::default()
        },
        ChannelInfo {
            index: 1,
            name: Some("Bright".into()),
            acquisition_mode: Some("Brightfield (RGB)".into()),
            ..ChannelInfo::default()
        },
        ChannelInfo {
            index: 2,
            name: Some("mCherry".into()),
            emission_range_nm: Some([580.0, 640.0]),
            acquisition_mode: Some("Spinning Disk Confocal Fluorescence".into()),
            ..ChannelInfo::default()
        },
    ];
    im.objective = Some(ObjectiveInfo {
        model: Some("Plan-Apochromat 40x/1.25 Sil".into()),
        immersion: Some("Silicone".into()),
        ..ObjectiveInfo::default()
    });
    im.instrument = Some(InstrumentInfo {
        detector: Some("Hamamatsu C11440-22C SN:101412".into()),
        software: Some("NIS-Elements".into()),
        ..InstrumentInfo::default()
    });
    im.extra.insert(
        "channel_settings".into(),
        json!([{"index": 0, "detector": "Flash4.0, SN:101412"}]),
    );
    im.finish()
}

impl Dataset for Synthetic {
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
            plane_count: 6,
            images: self.images.clone(),
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
    fn read_plane(&mut self, _: u32, idx: PlaneIndex) -> Result<Plane> {
        Ok(Plane {
            width: 8,
            height: 4,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data: vec![u8::try_from(idx.c).unwrap_or(0); 64],
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synthetic", "synthetic"))
    }
}

#[test]
fn normalized_metadata_survives_ome_tiff() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("m.ome.tiff");
    let mut ds = Synthetic {
        images: vec![lambda_scan(), widefield()],
    };
    let r = export_ome_tiff(
        &mut ds,
        Path::new("synthetic"),
        &out,
        &ExportOptions::default(),
    )
    .unwrap();
    assert!(r.verified);
    let back = openreadout_tiff::TiffReader
        .open_input(&openreadout_core::Input::local(&out))
        .unwrap()
        .info()
        .unwrap();
    let src = ds.info().unwrap();

    // What the model made of the source text.
    let (a, b) = (&src.images[0], &src.images[1]);
    assert_eq!(
        a.objective.as_ref().unwrap().model.as_deref(),
        Some("HC PL APO CS2    10x/0.40 DRY")
    );
    assert_eq!(
        a.objective.as_ref().unwrap().immersion.as_deref(),
        Some("Air")
    );
    assert_eq!(b.channels[0].acquisition_mode.as_deref(), Some("Widefield"));
    assert_eq!(a.channels[1].emission_band_center_nm, Some(440.0));

    // Read back: the bands, modes, objective, software and exact time.
    let (ra, rb) = (&back.images[0], &back.images[1]);
    assert_eq!(ra.channels[2].emission_range_nm, Some([440.0, 460.0]));
    assert_eq!(ra.channels[2].emission_nm, Some(440.0));
    assert_eq!(rb.channels[2].emission_nm, None, "no invented wavelength");
    assert_eq!(
        rb.channels[1].acquisition_mode.as_deref(),
        Some("Brightfield (RGB)")
    );
    assert_eq!(
        ra.instrument.as_ref().unwrap().software_version.as_deref(),
        Some("4.8.1.29271")
    );
    assert_eq!(
        rb.objective.as_ref().unwrap().immersion.as_deref(),
        Some("Silicone")
    );
    assert_eq!(rb.name, None);

    let ignore: Vec<String> = DEFAULT_IGNORES.iter().map(|s| (*s).to_string()).collect();
    let (n, diffs) = json_diff(
        &serde_json::to_value(&src).unwrap(),
        &serde_json::to_value(&back).unwrap(),
        &ignore,
        false,
    );
    assert_eq!(n, 0, "{diffs:#?}");
}

//! Previews of a synthetic in-memory dataset: geometry, determinism and golden hashes.

#![allow(clippy::float_cmp, clippy::many_single_char_names)]

use std::collections::BTreeMap;

use openreadout_core::model::{
    ChannelInfo, CheckReport, ColumnInfo, FileInfo, FormatDescriptor, ImageInfo, LsEntry,
    SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::pixel::{PixelType, Plane};
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{Error, Result};
use openreadout_preview::{
    Axis, Contrast, Encoding, PreviewRequest, finish, finish_within, render,
};

/// Golden xxh3-128 of the pixels decoded from the default PNG preview of the synthetic image
/// (8-bit greyscale, row-major). The compressed bytes are only checked for run-to-run
/// determinism: they depend on the deflate implementation, which a dependency update or feature
/// unification may change without changing a single pixel.
const GOLDEN_PNG_PIXELS: &str = "30eaef99e9ec0cbf7d2f7d050d58def9";
/// Golden xxh3-128 of the raw RGB pixels of the composite MIP (independent of the encoder).
const GOLDEN_COMPOSITE_RGB: &str = "1081bb70ad81d740ece9fa79ef6c2906";

struct Synth;

fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: "synth".into(),
        name: "Synthetic".into(),
        vendor: "test".into(),
        extensions: vec![],
        family: "microscopy".into(),
        can_read: true,
        can_write: false,
        confidence: Confidence::High,
        known_gaps: vec![],
    }
}

fn info() -> FileInfo {
    let mut im = ImageInfo::new(0, 64, 48, PixelType::Uint16);
    im.size_c = 2;
    im.size_z = 3;
    im.channels = vec![
        ChannelInfo {
            index: 0,
            name: Some("DAPI".into()),
            emission_nm: Some(461.0),
            ..ChannelInfo::default()
        },
        ChannelInfo {
            index: 1,
            name: Some("GFP".into()),
            color: Some("#00FF00".into()),
            ..ChannelInfo::default()
        },
    ];
    let im = im.finish();
    let wells: Vec<ColumnInfo> = ["A1", "A2", "B1", "H12"]
        .iter()
        .enumerate()
        .map(|(i, n)| ColumnInfo {
            index: i as u32,
            name: (*n).into(),
            dtype: "float64".into(),
            ..ColumnInfo::default()
        })
        .collect();
    FileInfo {
        path: "synthetic".into(),
        size_bytes: 0,
        format: descriptor(),
        format_version: None,
        plane_count: im.plane_count,
        images: vec![im],
        tables: vec![TableInfo {
            index: 0,
            name: None,
            row_count: 2,
            columns: wells,
            extra: BTreeMap::new(),
        }],
        spectra: vec![],
        traces: vec![TraceInfo {
            index: 0,
            name: None,
            sample_rate_hz: 1000.0,
            sample_count: 5000,
            sweep_count: 1,
            channels: vec![SignalChannelInfo {
                index: 0,
                name: "Vm".into(),
                unit: Some("mV".into()),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            }],
            start_s: Some(0.0),
            extra: BTreeMap::new(),
        }],
        notes: vec![],
    }
}

impl Dataset for Synth {
    fn info(&self) -> Result<FileInfo> {
        Ok(info())
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::default()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, image: u32, i: PlaneIndex) -> Result<Plane> {
        if image != 0 || i.c > 1 || i.z > 2 || i.t > 0 {
            return Err(Error::Usage("out of range".into()));
        }
        let mut data = Vec::with_capacity(64 * 48 * 2);
        for y in 0..48u32 {
            for x in 0..64u32 {
                let v: u32 = if i.c == 0 {
                    x * 100 + y * 10 + i.z * 7
                } else {
                    // a bright disc that moves with z
                    let (cx, cy) = (20 + i.z * 10, 24u32);
                    let d2 = (x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2);
                    if d2 < 64 { 4000 } else { 100 }
                };
                data.extend_from_slice(&(v as u16).to_le_bytes());
            }
        }
        Ok(Plane {
            width: 64,
            height: 48,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data,
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synthetic", "synth"))
    }
    fn read_trace(&mut self, _: u32, _: u32, first: u64, max: u64) -> Result<Trace> {
        let n = max.min(5000u64.saturating_sub(first));
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample: first,
            channels: vec![
                (first..first + n)
                    .map(|i| {
                        -70.0
                            + 20.0 * ((i as f64) / 200.0).sin()
                            + if i % 997 == 0 { 60.0 } else { 0.0 }
                    })
                    .collect(),
            ],
        })
    }
    fn read_table(&mut self, _: u32, first: u64, max: u64) -> Result<Table> {
        let rows = [[1.0, 2.0, 3.0, 4.0], [3.0, 4.0, 5.0, 6.0]];
        let r: Vec<&[f64; 4]> = rows
            .iter()
            .skip(first as usize)
            .take(max as usize)
            .collect();
        Ok(Table {
            table: 0,
            first_row: first,
            columns: (0..4)
                .map(|c| r.iter().map(|row| row[c]).collect())
                .collect(),
        })
    }
}

fn hash(b: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(b))
}

fn png_size(b: &[u8]) -> (u32, u32) {
    assert_eq!(&b[..8], b"\x89PNG\r\n\x1a\n");
    (
        u32::from_be_bytes([b[16], b[17], b[18], b[19]]),
        u32::from_be_bytes([b[20], b[21], b[22], b[23]]),
    )
}

#[test]
fn default_image_preview_is_deterministic_and_golden() {
    let info = info();
    // plain (no rulers): the bare downsampled plane, as before rulers existed
    let mut req = PreviewRequest::default();
    req.axes = false;
    let a = render(&mut Synth, &info, &req).unwrap();
    let b = render(&mut Synth, &info, &req).unwrap();
    let (out, bytes) = finish(&a, Encoding::Png, 90).unwrap();
    let (_, bytes2) = finish(&b, Encoding::Png, 90).unwrap();
    assert_eq!(bytes, bytes2, "identical requests give identical bytes");
    assert_eq!(png_size(&bytes), (64, 48));
    assert_eq!(out.kind, "image");
    let im = out.image.as_ref().unwrap();
    assert_eq!((im.z.clone(), im.c.clone()), (vec![1], vec![0]));
    assert_eq!(im.lut, "gray");
    assert_eq!(out.xxh3, hash(&bytes));
    let distinct: std::collections::BTreeSet<u8> = a.canvas.rgb.iter().copied().collect();
    assert!(distinct.len() > 50, "a gradient must use many grey levels");
    let dec = png::Decoder::new(std::io::Cursor::new(&bytes[..]));
    let mut rd = dec.read_info().unwrap();
    let mut px = vec![0u8; rd.output_buffer_size().unwrap()];
    let frame = rd.next_frame(&mut px).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    px.truncate(frame.buffer_size());
    assert_eq!(px.len(), 64 * 48);
    let gray: Vec<u8> = a
        .canvas
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| p[0])
        .collect();
    assert_eq!(px, gray, "the PNG holds the rendered pixels");
    assert_eq!(hash(&px), GOLDEN_PNG_PIXELS, "preview pixels changed");
}

#[test]
fn composite_mip_downsampled() {
    let info = info();
    let req = {
        let mut preview_request = PreviewRequest::default();
        preview_request.composite = true;
        preview_request.mip = Some(Axis::Z);
        preview_request.max_size = 32;
        preview_request.contrast = Contrast::MinMax;
        preview_request
    };
    let r = render(&mut Synth, &info, &req).unwrap();
    assert_eq!((r.canvas.width, r.canvas.height), (32, 24));
    let im = r.output.image.as_ref().unwrap();
    assert_eq!(im.downsample, 2);
    assert_eq!(im.z, vec![0, 1, 2]);
    assert_eq!(im.projection.as_deref(), Some("max-z"));
    assert_eq!(im.channels[0].color_source, "wavelength");
    assert_eq!(im.channels[1].color, "#00FF00");
    assert!(!r.canvas.is_gray());
    let h = hash(&r.canvas.rgb);
    assert_eq!(h, GOLDEN_COMPOSITE_RGB, "composite pixels changed");
}

#[test]
fn selection_errors_are_usage() {
    let info = info();
    for sel in ["z=0-1", "c=5", "z=9"] {
        let req = {
            let mut preview_request = PreviewRequest::default();
            preview_request.select = vec![sel.into()];
            preview_request
        };
        let e = render(&mut Synth, &info, &req).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{sel}: {e}");
    }
    // several channels imply a composite
    let req = {
        let mut preview_request = PreviewRequest::default();
        preview_request.select = vec!["c=0,1,z=2".into()];
        preview_request
    };
    let r = render(&mut Synth, &info, &req).unwrap();
    assert!(r.output.image.unwrap().composite);
}

#[test]
fn trace_and_plate_previews() {
    let info = info();
    let req = {
        let mut preview_request = PreviewRequest::default();
        preview_request.sweep = Some(0);
        preview_request.max_size = 400;
        preview_request
    };
    let r = render(&mut Synth, &info, &req).unwrap();
    assert_eq!(r.output.kind, "trace");
    assert_eq!(r.canvas.width, 400);
    let t = r.output.trace.as_ref().unwrap();
    assert_eq!(t.sample_count, 5000);
    let ax = t.x_axis.as_ref().unwrap();
    assert_eq!((ax.unit.as_str(), ax.first), ("s", 0.0));
    assert!((ax.last - 4.999).abs() < 1e-9);
    assert!(!r.canvas.is_gray(), "the trace is drawn in colour");

    let req = {
        let mut preview_request = PreviewRequest::default();
        preview_request.table = Some(0);
        preview_request.max_size = 300;
        preview_request
    };
    let r = render(&mut Synth, &info, &req).unwrap();
    let p = r.output.plate.as_ref().unwrap();
    assert_eq!((p.rows, p.columns, p.wells), (8, 12, 4));
    assert_eq!(p.value, "mean of 2 rows");
    assert_eq!((p.min, p.max), (Some(2.0), Some(5.0)));
    let (_, bytes) = finish(&r, Encoding::Png, 90).unwrap();
    assert_eq!(png_size(&bytes), (r.canvas.width, r.canvas.height));
}

#[test]
fn jpeg_decodes_and_budget_is_respected() {
    let info = info();
    let req = {
        let mut preview_request = PreviewRequest::default();
        preview_request.composite = true;
        preview_request.axes = false;
        preview_request
    };
    let r = render(&mut Synth, &info, &req).unwrap();
    let (out, bytes) = finish(&r, Encoding::Jpeg, 90).unwrap();
    assert_eq!(out.encoding, "jpeg");
    let mut d = jpeg_decoder::Decoder::new(&bytes[..]);
    let px = d.decode().unwrap();
    let meta = d.info().unwrap();
    assert_eq!((u32::from(meta.width), u32::from(meta.height)), (64, 48));
    // mean absolute error of the decoded image stays small at quality 90
    let err: f64 = px
        .iter()
        .zip(&r.canvas.rgb)
        .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
        .sum::<f64>()
        / px.len() as f64;
    assert!(err < 6.0, "mean abs error {err}");
    let (small, b) = finish_within(&r, Encoding::Png, 90, 100).unwrap();
    assert!(b.len() <= 100 || small.width <= 32, "{} bytes", b.len());
    assert_eq!(small.encoding, "jpeg");
    assert!(!small.notes.is_empty());
}

#[test]
fn rulers_frame_the_unchanged_plane() {
    let info = info();
    let mut plain = PreviewRequest::default();
    plain.axes = false;
    let p = render(&mut Synth, &info, &plain).unwrap();
    let a = render(&mut Synth, &info, &PreviewRequest::default()).unwrap();
    let b = render(&mut Synth, &info, &PreviewRequest::default()).unwrap();
    let (_, ab) = finish(&a, Encoding::Png, 90).unwrap();
    let (_, bb) = finish(&b, Encoding::Png, 90).unwrap();
    assert_eq!(ab, bb, "identical requests give identical bytes");
    let im = a.output.image.as_ref().unwrap();
    assert!(im.axes);
    let pa = im.plot_area;
    assert_eq!((pa.width, pa.height), (64, 48));
    assert!(pa.x > 0 && pa.y > 0);
    assert!(a.canvas.width > 64 && a.canvas.height > 48);
    assert!(a.canvas.width.max(a.canvas.height) <= 1024);
    assert_eq!(png_size(&ab), (a.canvas.width, a.canvas.height));
    assert_eq!(im.source_per_pixel, 1.0);
    assert_eq!((im.source_origin.x, im.source_origin.y), (0.0, 0.0));
    assert_eq!(
        im.full_res_region,
        openreadout_core::Region::new(0, 0, 64, 48)
    );
    assert!(im.scale_bar.is_none(), "no pixel size, no scale bar");
    // the plot area holds exactly the plain rendering
    let w = a.canvas.width as usize;
    for y in 0..48usize {
        let row = (y + pa.y as usize) * w + pa.x as usize;
        assert_eq!(
            &a.canvas.rgb[row * 3..(row + 64) * 3],
            &p.canvas.rgb[y * 64 * 3..(y + 1) * 64 * 3]
        );
    }
    let pi = p.output.image.as_ref().unwrap();
    assert!(!pi.axes);
    assert_eq!(pi.plot_area, openreadout_core::Region::new(0, 0, 64, 48));
    // the whole picture honours max_size, rulers included
    let mut small = PreviewRequest::default();
    small.max_size = 60;
    let s = render(&mut Synth, &info, &small).unwrap();
    assert!(s.canvas.width.max(s.canvas.height) <= 60);
    let si = s.output.image.as_ref().unwrap();
    assert!(si.axes);
    assert!(si.source_per_pixel > 1.0);
}

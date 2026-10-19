//! Synthetic Igor files written from the format note (`docs/formats/igor.md`): version-5 and
//! version-2 binary waves in both byte orders, a labelled force-curve wave, an AFM-style image
//! with labelled layers, a packed experiment with a data folder, variables and a text record,
//! and damaged files (no panic, clean errors).

use openreadout_core::FormatReader;
use openreadout_core::reader::PlaneIndex;
use openreadout_igor::IgorReader;

fn fix_checksum(b: &mut [u8], n: usize, big: bool, at: usize) {
    b[at] = 0;
    b[at + 1] = 0;
    let mut sum: u16 = 0;
    for w in b[..n].chunks_exact(2) {
        let v = if big {
            u16::from_be_bytes([w[0], w[1]])
        } else {
            u16::from_le_bytes([w[0], w[1]])
        };
        sum = sum.wrapping_add(v);
    }
    let c = 0u16.wrapping_sub(sum);
    let bytes = if big { c.to_be_bytes() } else { c.to_le_bytes() };
    b[at..at + 2].copy_from_slice(&bytes);
}

struct W5 {
    name: &'static str,
    wave_type: i16,
    dims: [i32; 4],
    steps: [f64; 4],
    offsets: [f64; 4],
    data_units: &'static str,
    dim_units: [&'static str; 4],
    data: Vec<u8>,
    note: &'static str,
    labels: [Vec<&'static str>; 4],
    big: bool,
}

impl W5 {
    fn bytes(&self) -> Vec<u8> {
        let big = self.big;
        let i16b = |v: i16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let i32b = |v: i32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let f64b = |v: f64| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut b = vec![0u8; 384];
        b[0..2].copy_from_slice(&i16b(5));
        b[4..8].copy_from_slice(&i32b(320 + self.data.len() as i32));
        b[12..16].copy_from_slice(&i32b(self.note.len() as i32));
        let label_bytes: Vec<Vec<u8>> = self
            .labels
            .iter()
            .map(|l| {
                let mut out = Vec::new();
                for s in l {
                    let mut f = [0u8; 32];
                    f[..s.len()].copy_from_slice(s.as_bytes());
                    out.extend_from_slice(&f);
                }
                out
            })
            .collect();
        for (d, l) in label_bytes.iter().enumerate() {
            b[36 + 4 * d..40 + 4 * d].copy_from_slice(&i32b(l.len() as i32));
        }
        let w = 64;
        let points: i32 = self.dims.iter().filter(|d| **d > 0).product();
        b[w + 4..w + 8].copy_from_slice(&i32b(3_700_000_000u32 as i32));
        b[w + 12..w + 16].copy_from_slice(&i32b(points));
        b[w + 16..w + 18].copy_from_slice(&i16b(self.wave_type));
        b[w + 26..w + 28].copy_from_slice(&i16b(1));
        b[w + 28..w + 28 + self.name.len()].copy_from_slice(self.name.as_bytes());
        for d in 0..4 {
            b[w + 68 + 4 * d..w + 72 + 4 * d].copy_from_slice(&i32b(self.dims[d]));
            b[w + 84 + 8 * d..w + 92 + 8 * d].copy_from_slice(&f64b(self.steps[d]));
            b[w + 116 + 8 * d..w + 124 + 8 * d].copy_from_slice(&f64b(self.offsets[d]));
            let u = self.dim_units[d].as_bytes();
            b[w + 152 + 4 * d..w + 152 + 4 * d + u.len()].copy_from_slice(u);
        }
        let u = self.data_units.as_bytes();
        b[w + 148..w + 148 + u.len()].copy_from_slice(u);
        fix_checksum(&mut b, 384, big, 2);
        b.extend_from_slice(&self.data);
        b.extend_from_slice(self.note.as_bytes());
        for l in &label_bytes {
            b.extend_from_slice(l);
        }
        b
    }
}

fn f32s(v: &[f32], big: bool) -> Vec<u8> {
    v.iter()
        .flat_map(|x| if big { x.to_be_bytes() } else { x.to_le_bytes() })
        .collect()
}

fn write(dir: &tempfile::TempDir, name: &str, b: &[u8]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, b).unwrap();
    p
}

fn trace_wave(big: bool) -> W5 {
    W5 {
        name: "sweep0",
        wave_type: 2,
        dims: [5, 0, 0, 0],
        steps: [1e-4, 1.0, 1.0, 1.0],
        offsets: [0.5, 0.0, 0.0, 0.0],
        data_units: "A",
        dim_units: ["s", "", "", ""],
        data: f32s(&[1.0, 2.0, 3.0, 4.0, 5.0], big),
        note: "Amp: 1\rGain: 2",
        labels: Default::default(),
        big,
    }
}

#[test]
fn version5_trace_in_both_byte_orders() {
    let dir = tempfile::tempdir().unwrap();
    for big in [false, true] {
        let p = write(&dir, "sweep.ibw", &trace_wave(big).bytes());
        let mut ds = IgorReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.format_version.as_deref(), Some("ibw 5"));
        assert_eq!(info.traces.len(), 1);
        let t = &info.traces[0];
        assert_eq!(t.sample_count, 5);
        assert!((t.sample_rate_hz - 10_000.0).abs() < 1e-6);
        assert_eq!(t.start_s, Some(0.5));
        assert_eq!(t.channels[0].unit.as_deref(), Some("A"));
        let tr = ds.read_trace(0, 0, 1, 3).unwrap();
        assert_eq!(tr.channels[0], vec![2.0, 3.0, 4.0]);
        let v = ds.vendor_metadata().unwrap();
        assert_eq!(v["waves"][0]["note_fields"]["Gain"], "2");
        let r = ds.check().unwrap();
        assert!(r.findings.iter().all(|f| f.code != "checksum"), "{:?}", r.findings);
    }
}

#[test]
fn labelled_columns_are_trace_channels_and_labelled_layers_are_image_channels() {
    let dir = tempfile::tempdir().unwrap();
    let curve = W5 {
        name: "curve",
        wave_type: 4,
        dims: [3, 2, 0, 0],
        steps: [1.0, 1.0, 1.0, 1.0],
        offsets: [0.0; 4],
        data_units: "m",
        dim_units: ["", "", "", ""],
        data: [1.0f64, 2.0, 3.0, 10.0, 20.0, 30.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect(),
        note: "",
        labels: [vec![], vec!["", "Raw", "Defl"], vec![], vec![]],
        big: false,
    };
    let p = write(&dir, "curve.ibw", &curve.bytes());
    let mut ds = IgorReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.channels.len(), 2);
    assert_eq!(t.channels[1].name, "Defl");
    assert!(t.extra.contains_key("axis"));
    assert_eq!(ds.read_trace(0, 0, 0, 10).unwrap().channels[1], vec![10.0, 20.0, 30.0]);

    let img = W5 {
        name: "Image0001",
        wave_type: 2,
        dims: [2, 3, 2, 0],
        steps: [1e-8, 1e-8, 1.0, 1.0],
        offsets: [0.0; 4],
        data_units: "m",
        dim_units: ["m", "m", "", ""],
        data: f32s(&(0..12).map(|x| x as f32).collect::<Vec<_>>(), false),
        note: "",
        labels: [vec![], vec![], vec!["", "HeightRetrace", "PhaseRetrace"], vec![]],
        big: false,
    };
    let p = write(&dir, "img.ibw", &img.bytes());
    let mut ds = IgorReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_c, im.size_z), (2, 3, 2, 1));
    assert_eq!(im.channels[1].name.as_deref(), Some("PhaseRetrace"));
    assert!((im.physical_size.x.unwrap() - 0.01).abs() < 1e-12);
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    let vals: Vec<f32> = plane
        .data
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(vals, vec![6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
}

fn version2(big: bool) -> Vec<u8> {
    let i16b = |v: i16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let i32b = |v: i32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let note = b"This is a test.";
    let mut b = vec![0u8; 16 + 110];
    b[0..2].copy_from_slice(&i16b(2));
    b[2..6].copy_from_slice(&i32b(110 + 20 + 16));
    b[6..10].copy_from_slice(&i32b(note.len() as i32));
    b[16..18].copy_from_slice(&i16b(2));
    b[22..30].copy_from_slice(b"version2");
    b[58..62].copy_from_slice(&i32b(5));
    b[64..72].copy_from_slice(&if big { 1.0f64.to_be_bytes() } else { 1.0f64.to_le_bytes() });
    b.extend_from_slice(&f32s(&[5.0, 4.0, 3.0, 2.0, 1.0], big));
    b.extend_from_slice(&[0u8; 16]);
    fix_checksum(&mut b, 142, big, 14);
    b.extend_from_slice(note);
    b
}

#[test]
fn version2_waves_read_in_both_byte_orders() {
    let dir = tempfile::tempdir().unwrap();
    for big in [false, true] {
        let p = write(&dir, "v2.ibw", &version2(big));
        let mut ds = IgorReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.format_version.as_deref(), Some("ibw 2"));
        assert_eq!(ds.read_trace(0, 0, 0, 5).unwrap().channels[0], vec![5.0, 4.0, 3.0, 2.0, 1.0]);
        let v = ds.vendor_metadata().unwrap();
        assert_eq!(v["waves"][0]["note"], "This is a test.");
    }
}

fn record(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&kind.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&(data.len() as i32).to_le_bytes());
    b.extend_from_slice(data);
    b
}

fn packed() -> Vec<u8> {
    let mut vars = Vec::new();
    vars.extend_from_slice(&1i16.to_le_bytes());
    vars.extend_from_slice(&1i16.to_le_bytes()); // one system variable
    vars.extend_from_slice(&1i16.to_le_bytes()); // one numeric
    vars.extend_from_slice(&1i16.to_le_bytes()); // one string
    vars.extend_from_slice(&0.5f32.to_le_bytes());
    let mut name = [0u8; 32];
    name[..6].copy_from_slice(b"V_gain");
    vars.extend_from_slice(&name);
    vars.extend_from_slice(&1i16.to_le_bytes());
    vars.extend_from_slice(&4i16.to_le_bytes());
    vars.extend_from_slice(&20.0f64.to_le_bytes());
    vars.extend_from_slice(&0.0f64.to_le_bytes());
    vars.extend_from_slice(&[0u8; 4]);
    let mut name = [0u8; 32];
    name[..6].copy_from_slice(b"S_cell");
    vars.extend_from_slice(&name);
    vars.extend_from_slice(&5i16.to_le_bytes());
    vars.extend_from_slice(b"CA1-3");
    let mut folder = [0u8; 32];
    folder[..6].copy_from_slice(b"cell 1");
    let mut b = Vec::new();
    b.extend(record(11, &[0u8; 52]));
    b.extend(record(1, &vars));
    b.extend(record(9, &folder));
    b.extend(record(3, &trace_wave(false).bytes()));
    b.extend(record(10, &[]));
    b.extend(record(4, b"// Platform=WindowsNT, IGORVersion=9.050, architecture=Intel\r\n"));
    b.extend(record(0x8000 | 3, &version2(false)));
    b
}

#[test]
fn packed_experiment_with_folders_variables_and_a_superseded_wave() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(&dir, "exp.pxp", &packed());
    let mut ds = IgorReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("pxp"));
    assert_eq!(info.traces.len(), 1, "the superseded wave is skipped");
    assert_eq!(info.traces[0].name.as_deref(), Some("root:cell 1:sweep0"));
    assert_eq!(info.traces[0].extra["application_version"], "9.050");
    let v = ds.vendor_metadata().unwrap();
    assert_eq!(v["variables"][0]["variables"][0]["value"], 20.0);
    assert_eq!(v["variables"][0]["variables"][1]["text"], "CA1-3");
    let r = ds.check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "superseded"));
}

#[test]
fn damaged_files_give_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = trace_wave(false).bytes();
    b.truncate(390);
    let p = write(&dir, "cut.ibw", &b);
    let mut ds = IgorReader.open(&p).unwrap();
    assert!(ds.info().unwrap().traces.is_empty(), "the cut wave is not readable");
    assert!(ds.check().unwrap().findings.iter().any(|f| f.code == "truncated"));
    let p = write(&dir, "junk.pxp", b"not an igor file at all, just text");
    assert!(IgorReader.open(&p).is_err());
    // byte flips in the headers never panic
    let good = packed();
    for i in (0..good.len()).step_by(7) {
        let mut b = good.clone();
        b[i] ^= 0xff;
        let p = write(&dir, "flip.pxp", &b);
        if let Ok(mut ds) = IgorReader.open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.vendor_metadata();
            let _ = ds.read_trace(0, 0, 0, 100);
        }
    }
}

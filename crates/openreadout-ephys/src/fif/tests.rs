//! Synthetic-file tests of the FIF reader, with FIF files written here.

use super::*;

/// Writes FIF tags one after another (`next` = 0, no directory).
struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn new(version: i32) -> Self {
        let mut w = Writer { out: Vec::new() };
        let mut id = Vec::new();
        for v in [version, 1, 2, 1_600_000_000, 0] {
            id.extend_from_slice(&v.to_be_bytes());
        }
        w.tag(K_FILE_ID, T_ID_STRUCT, &id);
        w.int(K_DIR_POINTER, -1);
        w
    }

    fn tag(&mut self, kind: i32, ty: u32, data: &[u8]) {
        self.out.extend_from_slice(&kind.to_be_bytes());
        self.out.extend_from_slice(&ty.to_be_bytes());
        self.out
            .extend_from_slice(&(data.len() as i32).to_be_bytes());
        self.out.extend_from_slice(&0i32.to_be_bytes());
        self.out.extend_from_slice(data);
    }

    fn int(&mut self, kind: i32, v: i32) {
        self.tag(kind, T_INT, &v.to_be_bytes());
    }

    fn float(&mut self, kind: i32, v: f32) {
        self.tag(kind, T_FLOAT, &v.to_be_bytes());
    }

    fn text(&mut self, kind: i32, v: &str) {
        self.tag(kind, 10, v.as_bytes());
    }

    fn start(&mut self, block: i32) {
        self.int(K_BLOCK_START, block);
    }

    fn end(&mut self, block: i32) {
        self.int(K_BLOCK_END, block);
    }

    #[allow(clippy::too_many_arguments)]
    fn channel(
        &mut self,
        scan: i32,
        kind: i32,
        range: f32,
        cal: f32,
        coil: i32,
        unit: i32,
        name: &str,
    ) {
        let mut d = Vec::new();
        for v in [scan, scan, kind] {
            d.extend_from_slice(&v.to_be_bytes());
        }
        d.extend_from_slice(&range.to_be_bytes());
        d.extend_from_slice(&cal.to_be_bytes());
        d.extend_from_slice(&coil.to_be_bytes());
        for k in 0..12 {
            d.extend_from_slice(&(k as f32 * 0.01).to_be_bytes());
        }
        d.extend_from_slice(&unit.to_be_bytes());
        d.extend_from_slice(&0i32.to_be_bytes());
        let mut n = [0u8; 16];
        n[..name.len()].copy_from_slice(name.as_bytes());
        d.extend_from_slice(&n);
        self.tag(K_CH_INFO, T_CH_INFO_STRUCT, &d);
    }
}

/// A two-channel recording: info, then `buffers` (each a list of samples × 2 channels).
fn recording(ty: u32, first_sample: Option<i32>, buffers: &[Option<Vec<[i16; 2]>>]) -> Vec<u8> {
    let mut w = Writer::new(0x0001_0004);
    w.start(B_MEAS);
    w.start(B_MEAS_INFO);
    let mut date = Vec::new();
    date.extend_from_slice(&1_551_255_482i32.to_be_bytes());
    date.extend_from_slice(&250_000i32.to_be_bytes());
    w.tag(K_MEAS_DATE, T_INT, &date);
    w.int(K_NCHAN, 2);
    w.float(K_SFREQ, 200.0);
    w.float(K_LOWPASS, 100.0);
    w.channel(1, 2, 1.0, 1e-6, 1, 107, "EEG 001");
    w.channel(2, 3, 1.0, 1.0, 0, 0, "STI 014");
    w.start(B_MNE_BAD_CHANNELS);
    w.text(K_MNE_CH_NAME_LIST, "EEG 001");
    w.end(B_MNE_BAD_CHANNELS);
    w.end(B_MEAS_INFO);
    w.start(B_RAW);
    if let Some(s) = first_sample {
        w.int(K_FIRST_SAMPLE, s);
    }
    for b in buffers {
        match b {
            None => w.int(K_DATA_SKIP, 1),
            Some(rows) => {
                let mut d = Vec::new();
                for r in rows {
                    for v in r {
                        match ty {
                            T_FLOAT => d.extend_from_slice(&f32::from(*v).to_be_bytes()),
                            T_INT => d.extend_from_slice(&i32::from(*v).to_be_bytes()),
                            _ => d.extend_from_slice(&v.to_be_bytes()),
                        }
                    }
                }
                w.tag(K_DATA_BUFFER, ty, &d);
            }
        }
    }
    w.end(B_RAW);
    w.end(B_MEAS);
    w.out
}

fn open(bytes: Vec<u8>, name: &str) -> (tempfile::TempDir, FifDataset) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    let ds = FifDataset::open(&p).unwrap();
    (dir, ds)
}

#[test]
fn detects_by_the_file_id_tag() {
    let b = recording(T_SHORT, None, &[Some(vec![[1, 2]])]);
    assert!(looks_like_fif(&b));
    assert!(!looks_like_fif(b"\0\0\0\x64\0\0\0\x1f\0\0\0\x15"));
    let d = FifReader.sniff(&b, Path::new("x.dat")).unwrap();
    assert_eq!(d.confidence, DetectConfidence::Definite);
}

#[test]
fn reads_int16_buffers_with_range_times_cal() {
    let rows: Vec<[i16; 2]> = (0..4).map(|i| [i * 100, i]).collect();
    let (_d, mut ds) = open(
        recording(T_DAU_PACK16, Some(2000), &[Some(rows.clone()), Some(rows)]),
        "a_raw.fif",
    );
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("1.4"));
    let t = &info.traces[0];
    assert_eq!(t.sample_rate_hz, 200.0);
    assert_eq!(t.sample_count, 8);
    assert_eq!(t.sweep_count, 1);
    assert_eq!(t.start_s, Some(10.0));
    assert_eq!(t.channels[0].name, "EEG 001");
    assert_eq!(t.channels[0].unit.as_deref(), Some("V"));
    assert_eq!(t.channels[0].extra["kind"], "eeg");
    assert_eq!(t.channels[0].extra["bad"], true);
    assert_eq!(t.channels[1].unit.as_deref(), Some(""));
    assert_eq!(t.extra["recorded_at"], "2019-02-27T08:18:02.250000Z");
    let tr = ds.read_trace(0, 0, 3, 3).unwrap();
    let cal = f64::from(1e-6f32);
    assert_eq!(tr.channels[0], vec![300.0 * cal, 0.0, 100.0 * cal]);
    assert_eq!(tr.channels[1], vec![3.0, 0.0, 1.0]);
}

#[test]
fn float_and_int32_buffers() {
    for ty in [T_FLOAT, T_INT] {
        let (_d, mut ds) = open(
            recording(ty, None, &[Some(vec![[5, -7], [6, 8]])]),
            "b_raw.fif",
        );
        let tr = ds.read_trace(0, 0, 0, 10).unwrap();
        assert_eq!(tr.channels[1], vec![-7.0, 8.0]);
        let dtype = &ds.info().unwrap().traces[0].channels[0].dtype;
        assert_eq!(dtype, if ty == T_FLOAT { "float32" } else { "int32" });
    }
}

#[test]
fn skips_split_sweeps() {
    let rows: Vec<[i16; 2]> = (0..3).map(|i| [i, i]).collect();
    let (_d, mut ds) = open(
        recording(
            T_SHORT,
            Some(10),
            &[Some(rows.clone()), None, Some(rows.clone()), Some(rows)],
        ),
        "c_raw.fif",
    );
    let f = ds.fif();
    assert_eq!(f.sweeps.len(), 2);
    assert_eq!(f.sweeps[1].first, 10 + 3 + 3);
    assert_eq!(f.sweeps[1].samples, 6);
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].sweep_count, 2);
    assert_eq!(info.traces[0].extra["sweep_starts_s"], json!([0.0, 0.03]));
    let tr = ds.read_trace(0, 1, 2, 10).unwrap();
    assert_eq!(tr.channels[1], vec![2.0, 0.0, 1.0, 2.0]);
}

#[test]
fn an_initial_skip_moves_the_first_sample() {
    let rows: Vec<[i16; 2]> = (0..5).map(|i| [i, i]).collect();
    let (_d, ds) = open(recording(T_SHORT, None, &[None, Some(rows)]), "d_raw.fif");
    assert_eq!(ds.fif().first_sample, 5);
    assert_eq!(ds.fif().sweeps.len(), 1);
}

#[test]
fn annotations_count_from_the_first_sample() {
    let mut b = recording(T_SHORT, Some(200), &[Some(vec![[0, 0]; 4])]);
    // append an annotation block after the measurement
    let mut w = Writer { out: Vec::new() };
    w.start(B_ANNOTATIONS);
    let onsets: Vec<u8> = [1.5f32, 2.0].iter().flat_map(|v| v.to_be_bytes()).collect();
    let ends: Vec<u8> = [2.0f32, 2.0].iter().flat_map(|v| v.to_be_bytes()).collect();
    w.tag(K_ANNOT_ONSET, T_FLOAT, &onsets);
    w.tag(K_ANNOT_END, T_FLOAT, &ends);
    w.text(K_COMMENT, "BAD_x:stim\\:1");
    w.end(B_ANNOTATIONS);
    b.extend_from_slice(&w.out);
    let (_d, mut ds) = open(b, "e_raw.fif");
    let a = &ds.fif().annotations;
    assert_eq!(a.len(), 2);
    // no origin in the block, a measurement date in the info: onsets count from the first sample
    assert_eq!(a[0].onset_s, 1.5);
    assert_eq!(a[0].duration_s, 0.5);
    assert_eq!(a[1].text, "stim:1");
    let t = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(t.columns[2], vec![0.0, 1.0]);
}

#[test]
fn name_lists_unescape_colons() {
    assert_eq!(name_list("a:b\\:c:"), vec!["a", "b:c", ""]);
    assert!(name_list("").is_empty());
}

#[test]
fn refuses_files_without_raw_data() {
    let mut w = Writer::new(0x0001_0004);
    w.start(B_MEAS);
    w.start(B_MEAS_INFO);
    w.int(K_NCHAN, 1);
    w.float(K_SFREQ, 100.0);
    w.channel(1, 2, 1.0, 1.0, 1, 107, "EEG");
    w.end(B_MEAS_INFO);
    w.start(B_MNE_EPOCHS);
    w.end(B_MNE_EPOCHS);
    w.end(B_MEAS);
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x-epo.fif");
    std::fs::write(&p, w.out).unwrap();
    let e = FifDataset::open(&p).unwrap_err();
    assert!(e.to_string().contains("epochs"), "{e}");
}

#[test]
fn malformed_files_give_errors_not_panics() {
    let good = recording(T_SHORT, Some(0), &[Some(vec![[1, 2]; 8])]);
    for cut in [0, 5, 16, 36, 60, 100, good.len() / 2, good.len() - 3] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m_raw.fif");
        std::fs::write(&p, &good[..cut]).unwrap();
        if let Ok(mut ds) = FifDataset::open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 100);
        }
    }
    // a negative size and a block-start without data
    let mut bad = good.clone();
    bad[44..48].copy_from_slice(&(-5i32).to_be_bytes());
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("n_raw.fif");
    std::fs::write(&p, &bad).unwrap();
    let _ = FifDataset::open(&p);
}

#[test]
fn follows_split_parts() {
    let rows: Vec<[i16; 2]> = (0..3).map(|i| [i, i]).collect();
    let dir = tempfile::tempdir().unwrap();
    let mut first = recording(T_SHORT, Some(0), &[Some(rows.clone())]);
    let mut w = Writer { out: Vec::new() };
    w.start(B_REF);
    w.int(K_REF_ROLE, 2);
    w.int(K_REF_FILE_NUM, 1);
    w.end(B_REF);
    first.extend_from_slice(&w.out);
    std::fs::write(dir.path().join("rec_raw.fif"), first).unwrap();
    std::fs::write(
        dir.path().join("rec_raw-1.fif"),
        recording(T_SHORT, Some(3), &[Some(rows)]),
    )
    .unwrap();
    let mut ds = FifDataset::open(&dir.path().join("rec_raw.fif")).unwrap();
    assert_eq!(ds.fif().parts.len(), 2);
    assert_eq!(ds.member_files().len(), 2);
    let tr = ds.read_trace(0, 0, 0, 100).unwrap();
    assert_eq!(tr.channels[1], vec![0.0, 1.0, 2.0, 0.0, 1.0, 2.0]);
}

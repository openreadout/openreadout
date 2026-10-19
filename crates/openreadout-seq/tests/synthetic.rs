//! The readers on the committed synthetic fixtures (`oracle/make_seq_fixtures.py`): tiny Illumina
//! run folders with BCL and CBCL base calls, FASTQ, BAM, CRAM, POD5 and FAST5, with values known
//! by construction.
#![allow(clippy::float_cmp, clippy::many_single_char_names)] // values known exactly by construction

use std::path::{Path, PathBuf};

use openreadout_core::source::Input;
use openreadout_core::{Dataset, FormatReader};
use openreadout_seq::{
    BamReader, CramReader, Fast5Reader, FastqOptions, FastqReader, IlluminaRun, IlluminaRunReader,
    Pod5Reader, export_fastq,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn column(ds: &mut dyn Dataset, table: u32, name: &str) -> Vec<f64> {
    let info = ds.info().unwrap();
    let t = &info.tables[table as usize];
    let c = t.columns.iter().position(|c| c.name == name).unwrap();
    ds.read_table(table, 0, t.row_count).unwrap().columns[c].clone()
}

#[test]
fn bcl_run_folder() {
    let p = fixture("run-bcl");
    let r = IlluminaRunReader;
    assert!(r.sniff(&[], &p).is_some());
    assert!(r.sniff(&[], &p.join("RunInfo.xml")).is_some());
    let mut ds = r.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format.id, "illumina-run");
    let s = &info.tables[0];
    assert_eq!(s.extra["totals"]["read_structure"], "2T");
    assert_eq!(s.extra["instrument_model"], "MiSeq");
    // Q histogram per tile and cycle: 1 call at Q30 and 2 at Q31; the last cycle of a read is
    // left out of the read's totals
    assert_eq!(column(&mut *ds, 0, "percent_q30"), vec![100.0]);
    assert_eq!(column(&mut *ds, 0, "reads_pf"), vec![3e-6]);
    assert!((column(&mut *ds, 0, "phasing")[0] - 0.1).abs() < 1e-6);
    assert_eq!(
        column(&mut *ds, 1, "mean_qscore")[0],
        (30.0 + 2.0 * 31.0) / 3.0
    );
    // base calls table: counted from the filter file
    assert_eq!(column(&mut *ds, 5, "clusters"), vec![4.0]);
    assert_eq!(column(&mut *ds, 5, "clusters_pf"), vec![3.0]);
    let tr = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(tr.channels[0].len(), 2);
    let c = ds.check().unwrap();
    assert!(c.ok, "{:?}", c.findings);
    let e = ds.experiment().unwrap();
    assert_eq!(e.instrument.unwrap().serial.as_deref(), Some("M00001"));
}

#[test]
fn bcl_fastq_export() {
    let run =
        IlluminaRun::open(&openreadout_core::source::Input::local(fixture("run-bcl"))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let r = export_fastq(&run, dir.path(), &FastqOptions::default()).unwrap();
    assert!(r.verified);
    assert_eq!(r.clusters, 3);
    assert_eq!(r.filtered_out, 1);
    let f = &r.files[0];
    let text = String::from_utf8(
        std::io::Read::bytes(flate2::read::GzDecoder::new(
            std::fs::File::open(&f.path).unwrap(),
        ))
        .map(Result::unwrap)
        .collect(),
    )
    .unwrap();
    // cluster 0: A Q30, G Q31; cluster 1: C Q35, C Q36; cluster 3: T Q20, then a no-call
    let expected = "@M00001:1:000000000-AAAAA:1:1101:1100:1110 1:N:0:\nAG\n+\n?@\n\
                    @M00001:1:000000000-AAAAA:1:1101:1205:1213 1:N:0:\nCC\n+\nDE\n\
                    @M00001:1:000000000-AAAAA:1:1101:15190:1020 1:N:0:\nTN\n+\n5#\n";
    assert_eq!(text, expected);
}

#[test]
fn cbcl_run_folder() {
    let p = fixture("run-cbcl");
    let mut ds = IlluminaRunReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.tables[0].extra["tile_area_mm2"], 1.38);
    assert_eq!(column(&mut *ds, 0, "reads"), vec![4e-6]);
    let c = ds.check().unwrap();
    assert!(c.ok, "{:?}", c.findings);
    let run = IlluminaRun::open(&openreadout_core::source::Input::local(&p)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let r = export_fastq(&run, dir.path(), &FastqOptions::default()).unwrap();
    assert_eq!(r.clusters, 3);
    let text = String::from_utf8(
        std::io::Read::bytes(flate2::read::GzDecoder::new(
            std::fs::File::open(&r.files[0].path).unwrap(),
        ))
        .map(Result::unwrap)
        .collect(),
    )
    .unwrap();
    // cycle 1 nibbles 0x9D 0x0E → clusters D(bin 3 C), 9(bin 2 C), E(bin 3 G), 0 (no call);
    // cycle 2 nibbles 0xE1 0x09 → 1(bin 0 C), E(bin 3 G), 9(bin 2 C), 0
    let recs: Vec<&str> = text.lines().collect();
    assert_eq!(recs[1], "CC", "{text}");
    assert_eq!(recs[3], "F!", "{text}");
    assert_eq!(recs[5], "GC", "{text}");
    assert_eq!(recs[9], "NN", "{text}");
}

#[test]
fn fastq_plain_and_gzip() {
    for name in ["mini.fastq", "mini.fastq.gz"] {
        let p = fixture(name);
        let head = std::fs::read(&p).unwrap();
        assert!(FastqReader.sniff(&head, &p).is_some(), "{name}");
        let mut ds = FastqReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        let s = &info.tables[0].extra["summary"];
        assert_eq!(s["reads"], 2, "{name}");
        assert_eq!(s["bases"], 18);
        assert_eq!(info.tables[0].extra["read_count_exact"], true);
        assert_eq!(column(&mut *ds, 0, "length"), vec![10.0, 8.0]);
        assert_eq!(column(&mut *ds, 0, "n_bases"), vec![1.0, 0.0]);
        let c = ds.check().unwrap();
        assert!(c.ok);
    }
}

#[test]
fn bam_records_and_tags() {
    let p = fixture("mini.bam");
    let head = std::fs::read(&p).unwrap();
    assert!(BamReader.sniff(&head, &p).is_some());
    assert!(FastqReader.sniff(&head, &p).is_none());
    let mut ds = BamReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let e = &info.tables[0].extra;
    assert_eq!(e["platform"], "PACBIO");
    assert_eq!(e["summary"]["reads"], 2);
    assert_eq!(column(&mut *ds, 0, "passes"), vec![5.0, 5.0]);
    let pos = column(&mut *ds, 0, "pos");
    assert!(pos[0] == 10.0 && pos[1].is_nan(), "{pos:?}");
    let q = column(&mut *ds, 0, "mean_quality");
    assert!(
        (q[0] - 30.0).abs() < 1e-9 && (q[1] - 31.0).abs() < 1e-9,
        "{q:?}"
    );
}

#[test]
fn cram_with_and_without_its_reference() {
    let p = fixture("mini.cram");
    let head = std::fs::read(&p).unwrap();
    assert!(CramReader.sniff(&head, &p).is_some());
    assert!(BamReader.sniff(&head, &p).is_none());
    let mut ds = CramReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("3.0"));
    let e = &info.tables[0].extra;
    assert_eq!(e["summary"]["records"], 3);
    assert_eq!(e["summary"]["mapped"], 2);
    assert_eq!(e["platform"], "PACBIO");
    assert_eq!(e["reference"]["source"], "mini.fa");
    assert!(info.notes.is_empty(), "{:?}", info.notes);
    assert_eq!(column(&mut *ds, 0, "length"), vec![23.0, 30.0, 8.0]);
    assert_eq!(column(&mut *ds, 0, "flag"), vec![0.0, 16.0, 4.0]);
    assert_eq!(column(&mut *ds, 0, "mapq"), vec![60.0, 30.0, 0.0]);
    let pos = column(&mut *ds, 0, "pos");
    assert!(
        pos[0] == 1.0 && pos[1] == 21.0 && pos[2].is_nan(),
        "{pos:?}"
    );
    assert_eq!(column(&mut *ds, 0, "passes")[1], 7.0);
    // GGACGTCCGTTTGGAAACCAAGG: 13 G or C of 23; the 30 reference bases from position 21: 17 of 30
    let gc = column(&mut *ds, 0, "gc_percent");
    assert!((gc[0] - 13.0 / 23.0 * 100.0).abs() < 1e-9, "{gc:?}");
    assert!((gc[1] - 17.0 / 30.0 * 100.0).abs() < 1e-9, "{gc:?}");
    assert!((gc[2] - 50.0).abs() < 1e-9, "{gc:?}");
    assert_eq!(column(&mut *ds, 0, "n_bases")[2], 4.0);

    // the same bytes without the FASTA beside them: no bases for the aligned reads
    let input = Input::from_bytes("alone.cram", head);
    let mut ds = CramReader.open_input(&input).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.tables[0].extra["summary"]["reads_without_bases"], 2);
    assert!(
        info.notes.iter().any(|n| n.contains("were not restored")),
        "{:?}",
        info.notes
    );
    let gc = column(&mut *ds, 0, "gc_percent");
    assert!(
        gc[0].is_nan() && gc[1].is_nan() && (gc[2] - 50.0).abs() < 1e-9,
        "{gc:?}"
    );
    assert_eq!(column(&mut *ds, 0, "length"), vec![23.0, 30.0, 8.0]);
    let rep = ds.check().unwrap();
    assert!(rep.findings.iter().any(|f| f.code == "reference_missing"));
}

#[test]
fn cram_damage_is_an_error_not_a_panic() {
    let good = std::fs::read(fixture("mini.cram")).unwrap();
    for cut in (27..good.len()).step_by(7) {
        let input = Input::from_bytes("cut.cram", good[..cut].to_vec());
        if let Ok(mut ds) = CramReader.open_input(&input) {
            let _ = ds.info();
            let _ = ds.check();
        }
    }
    for i in (26..good.len()).step_by(3) {
        let mut b = good.clone();
        b[i] ^= 0x5a;
        let input = Input::from_bytes("flip.cram", b);
        if let Ok(mut ds) = CramReader.open_input(&input) {
            let _ = ds.info();
            if let Ok(info) = ds.info() {
                let _ = ds.read_table(0, 0, info.tables[0].row_count);
            }
            let _ = ds.check();
        }
    }
}

#[test]
fn pod5_signal_in_picoamperes() {
    let p = fixture("mini.pod5");
    if !p.exists() {
        return;
    }
    let mut ds = Pod5Reader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].sweep_count, 1);
    assert_eq!(info.traces[0].sample_rate_hz, 4000.0);
    let tr = ds.read_trace(0, 0, 0, 64).unwrap();
    let want: Vec<f64> = (0..64)
        .map(|i| (f64::from(i * 3) + 10.0) * f64::from(0.2f32))
        .collect();
    assert_eq!(tr.channels[0].len(), 64);
    for (a, b) in tr.channels[0].iter().zip(&want) {
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }
    assert!(ds.check().unwrap().ok);
}

#[test]
fn fast5_signal_in_picoamperes() {
    let p = fixture("mini.fast5");
    let mut ds = Fast5Reader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.tables[0].row_count, 1);
    let tr = ds.read_trace(0, 0, 0, 40).unwrap();
    for (i, v) in tr.channels[0].iter().enumerate() {
        let raw = f64::from(((i as i16) * 5) % 211);
        assert!((v - (raw + 10.0) * 1400.0 / 8192.0).abs() < 1e-9);
    }
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.unwrap().id.as_deref(), Some("synthetic"));
}

#[test]
fn malformed_inputs_are_errors_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    for (name, bytes) in [
        ("x.pod5", b"\x8BPOD\r\n\x1A\nshort".to_vec()),
        ("x.bam", vec![0x1f, 0x8b, 8, 4, 0, 0, 0, 0, 0, 0xff]),
        ("x.fastq", b"@r\nACGT\n+\nII\n".to_vec()),
        ("x.fast5", b"\x89HDF\r\n\x1a\n".to_vec()),
    ] {
        let p = dir.path().join(name);
        std::fs::write(&p, &bytes).unwrap();
        let r: &dyn FormatReader = match name {
            "x.pod5" => &Pod5Reader,
            "x.bam" => &BamReader,
            "x.fastq" => &FastqReader,
            _ => &Fast5Reader,
        };
        if let Ok(mut ds) = r.open(&p) {
            let _ = ds.info();
            let _ = ds.check();
        }
    }
    // a run folder whose InterOp files are cut short
    let run = dir.path().join("run");
    std::fs::create_dir_all(run.join("InterOp")).unwrap();
    std::fs::copy(fixture("run-bcl/RunInfo.xml"), run.join("RunInfo.xml")).unwrap();
    let q = std::fs::read(fixture("run-bcl/InterOp/QMetricsOut.bin")).unwrap();
    std::fs::write(run.join("InterOp/QMetricsOut.bin"), &q[..q.len() - 7]).unwrap();
    let ds = IlluminaRunReader.open(&run).unwrap();
    let info = ds.info().unwrap();
    assert!(
        info.notes.iter().any(|n| n.contains("partial record")),
        "{:?}",
        info.notes
    );
}

//! Synthetic SpikeGLX streams: .meta parsing, saved subsets, scaling, truncation, detection.

use std::path::PathBuf;

use openreadout_core::Error;
use openreadout_core::reader::FormatReader;
use openreadout_spikeglx::SpikeGlxReader;

fn stream(meta: &str, samples: &[i16]) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("run_g0_t0.imec0.ap.bin");
    std::fs::write(
        d.path().join("run_g0_t0.imec0.ap.meta"),
        meta.replace('\n', "\r\n"),
    )
    .unwrap();
    let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(&bin, bytes).unwrap();
    (d, bin)
}

const NP1: &str = "typeThis=imec\nnSavedChans=3\nfileSizeBytes=18\nimSampRate=30000\nfirstSample=300\nacqApLfSy=2,2,1\nsnsApLfSy=2,0,1\nsnsSaveChanSubset=0:1,4\nimDatPrb_type=0\nimAiRangeMax=0.6\nimAiRangeMin=-0.6\n~imroTbl=(0,2)(0 0 0 500 250 1)(1 0 0 1000 250 1)\n~snsChanMap=(2,0,1)(AP0;0:0)(AP1;1:1)(SY0;4:4)\nappVersion=20201103\nfileCreateTime=2022-05-19T17:39:28\n";

#[test]
#[allow(clippy::float_cmp)]
fn np1_stream() {
    let (_d, bin) = stream(NP1, &[10, 20, 64, -10, -20, 1, 5, 5, 0]);
    let mut ds = SpikeGlxReader.open(&bin).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("imec0.ap"));
    assert_eq!(t.sample_count, 3);
    assert_eq!(t.start_s, Some(0.01));
    assert_eq!(t.channels[2].name, "SY0");
    assert_eq!(t.channels[2].unit, None);
    let tr = ds.read_trace(0, 0, 1, 10).unwrap();
    assert_eq!(tr.channels[0].len(), 2);
    assert!((tr.channels[0][0] - (-10.0) * 0.6 / 512.0 / 500.0 * 1e6).abs() < 1e-9);
    assert!((tr.channels[1][0] - (-20.0) * 0.6 / 512.0 / 1000.0 * 1e6).abs() < 1e-9);
    assert_eq!(tr.channels[2][0], 1.0);
    assert!(ds.check().unwrap().ok);
    // opening the .meta opens the same stream
    let meta = bin.with_extension("meta");
    assert_eq!(
        SpikeGlxReader.open(&meta).unwrap().info().unwrap().traces[0].sample_count,
        3
    );
}

#[test]
fn truncated_bin_is_reported() {
    let (_d, bin) = stream(
        &NP1.replace("fileSizeBytes=18", "fileSizeBytes=600"),
        &[1, 2, 3, 4, 5, 6],
    );
    let mut ds = SpikeGlxReader.open(&bin).unwrap();
    assert_eq!(ds.info().unwrap().traces[0].sample_count, 2);
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "truncated"));
}

#[test]
fn missing_meta_is_unsupported() {
    let d = tempfile::tempdir().unwrap();
    let bin = d.path().join("x.bin");
    std::fs::write(&bin, [0u8; 8]).unwrap();
    assert!(SpikeGlxReader.sniff(&[0u8; 8], &bin).is_none());
    match SpikeGlxReader.open(&bin) {
        Err(Error::Unsupported { .. }) => {}
        Err(e) => panic!("unexpected {e}"),
        Ok(_) => panic!("opened a .bin without .meta"),
    }
}

#[test]
fn bad_subset_is_corrupt() {
    let (_d, bin) = stream(
        &NP1.replace("snsSaveChanSubset=0:1,4", "snsSaveChanSubset=0:3"),
        &[0; 9],
    );
    match SpikeGlxReader.open(&bin) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        Err(e) => panic!("unexpected {e}"),
        Ok(_) => panic!("accepted a subset that disagrees with nSavedChans"),
    }
}

#[test]
fn detection_uses_the_meta() {
    let (_d, bin) = stream(NP1, &[0; 9]);
    assert!(SpikeGlxReader.sniff(&[0u8; 16], &bin).is_some());
}

#[test]
fn run_directories_include_probe_subdirectories_only() {
    let root = tempfile::tempdir().unwrap();
    let run = root.path().join("rec_g0");
    let probe = run.join("rec_g0_imec0");
    let other = run.join("kilosort");
    std::fs::create_dir_all(&probe).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        probe.join("rec_g0_t0.imec0.ap.meta"),
        NP1.replace('\n', "\r\n"),
    )
    .unwrap();
    std::fs::write(probe.join("rec_g0_t0.imec0.ap.bin"), [0u8; 18]).unwrap();
    std::fs::write(other.join("spike_times.npy"), b"x").unwrap();
    std::fs::write(run.join("notes.txt"), b"x").unwrap();
    let files = openreadout_spikeglx::session_files(&run).unwrap();
    assert_eq!(files, [probe.join("rec_g0_t0.imec0.ap.bin")]);
    assert!(SpikeGlxReader.sniff(&[], &run).is_some());
    let info = SpikeGlxReader.open(&run).unwrap().info().unwrap();
    assert_eq!(info.traces.len(), 1);
    // a directory that only wraps the run is not a run itself
    assert!(openreadout_spikeglx::session_files(root.path()).is_none());
    // nor is a run directory holding a foreign file
    std::fs::write(run.join("video.avi"), b"x").unwrap();
    assert!(openreadout_spikeglx::session_files(&run).is_none());
}

#[test]
fn memory_run_discovers_probe_subdirectories() {
    use openreadout_core::source::{Fs, Input, MemFs, MemSource};
    use std::sync::Arc;
    let fs = MemFs::new()
        .with(
            "memory/rec_imec0/rec.ap.meta",
            Arc::new(MemSource::new("rec.ap.meta", NP1)),
        )
        .with(
            "memory/rec_imec0/rec.ap.bin",
            Arc::new(MemSource::new("rec.ap.bin", [0u8; 18])),
        );
    let input = Input::new("memory", Fs::new(Arc::new(fs)));
    assert!(SpikeGlxReader.sniff_input(&[], &input).is_some());
    let mut ds = SpikeGlxReader.open_input(&input).unwrap();
    assert_eq!(ds.info().unwrap().traces.len(), 1);
    assert!(!ds.read_trace(0, 0, 0, 2).unwrap().channels.is_empty());
    assert!(ds.check().unwrap().ok);
}

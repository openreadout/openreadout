//! Memory of an MRM `.wiff` whose cycles hold hundreds of precursors (docs/architecture-memory.md).
//!
//! Each MRM cycle yields one spectrum per precursor, so a 14.5 MB multi-sample file can list
//! 30 million spectra. The reader keeps one entry per index record and works out a spectrum's
//! transitions when it reads the spectrum, so memory grows with the cycles, not with
//! cycles x precursors. The heap high-water mark comes from a counting global allocator.

use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;
use openreadout_sciex::SciexWiffReader;
use peak_alloc::PeakAlloc;

mod common;
use common::{
    compound_file, experiment_header, f32s, index, mass_ranges, sample_streams, scan_file,
};

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

/// Transitions per cycle, each with its own Q1: every cycle holds this many spectra.
const TRANSITIONS: usize = 800;

/// An unscheduled MRM acquisition of `TRANSITIONS` transitions and `cycles` cycles, in `dir`.
fn write_mrm(dir: &Path, cycles: usize) -> PathBuf {
    let base = "MethodSubtree/Method1/DeviceMethod0/Period0/Experiment0";
    let ranges: Vec<(f32, f32, f32, &str, f32)> = (0..TRANSITIONS)
        .map(|k| (100.0 + k as f32, 50.0 + k as f32, 1.0, "T", 20.0))
        .collect();
    // a cycle: TRANSITIONS leading zeros, then one value per transition
    let mut row = vec![-(TRANSITIONS as f32) - 0.01];
    row.extend((0..TRANSITIONS).map(|k| k as f32 + 1.0));
    let chunks: Vec<Vec<u8>> = (0..cycles).map(|_| f32s(&row)).collect();
    let (scan, spans) = scan_file(&chunks);
    let records: Vec<(u32, u32, f64, f64, f64, f64)> = spans
        .iter()
        .enumerate()
        .map(|(c, &(off, len))| (off, len, 1000.0 * (c + 1) as f64, 1.0, 0.0, 0.0))
        .collect();
    let mut streams = sample_streams();
    let owned = [
        (
            format!("{base}/ExperimentHeader"),
            experiment_header(4, 0, TRANSITIONS as u32),
        ),
        (
            format!("{base}/MassRangeEx/MassRangeEx"),
            mass_ranges(&ranges),
        ),
    ];
    for (p, b) in &owned {
        streams.push((p.as_str(), b.clone()));
    }
    streams.push(("SampleSubtree/Sample1/Idx", index(&records)));
    let wiff = dir.join(format!("mrm{cycles}.wiff"));
    std::fs::write(&wiff, compound_file(&streams)).unwrap();
    std::fs::write(dir.join(format!("mrm{cycles}.wiff.scan")), scan).unwrap();
    wiff
}

/// Peak heap while opening `wiff` and summarising it.
fn open_peak(wiff: &Path) -> usize {
    PEAK.reset_peak_usage();
    let base = PEAK.current_usage();
    let ds = SciexWiffReader.open(wiff).unwrap();
    let info = ds.info().unwrap();
    let peak = PEAK.peak_usage().saturating_sub(base);
    assert_eq!(
        info.spectra[0].scan_count,
        (TRANSITIONS * cycles_of(wiff)) as u64
    );
    peak
}

fn cycles_of(wiff: &Path) -> usize {
    let stem = wiff.file_stem().unwrap().to_string_lossy();
    stem.trim_start_matches("mrm").parse().unwrap()
}

#[test]
fn memory_follows_the_cycles_not_the_spectra() {
    let tmp = tempfile::tempdir().unwrap();
    let small = write_mrm(tmp.path(), 20);
    let large = write_mrm(tmp.path(), 100);
    // 16,000 and 80,000 spectra. A table of the spectra (~40 bytes each) would grow by over
    // 2 MiB; one entry per cycle grows by a few kilobytes.
    let (a, b) = (open_peak(&small), open_peak(&large));
    assert!(
        b.saturating_sub(a) < 256 << 10,
        "opening 80 more cycles of {TRANSITIONS} spectra took {} more bytes ({a} -> {b})",
        b.saturating_sub(a)
    );
    // Reading every spectrum in order holds one cycle at a time.
    let mut ds = SciexWiffReader.open(&large).unwrap();
    PEAK.reset_peak_usage();
    let base = PEAK.current_usage();
    let mut sum = 0.0f64;
    for i in 0..(TRANSITIONS * 100) as u64 {
        let sp = ds.read_spectrum(0, i).unwrap();
        sum += f64::from(sp.intensity[0]);
    }
    let read = PEAK.peak_usage().saturating_sub(base);
    // each cycle's values 1..=TRANSITIONS, once per cycle
    let per_cycle = (TRANSITIONS * (TRANSITIONS + 1) / 2) as f64;
    assert!((sum - per_cycle * 100.0).abs() < 1e-6, "sum {sum}");
    assert!(read < 1 << 20, "reading the spectra peaked at {read} bytes");
}

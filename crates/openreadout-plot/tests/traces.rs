//! Traces of several channels from a synthetic reader: EEG-like channels faceted one per small
//! plot, and a short frequency sweep drawn on log axes against a channel picked for x.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo, Table,
    Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::{Error, Registry, Result};
use openreadout_plot::{parse_spec, plot};
use serde_json::json;

/// A trace: its name, rate, channels (name, unit) and samples per channel.
#[derive(Clone)]
struct Synth {
    name: &'static str,
    rate: f64,
    channels: Vec<(&'static str, Option<&'static str>)>,
    samples: Vec<Vec<f64>>,
}

fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: "synth-trace".into(),
        name: "Synthetic trace".into(),
        vendor: "test".into(),
        extensions: vec!["syn".into()],
        family: "electrophysiology".into(),
        can_read: true,
        can_write: false,
        confidence: Confidence::High,
        known_gaps: vec![],
    }
}

impl Dataset for Synth {
    fn info(&self) -> Result<FileInfo> {
        let channels = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, (n, u))| SignalChannelInfo {
                index: i as u32,
                name: (*n).into(),
                unit: u.map(str::to_string),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            })
            .collect();
        Ok(FileInfo {
            path: "run.syn".into(),
            size_bytes: 1,
            format: descriptor(),
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![TraceInfo {
                index: 0,
                name: Some(self.name.into()),
                sample_rate_hz: self.rate,
                sample_count: self.samples[0].len() as u64,
                sweep_count: 1,
                channels,
                start_s: None,
                extra: BTreeMap::new(),
            }],
            plane_count: 0,
            notes: vec![],
        })
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
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synth-trace", "run.syn"))
    }
    fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage("no images".into()))
    }
    fn read_table(&mut self, _: u32, _: u64, _: u64) -> Result<Table> {
        Err(Error::Usage("no tables".into()))
    }
    fn read_trace(&mut self, _: u32, _: u32, first: u64, max: u64) -> Result<Trace> {
        let channels = self
            .samples
            .iter()
            .map(|c| {
                c.iter()
                    .skip(first as usize)
                    .take(max as usize)
                    .copied()
                    .collect()
            })
            .collect();
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample: first,
            channels,
        })
    }
}

struct Reader(Synth);

impl FormatReader for Reader {
    fn descriptor(&self) -> FormatDescriptor {
        descriptor()
    }
    fn sniff(&self, head: &[u8], _: &Path) -> Option<Detection> {
        head.starts_with(b"SYNTH-TRACE").then_some(Detection {
            format_id: "synth-trace",
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }
    fn open(&self, _: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(self.0.clone()))
    }
}

fn setup(t: Synth) -> (tempfile::TempDir, String, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("run.syn");
    std::fs::write(&p, b"SYNTH-TRACE").unwrap();
    let mut reg = Registry::new();
    reg.register(Box::new(Reader(t)));
    let f = p.display().to_string();
    (dir, f, reg)
}

/// Three EEG-like channels: a few µV of signal on DC offsets far apart, as an amplifier that
/// stores raw values without high-pass filtering records them.
fn eeg() -> Synth {
    let n = 500;
    let wave = |offset: f64, f: f64| -> Vec<f64> {
        (0..n)
            .map(|i| offset + 20.0 * (i as f64 * f).sin())
            .collect()
    };
    Synth {
        name: "signals",
        rate: 250.0,
        channels: vec![("Fp1", Some("µV")), ("Fp2", Some("µV")), ("Cz", Some("µV"))],
        samples: vec![
            wave(400_000.0, 0.1),
            wave(1_100_000.0, 0.13),
            wave(150_000.0, 0.07),
        ],
    }
}

#[test]
fn faceted_channels_get_one_plot_each_with_their_own_y_range() {
    let (_dir, f, reg) = setup(eeg());
    let data: Vec<_> = (0..3).map(|c| json!({"file": f, "channel": c})).collect();
    let spec = parse_spec(json!({"data": data, "facet": "series", "columns": 1})).unwrap();
    let p = plot(&reg, &spec).unwrap();
    let panel = &p.output.panels[0];
    // one small plot per channel, named by the channel
    assert_eq!(panel.facets, Some(3));
    let names: Vec<&str> = panel.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Fp1", "Fp2", "Cz"]);
    // each on a range that fits its own wave, not one range for all three offsets
    assert_eq!(panel.y.range, None);
    assert_eq!(panel.y.facet_ranges.len(), 3);
    for (r, s) in panel.y.facet_ranges.iter().zip(&panel.series) {
        let [lo, hi] = s.y_range.unwrap();
        assert!(r[0] <= lo && r[1] >= hi, "{r:?} does not hold {lo}–{hi}");
        assert!(
            r[1] - r[0] < 200.0,
            "facet range {r:?} is too wide for a 40 µV wave"
        );
    }

    // share y keeps one range for all of them
    let spec = parse_spec(json!({"data": data, "facet": "series", "share": "y"})).unwrap();
    let p = plot(&reg, &spec).unwrap();
    let y = &p.output.panels[0].y;
    assert!(y.facet_ranges.is_empty());
    let [lo, hi] = y.range.unwrap();
    assert!(lo <= 150_000.0 && hi >= 1_100_000.0, "{lo}–{hi}");
}

#[test]
fn facets_at_one_level_share_y() {
    let mut t = eeg();
    t.samples = (0..3)
        .map(|c| {
            (0..500)
                .map(|i| 20.0 * (i as f64 * 0.1 + c as f64).sin())
                .collect()
        })
        .collect();
    let (_dir, f, reg) = setup(t);
    let data: Vec<_> = (0..3).map(|c| json!({"file": f, "channel": c})).collect();
    let spec = parse_spec(json!({"data": data, "facet": "series"})).unwrap();
    let p = plot(&reg, &spec).unwrap();
    let panel = &p.output.panels[0];
    assert_eq!(panel.facets, Some(3));
    assert!(panel.y.facet_ranges.is_empty());
    assert!(panel.y.range.is_some());
}

/// A rheometer frequency sweep: eleven steps, storage and loss modulus against step time and
/// angular frequency. The loss modulus starts at a few Pa and its last point is negative (the
/// instrument's inertia dominates the torque at the highest frequency).
fn sweep() -> Synth {
    let freq: Vec<f64> = (0..11)
        .map(|i| std::f64::consts::TAU * 10f64.powf(i as f64 / 5.0))
        .collect();
    Synth {
        name: "Frequency sweep - 1",
        rate: 0.0,
        channels: vec![
            ("step_time", Some("s")),
            ("angular_frequency", Some("rad/s")),
            ("storage_modulus", Some("Pa")),
            ("loss_modulus", Some("Pa")),
        ],
        samples: vec![
            (0..11).map(|i| 6.0 + 6.7 * i as f64).collect(),
            freq,
            vec![
                3186.0, 3194.0, 3187.0, 3251.0, 3468.0, 3903.0, 4865.0, 6901.0, 11_096.0, 31_515.0,
                133_557.0,
            ],
            vec![
                2.45, 13.0, 5.23, 0.94, 5.28, 22.1, 155.0, 418.0, 1668.0, 1809.0, -98_371.0,
            ],
        ],
    }
}

#[test]
fn a_short_series_with_a_negative_end_still_draws_on_a_log_axis() {
    let (_dir, f, reg) = setup(sweep());
    let spec = parse_spec(json!({
        "data": [
            {"file": f, "y": "storage_modulus", "x": "angular_frequency"},
            {"file": f, "y": "loss_modulus", "x": "angular_frequency"},
        ],
        "x": {"log": true},
        "y": {"log": true},
    }))
    .unwrap();
    let p = plot(&reg, &spec).unwrap();
    let panel = &p.output.panels[0];
    let names: Vec<&str> = panel.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["storage_modulus", "loss_modulus"]);
    // x is the angular frequency, not the step time
    assert_eq!(panel.x.label, "angular frequency (rad/s)");
    let [x0, x1] = panel.x.range.unwrap();
    assert!(x0 < 6.3 && x1 > 628.0, "x {x0}–{x1}");
    // the loss modulus is drawn, not blanked as noise below a floor
    let loss = panel.series[1]
        .y_range
        .expect("the loss modulus has no points drawn");
    assert!(loss[1] >= 1809.0, "{loss:?}");
    let svg = p.figure.to_svg();
    assert!(
        svg.contains("data-series=\"loss_modulus\""),
        "no loss modulus drawn"
    );
}

#[test]
fn an_unknown_x_channel_lists_the_channels() {
    let (_dir, f, reg) = setup(sweep());
    let spec =
        parse_spec(json!({"data": {"file": f, "y": "loss_modulus", "x": "frequency"}})).unwrap();
    let e = plot(&reg, &spec).unwrap_err().to_string();
    assert!(
        e.contains("no channel frequency") && e.contains("angular_frequency"),
        "{e}"
    );
}

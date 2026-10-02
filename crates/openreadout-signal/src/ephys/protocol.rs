//! The stimulus of each sweep, from an ABF epoch table (`info` → `traces[].extra.outputs`).
//!
//! Timing follows the ABF episodic convention as pyABF documents it (MIT, read as
//! documentation): the first `floor(sweep_samples / 64)` samples of every sweep are a holding
//! period, then the epochs follow back to back, epoch `e` lasting
//! `duration + sweep · duration_step` samples at level `level + sweep · level_step`; the rest of
//! the sweep holds. Only step epochs define a stimulus level here; ramps and trains are listed
//! but not used for current-step analysis.
//!
//! The **stimulus epoch** of a protocol is the first step epoch whose level changes from sweep
//! to sweep (`level_step ≠ 0`); when no level changes, the first step epoch whose level differs
//! from the holding level. Its level is the injected current (current clamp) or command voltage
//! (voltage clamp) of each sweep.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use openreadout_core::model::TraceInfo;

/// One epoch placed in one sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SweepEpoch {
    /// Epoch index in the protocol table.
    pub index: u32,
    /// Epoch kind (`step`, `ramp`, `pulse`, …).
    pub kind: String,
    /// First sample within the sweep.
    pub start_sample: u64,
    /// One past the last sample.
    pub end_sample: u64,
    /// Command level in this sweep, in the output's unit.
    pub level: f64,
}

/// The command output driving a trace and its epochs.
#[derive(Debug, Clone, Default)]
pub struct Protocol {
    /// Output (DAC) name.
    pub output: String,
    /// Output unit (`pA`, `mV`, …).
    pub unit: Option<String>,
    /// Holding level.
    pub holding: f64,
    epochs: Vec<EpochDef>,
    sweep_samples: u64,
    stimulus: Option<usize>,
}

#[derive(Debug, Clone)]
struct EpochDef {
    index: u32,
    kind: String,
    level: f64,
    level_step: f64,
    duration: i64,
    duration_step: i64,
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

impl Protocol {
    /// The protocol of an ABF trace: the first output with its waveform enabled and a non-empty
    /// epoch table. `None` for other formats and gap-free recordings.
    pub fn from_trace(t: &TraceInfo) -> Option<Self> {
        let outputs = t.extra.get("outputs")?.as_array()?;
        let o = outputs.iter().find(|o| {
            o.get("waveform_enabled").and_then(Value::as_bool) == Some(true)
                && o.get("epochs")
                    .and_then(Value::as_array)
                    .is_some_and(|e| !e.is_empty())
        })?;
        let epochs: Vec<EpochDef> = o
            .get("epochs")?
            .as_array()?
            .iter()
            .filter_map(|e| {
                Some(EpochDef {
                    index: e.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                    kind: e
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    level: f(e, "level")?,
                    level_step: f(e, "level_step").unwrap_or(0.0),
                    duration: e.get("duration").and_then(Value::as_i64).unwrap_or(0),
                    duration_step: e.get("duration_step").and_then(Value::as_i64).unwrap_or(0),
                })
            })
            .filter(|e| e.kind != "off" && e.kind != "disabled")
            .collect();
        if epochs.is_empty() {
            return None;
        }
        let holding = f(o, "holding_level").unwrap_or(0.0);
        let steps = |e: &&EpochDef| e.kind == "step";
        let stimulus = epochs
            .iter()
            .filter(steps)
            .position(|e| e.level_step != 0.0)
            .or_else(|| {
                epochs
                    .iter()
                    .filter(steps)
                    .position(|e| (e.level - holding).abs() > 1e-9)
            })
            .and_then(|k| {
                // position among step epochs → position in the table
                let idx = epochs.iter().filter(steps).nth(k)?.index;
                epochs.iter().position(|e| e.index == idx)
            });
        Some(Self {
            output: o
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            unit: o.get("unit").and_then(Value::as_str).map(str::to_string),
            holding,
            epochs,
            sweep_samples: t.sample_count,
            stimulus,
        })
    }

    /// Epochs of sweep `sweep`, clipped to the sweep.
    pub fn sweep_epochs(&self, sweep: u32) -> Vec<SweepEpoch> {
        let n = self.sweep_samples;
        let mut pos = (n / 64) as i64;
        let s = f64::from(sweep);
        let mut out = Vec::new();
        for e in &self.epochs {
            let dur = (e.duration + e.duration_step.saturating_mul(i64::from(sweep))).max(0);
            let a = pos.clamp(0, n as i64) as u64;
            let b = pos.saturating_add(dur).clamp(0, n as i64) as u64;
            out.push(SweepEpoch {
                index: e.index,
                kind: e.kind.clone(),
                start_sample: a,
                end_sample: b,
                level: e.level + s * e.level_step,
            });
            pos = pos.saturating_add(dur);
        }
        out
    }

    /// The stimulus epoch of sweep `sweep` (module docs).
    pub fn stimulus(&self, sweep: u32) -> Option<SweepEpoch> {
        let k = self.stimulus?;
        let e = self.sweep_epochs(sweep).into_iter().nth(k)?;
        (e.end_sample > e.start_sample).then_some(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trace() -> TraceInfo {
        let mut t = TraceInfo {
            sample_count: 60000,
            sweep_count: 17,
            ..TraceInfo::default()
        };
        t.extra.insert(
            "outputs".into(),
            json!([{"name": "Cmd 0", "unit": "pA", "holding_level": 0.0, "waveform_enabled": true,
                "epochs": [
                    {"index": 0, "kind": "step", "level": 0.0, "level_step": 0.0, "duration": 2000, "duration_step": 0},
                    {"index": 1, "kind": "step", "level": -100.0, "level_step": 25.0, "duration": 10000, "duration_step": 0},
                    {"index": 2, "kind": "step", "level": 0.0, "level_step": 0.0, "duration": 10000, "duration_step": 0}]},
                {"name": "Cmd 1", "waveform_enabled": false, "epochs": []}]),
        );
        t
    }

    #[test]
    fn stimulus_epoch_and_timing() {
        let p = Protocol::from_trace(&trace()).unwrap();
        assert_eq!(p.unit.as_deref(), Some("pA"));
        let s = p.stimulus(6).unwrap();
        assert_eq!(s.index, 1);
        assert_eq!(s.start_sample, 937 + 2000);
        assert_eq!(s.end_sample, 937 + 12000);
        assert!((s.level - 50.0).abs() < 1e-12);
    }

    #[test]
    fn no_protocol_without_outputs() {
        assert!(Protocol::from_trace(&TraceInfo::default()).is_none());
    }
}

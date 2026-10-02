//! Command (DAC) waveforms synthesized from the epoch table, one sweep at a time.
//!
//! Only what the header fixes completely is synthesized (right or refuse): ABF 2 episodic files
//! whose DAC waveform comes from the epoch table (not a stimulus file), with step, ramp and
//! pulse-train epochs only, fixed-length sweeps, and no user list, alternating DAC outputs or
//! conditioning train. Anything else leaves the command out and says why (`CommandPlan::refused`).
//!
//! Layout of a sweep of `n` samples (the rules pCLAMP applies, as pyABF documents them):
//! the first `n / 64` samples hold the pre-sweep level, then the epochs in order (duration and
//! level = first value + increment × sweep), then the post-sweep level to the end. The pre-sweep
//! level is the holding level, or with "use last epoch level" (`inter_episode_last`) the last
//! epoch level of the previous sweep; the post-sweep level is the holding level or that sweep's
//! last epoch level. A ramp runs from the level before it to its own level (both included); a
//! pulse train holds the level before it and raises it to the epoch level for `pulse_width`
//! samples every `pulse_period` samples.

use crate::file::{AbfFile, AcquisitionMode, EpochKind, Generation, OutputChannel};

/// Which DACs get a synthesized command trace, or why none do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandPlan {
    /// Indices into `AbfFile::outputs`.
    pub outputs: Vec<usize>,
    /// Why DAC waveforms that the file declares are not synthesized (empty when all are).
    pub refused: Vec<String>,
}

/// Decide which command waveforms can be synthesized exactly.
pub fn command_plan(f: &AbfFile) -> CommandPlan {
    let mut plan = CommandPlan::default();
    let declared: Vec<usize> = f
        .outputs
        .iter()
        .enumerate()
        .filter(|(_, o)| o.waveform_enabled && o.waveform_source_code != 0)
        .map(|(i, _)| i)
        .collect();
    if declared.is_empty() {
        return plan;
    }
    let file_reason = if f.generation != Generation::Abf2 {
        Some("ABF 1 command waveforms are not synthesized (the epoch table is reported)")
    } else if f.mode != AcquisitionMode::Episodic {
        Some("command waveforms are synthesized for episodic-stimulation files only")
    } else if f.variable_length() {
        Some("sweeps of different length: the command waveform is not synthesized")
    } else if f.user_list_active {
        Some("a user list varies epoch parameters: the command waveform is not synthesized")
    } else if f.alternate_dac_output {
        Some("alternating DAC outputs: the command waveform is not synthesized")
    } else {
        None
    };
    if let Some(r) = file_reason {
        plan.refused.push(r.to_string());
        return plan;
    }
    for i in declared {
        let o = &f.outputs[i];
        if let Some(r) = output_refusal(o) {
            plan.refused.push(format!("DAC {}: {r}", o.index));
        } else {
            plan.outputs.push(i);
        }
    }
    plan
}

fn output_refusal(o: &OutputChannel) -> Option<String> {
    if o.waveform_source_code != 1 {
        return Some(format!(
            "waveform source {} (a stimulus file) is not synthesized",
            o.waveform_source_code
        ));
    }
    if o.conditioning {
        return Some("a conditioning train precedes the sweeps; not synthesized".into());
    }
    for e in &o.epochs {
        match e.kind {
            EpochKind::Off | EpochKind::Step | EpochKind::Ramp => {}
            EpochKind::PulseTrain => {
                let (Some(p), Some(w)) = (e.pulse_period, e.pulse_width) else {
                    return Some("a pulse train without period or width".into());
                };
                if p <= 0 || w < 0 {
                    return Some(format!("a pulse train with period {p} and width {w}"));
                }
            }
            k => return Some(format!("{} epochs are not synthesized", k.name())),
        }
        if e.duration < 0 {
            return Some(format!("epoch {} has a negative duration", e.index));
        }
    }
    None
}

/// One segment of a synthesized sweep: `[start, end)` at `level`, shaped by `kind`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Segment {
    start: u64,
    end: u64,
    level: f64,
    kind: EpochKind,
    pulse_period: u64,
    pulse_width: u64,
}

/// The level a sweep starts from: holding, or the last epoch level of the previous sweep.
fn epoch_levels(o: &OutputChannel, sweep: u32) -> Vec<(u64, f64)> {
    o.epochs
        .iter()
        .filter(|e| e.kind != EpochKind::Off)
        .map(|e| {
            let d = i64::from(e.duration) + i64::from(e.duration_step) * i64::from(sweep);
            (
                u64::try_from(d.max(0)).unwrap_or(0),
                f64::from(e.level) + f64::from(e.level_step) * f64::from(sweep),
            )
        })
        .collect()
}

/// The command waveform of output `o` for sweep `sweep` (`n` samples), or `None` when its epochs
/// run past the end of the sweep.
pub fn command_sweep(o: &OutputChannel, sweep: u32, n: u64) -> Option<Vec<f64>> {
    let holding = f64::from(o.holding_level);
    let last_level = |s: u32| -> f64 {
        epoch_levels(o, s)
            .last()
            .map_or(holding, |&(_, level)| level)
    };
    let pre_level = if sweep > 0 && o.inter_episode_last {
        last_level(sweep - 1)
    } else {
        holding
    };
    let post_level = if o.inter_episode_last {
        last_level(sweep)
    } else {
        holding
    };
    let pre = n / 64;
    let mut segs = vec![Segment {
        start: 0,
        end: pre,
        level: pre_level,
        kind: EpochKind::Step,
        pulse_period: 0,
        pulse_width: 0,
    }];
    let mut pos = pre;
    let live: Vec<_> = o
        .epochs
        .iter()
        .filter(|e| e.kind != EpochKind::Off)
        .collect();
    for (e, (dur, level)) in live.iter().zip(epoch_levels(o, sweep)) {
        let end = pos.checked_add(dur)?;
        segs.push(Segment {
            start: pos,
            end,
            level,
            kind: e.kind,
            pulse_period: e
                .pulse_period
                .and_then(|v| u64::try_from(v).ok())
                .unwrap_or(0),
            pulse_width: e
                .pulse_width
                .and_then(|v| u64::try_from(v).ok())
                .unwrap_or(0),
        });
        pos = end;
    }
    if pos > n {
        return None;
    }
    segs.push(Segment {
        start: pos,
        end: n,
        level: post_level,
        kind: EpochKind::Step,
        pulse_period: 0,
        pulse_width: 0,
    });
    let mut out = vec![0.0f64; usize::try_from(n).ok()?];
    for (k, s) in segs.iter().enumerate() {
        let before = if k == 0 { s.level } else { segs[k - 1].level };
        let len = s.end - s.start;
        let chunk = out.get_mut(usize::try_from(s.start).ok()?..usize::try_from(s.end).ok()?)?;
        match s.kind {
            EpochKind::Ramp => linspace_into(chunk, before, s.level),
            EpochKind::PulseTrain => {
                chunk.fill(before);
                if let Some(pulses) = len.checked_div(s.pulse_period) {
                    for p in 0..pulses {
                        let a = p * s.pulse_period;
                        let b = (a + s.pulse_width).min(len);
                        for v in &mut chunk[usize::try_from(a).ok()?..usize::try_from(b).ok()?] {
                            *v = s.level;
                        }
                    }
                }
            }
            _ => chunk.fill(s.level),
        }
    }
    Some(out)
}

/// `n` evenly spaced values from `a` to `b`, both included (`i × step + a`, the last set to `b`).
fn linspace_into(out: &mut [f64], a: f64, b: f64) {
    let n = out.len();
    if n == 0 {
        return;
    }
    if n == 1 {
        out[0] = a;
        return;
    }
    let div = (n - 1) as f64;
    let step = (b - a) / div;
    for (i, v) in out.iter_mut().enumerate() {
        *v = if step == 0.0 { a } else { i as f64 * step + a };
    }
    out[n - 1] = b;
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::file::Epoch;

    fn out(epochs: Vec<Epoch>, last: bool) -> OutputChannel {
        OutputChannel {
            index: 0,
            name: "Cmd 0".into(),
            unit: "mV".into(),
            holding_level: -70.0,
            waveform_enabled: true,
            waveform_source_code: 1,
            stimulus_file: None,
            epochs,
            inter_episode_last: last,
            conditioning: false,
        }
    }

    fn epoch(kind: EpochKind, level: f32, step: f32, dur: i32, period: i32, width: i32) -> Epoch {
        Epoch {
            index: 0,
            kind,
            kind_code: 0,
            level,
            level_step: step,
            duration: dur,
            duration_step: 0,
            pulse_period: Some(period),
            pulse_width: Some(width),
        }
    }

    #[test]
    fn step_ramp_and_pulses() {
        let o = out(
            vec![
                epoch(EpochKind::Step, -50.0, 10.0, 4, 0, 0),
                epoch(EpochKind::Ramp, 0.0, 0.0, 3, 0, 0),
                epoch(EpochKind::PulseTrain, 20.0, 0.0, 6, 3, 1),
            ],
            false,
        );
        // 128 samples: 2 pre-sweep samples, then 4 + 3 + 6, then holding
        let w = command_sweep(&o, 1, 128).unwrap();
        assert_eq!(&w[..2], &[-70.0, -70.0]);
        assert_eq!(&w[2..6], &[-40.0; 4]);
        assert_eq!(&w[6..9], &[-40.0, -20.0, 0.0]);
        assert_eq!(&w[9..15], &[20.0, 0.0, 0.0, 20.0, 0.0, 0.0]);
        assert!(w[15..].iter().all(|v| *v == -70.0));
        // "use last epoch level": sweep 1 starts and ends at sweep 0's / its own last level
        let o = out(vec![epoch(EpochKind::Step, -50.0, 10.0, 4, 0, 0)], true);
        let w = command_sweep(&o, 1, 128).unwrap();
        assert_eq!((w[0], w[2], w[127]), (-50.0, -40.0, -40.0));
        // epochs longer than the sweep: not synthesized
        let o = out(vec![epoch(EpochKind::Step, 1.0, 0.0, 400, 0, 0)], false);
        assert!(command_sweep(&o, 0, 128).is_none());
    }
}

//! Voltammetry previews: current against potential, the way a voltammogram is read, instead of
//! each channel against time. Each cycle (the reader's `cycle` channel, or each sweep when a
//! file stores cycles as sweeps) is one loop, from faint (first) to ink (last).

use openreadout_core::model::{FileInfo, SignalChannelInfo, TraceInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Figure, GridLines, Panel, Rect, ScaleKind, Stroke,
    TextStyle, Theme, key_row,
};

use crate::color::to_hex;
use crate::{
    Frame, FramePanel, PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel, XAxis,
};

/// Most samples read per sweep.
const MAX_SAMPLES: u64 = 2_000_000;
/// Most cycles drawn (the last ones).
const MAX_CYCLES: usize = 50;

fn is_volts(c: &SignalChannelInfo) -> bool {
    c.unit
        .as_deref()
        .is_some_and(|u| u.trim() == "V" || u.trim() == "mV")
}

fn is_amps(c: &SignalChannelInfo) -> bool {
    c.unit
        .as_deref()
        .is_some_and(|u| matches!(u.trim(), "A" | "mA" | "µA" | "uA" | "μA" | "nA" | "pA"))
}

/// The working-electrode potential and the current channels.
fn channels(t: &TraceInfo) -> Option<(u32, u32)> {
    let name = |c: &SignalChannelInfo| c.name.to_ascii_lowercase();
    let we = t
        .channels
        .iter()
        .filter(|c| is_volts(c))
        .find(|c| {
            let n = name(c);
            (n.contains("ewe") || n == "potential" || n.contains("e_we") || n == "voltage")
                && !n.contains("control")
                && !n.contains("uncomp")
        })
        .or_else(|| {
            t.channels.iter().filter(|c| is_volts(c)).find(|c| {
                let n = name(c);
                !n.contains("control") && !n.contains("ece") && !n.contains("aux")
            })
        })?;
    let cur = t
        .channels
        .iter()
        .filter(|c| is_amps(c))
        .find(|c| name(c).contains("current") || name(c) == "i")
        .or_else(|| t.channels.iter().find(|c| is_amps(c)))?;
    Some((we.index, cur.index))
}

fn voltammetry(t: &TraceInfo) -> bool {
    let technique = t
        .extra
        .get("technique")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let words =
        format!("{technique} {}", t.name.as_deref().unwrap_or_default()).to_ascii_lowercase();
    words.contains("voltammetry")
        || words
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| matches!(w, "cv" | "lsv" | "cva"))
}

/// Should this request draw a voltammogram? Electrochemistry traces from a voltammetry
/// technique, with a potential and a current channel, when no channels were picked.
pub(crate) fn is_voltammetry(info: &FileInfo, req: &PreviewRequest) -> bool {
    if info.format.family != "electrochemistry" || !req.channels.is_empty() {
        return false;
    }
    let ti = req.trace.unwrap_or(0);
    info.traces
        .iter()
        .find(|t| t.index == ti)
        .is_some_and(|t| voltammetry(t) && channels(t).is_some())
}

fn axis_name(channel: &str, what: &str, unit: Option<&str>) -> String {
    let name = if channel.eq_ignore_ascii_case(what) {
        what.to_string()
    } else {
        format!("{what}, {channel}")
    };
    match unit {
        Some(u) => format!("{name} ({u})"),
        None => name,
    }
}

/// Amperes per unit of a current channel.
fn amps_per(unit: &str) -> Option<f64> {
    Some(match unit.trim() {
        "A" => 1.0,
        "mA" => 1e-3,
        "µA" | "uA" | "μA" => 1e-6,
        "nA" => 1e-9,
        "pA" => 1e-12,
        _ => return None,
    })
}

/// The unit that writes currents up to `largest` (in amperes) with 1–3 digits before the point,
/// and the factor from amperes to it.
fn current_unit(largest: f64) -> (&'static str, f64) {
    let units = [
        ("A", 1.0),
        ("mA", 1e-3),
        ("µA", 1e-6),
        ("nA", 1e-9),
        ("pA", 1e-12),
    ];
    for (u, a) in units {
        if largest >= a {
            return (u, 1.0 / a);
        }
    }
    ("pA", 1e12)
}

/// Cycles as runs of `(potential, current)`: split on the `cycle` channel when there is one.
fn split_cycles(e: &[f64], i: &[f64], cycle: Option<&[f64]>) -> Vec<(String, Vec<(f64, f64)>)> {
    let mut out: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
    for (k, (&x, &y)) in e.iter().zip(i).enumerate() {
        let key = cycle
            .and_then(|c| c.get(k))
            .filter(|v| v.is_finite())
            .map_or_else(String::new, |v| format!("{}", v.round() as i64));
        match out.last_mut() {
            Some((name, pts)) if *name == key => pts.push((x, y)),
            _ => out.push((key, vec![(x, y)])),
        }
    }
    out
}

/// A voltammogram read from a file: current against potential, one run of points per cycle.
#[derive(Debug, Clone)]
pub struct Voltammogram {
    /// Trace index.
    pub trace: u32,
    /// Trace name.
    pub name: String,
    /// The potential channel's index, name and unit.
    pub potential: (u32, String, Option<String>),
    /// The current channel's index, name and the unit the values are in (scaled from the
    /// file's so they read with 1–3 digits).
    pub current: (u32, String, Option<String>),
    /// Cycles in order: name (the cycle number, or the sweep, or empty) and points.
    pub cycles: Vec<(String, Vec<(f64, f64)>)>,
    /// Samples read.
    pub samples: u64,
    /// The first sweep read.
    pub sweep: u32,
    /// Remarks.
    pub notes: Vec<String>,
}

/// The voltammogram of trace `trace` (default 0): sweep `sweep`, or every sweep (files that
/// store each cycle as a sweep). `None` when the trace is no voltammetry or lacks a potential
/// or current channel.
pub fn voltammogram(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    trace: Option<u32>,
    sweep: Option<u32>,
) -> Result<Option<Voltammogram>> {
    let ti = trace.unwrap_or(0);
    let Some(t) = info.traces.iter().find(|t| t.index == ti).cloned() else {
        return Ok(None);
    };
    if info.format.family != "electrochemistry" || !voltammetry(&t) {
        return Ok(None);
    }
    let Some((we, cur)) = channels(&t) else {
        return Ok(None);
    };
    let cyc = t
        .channels
        .iter()
        .find(|c| {
            c.name.eq_ignore_ascii_case("cycle") || c.name.eq_ignore_ascii_case("cycle number")
        })
        .map(|c| c.index);
    let sweeps: Vec<u32> = match sweep {
        Some(s) if s < t.sweep_count => vec![s],
        Some(s) => {
            return Err(Error::Usage(format!(
                "sweep {s} out of range (trace {ti} has {} sweeps)",
                t.sweep_count
            )));
        }
        None => (0..t.sweep_count).collect(),
    };
    let mut cycles: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
    let mut drawn = 0u64;
    for &s in &sweeps {
        let n = openreadout_core::trace::sweep_samples(&t, s).min(MAX_SAMPLES);
        let mut want = vec![we, cur];
        want.extend(cyc);
        let cols = openreadout_core::trace::read_channels(ds, ti, s, 0, n, &want)?;
        drawn += cols.first().map_or(0, Vec::len) as u64;
        let (e, i) = (&cols[0], &cols[1]);
        let parts = split_cycles(e, i, cols.get(2).map(Vec::as_slice));
        for (k, p) in parts {
            let name = if sweeps.len() > 1 {
                format!("{}", s + 1)
            } else {
                k
            };
            cycles.push((name, p));
        }
    }
    let mut notes = Vec::new();
    if cycles.len() > MAX_CYCLES {
        notes.push(format!(
            "drew the last {MAX_CYCLES} of {} cycles",
            cycles.len()
        ));
        cycles.drain(..cycles.len() - MAX_CYCLES);
    }
    let wch = &t.channels[we as usize];
    let ich = &t.channels[cur as usize];
    // currents in a unit that keeps the tick labels short
    let unit_in = ich.unit.clone().unwrap_or_default();
    let mut i_unit = ich.unit.clone();
    if let Some(a) = amps_per(&unit_in) {
        let largest = cycles
            .iter()
            .flat_map(|c| c.1.iter())
            .map(|p| (p.1 * a).abs())
            .filter(|v| v.is_finite())
            .fold(0.0, f64::max);
        if largest > 0.0 {
            let (u, k) = current_unit(largest);
            if u != unit_in {
                for c in &mut cycles {
                    for p in &mut c.1 {
                        p.1 *= a * k;
                    }
                }
                i_unit = Some(u.to_string());
            }
        }
    }
    Ok(Some(Voltammogram {
        trace: ti,
        name: t.name.clone().unwrap_or_else(|| format!("trace {ti}")),
        potential: (we, wch.name.clone(), wch.unit.clone()),
        current: (cur, ich.name.clone(), i_unit),
        cycles,
        samples: drawn,
        sweep: sweeps.first().copied().unwrap_or(0),
        notes,
    }))
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let v = voltammogram(ds, info, req.trace, req.sweep)?
        .ok_or_else(|| Error::Other("no potential and current channels".into()))?;
    out.notes.extend(v.notes.iter().cloned());
    let Voltammogram {
        trace: ti,
        name,
        cycles,
        ..
    } = v.clone();
    let (_, wname, wunit) = v.potential.clone();
    let (cur, iname, i_unit) = v.current.clone();
    let drawn = v.samples;
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let all = cycles.iter().flat_map(|c| c.1.iter());
    let x_dom = Domain::of(all.clone().map(|p| p.0), 0.03, ScaleKind::Linear);
    let y_dom = Domain::of(all.map(|p| p.1), 0.06, ScaleKind::Linear);

    let w = req.max_size.max(64) as f32;
    let header = fs * 1.9;
    let many = cycles.len() > 1;
    let key_h = if many { fs * 1.8 } else { 0.0 };
    let panel_h = (w * 0.62)
        .min(req.max_size as f32 - header - key_h - fs * 4.0)
        .max(100.0);
    let height = (header + key_h + panel_h + fs * 5.0).ceil();
    let mut fig = Figure::new(w, height, Some(theme.paper));
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &name,
        &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
    );
    let colors = cycle_colors(&theme, cycles.len());
    if many {
        let first = cycles.first().map_or(String::new(), |c| c.0.clone());
        let last = cycles.last().map_or(String::new(), |c| c.0.clone());
        let items = if first.is_empty() {
            vec![]
        } else {
            vec![
                (colors[0], format!("cycle {first}")),
                (colors[colors.len() - 1], format!("cycle {last}")),
            ]
        };
        key_row(
            &mut fig,
            &theme,
            fs * 0.8,
            header + fs * 0.7,
            w,
            &items,
            false,
        );
    }
    let spec = AxesSpec {
        x_label: Some(axis_name(&wname, "potential", wunit.as_deref())),
        y_label: Some(axis_name(&iname, "current", i_unit.as_deref())),
        grid: GridLines::Both,
        ..AxesSpec::default()
    };
    let outer = Rect::new(0.0, header + key_h, w, height - header - key_h);
    let panel = Panel::fit(
        outer.inset_sides(fs * 0.8, 0.0, fs * 1.0, fs * 0.4),
        x_dom,
        y_dom,
        &theme,
        &spec,
    );
    panel.axes(&mut fig, &theme, &spec);
    if y_dom.lo < 0.0 && y_dom.hi > 0.0 {
        panel.rule_y(&mut fig, 0.0, Stroke::new(theme.rule, theme.hairline()));
    }
    for ((_, pts), c) in cycles.iter().zip(&colors) {
        panel.line(
            &mut fig,
            pts.iter().copied(),
            Stroke::new(*c, theme.line_width * 0.9).round(),
        );
    }
    out.notes.push(format!(
        "voltammogram: {iname} against {wname}{}; pass channels to draw channels against time",
        if many {
            format!(", {} cycles", cycles.len())
        } else {
            String::new()
        }
    ));
    out.trace = Some(TracePreview {
        trace: ti,
        sweep: v.sweep,
        sample_rate_hz: 0.0,
        sweep_sample_count: drawn,
        sample_count: drawn,
        truncated: false,
        x_axis: Some(XAxis {
            quantity: "potential".into(),
            unit: wunit.clone().unwrap_or_default(),
            first: x_dom.lo,
            last: x_dom.hi,
        }),
        channels: vec![TracePreviewChannel {
            index: cur,
            name: iname.clone(),
            unit: i_unit.clone(),
            color: to_hex(theme.ink.to_rgb()),
            min: Some(y_dom.lo),
            max: Some(y_dom.hi),
        }],
    });
    let panels = vec![FramePanel {
        channel: Some(cur),
        ..FramePanel::new(panel)
    }];
    Ok((fig, Frame::plots(panels)))
}

/// Cycle colours from faint (first) to ink (last); one cycle is ink.
fn cycle_colors(theme: &Theme, n: usize) -> Vec<openreadout_figure::Color> {
    if n <= 1 {
        return vec![theme.ink];
    }
    let start = theme.faint.mix(theme.ink, 0.25);
    (0..n)
        .map(|k| start.mix(theme.ink, k as f32 / (n - 1) as f32))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_split_where_the_cycle_number_changes() {
        let e = [0.0, 1.0, 2.0, 1.0, 0.0];
        let i = [0.0; 5];
        let c = [1.0, 1.0, 1.0, 2.0, 2.0];
        let parts = split_cycles(&e, &i, Some(&c));
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].0, "2");
        assert_eq!(split_cycles(&e, &i, None).len(), 1);
    }
}

//! `trace --tidy`: summary statistics of sampled signals (electrophysiology sweeps,
//! chromatograms, NMR spectra), one row per data set × trace × sweep × channel.

use openreadout_core::Result;
use openreadout_core::trace::{TraceRequest, slice_trace};

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Row, Value};

/// Trace statistics.
#[derive(Debug, Clone, Default)]
pub struct TraceMeasure {
    /// Only this trace (default: every trace).
    pub trace: Option<u32>,
    /// Only this sweep (default: every sweep).
    pub sweep: Option<u32>,
    /// Only these channels (default: all).
    pub channels: Vec<u32>,
    /// First sample of the window within each sweep.
    pub first: u64,
    /// Window length in samples (default: to the end of the sweep).
    pub count: Option<u64>,
    /// Window on each trace's axis instead (axis units; seconds without an axis).
    pub x_range: Option<[f64; 2]>,
}

impl Measure for TraceMeasure {
    fn id(&self) -> &'static str {
        "trace"
    }
    fn grain(&self) -> Vec<String> {
        vec!["trace".into(), "sweep".into(), "channel".into()]
    }
    fn columns(&self) -> Vec<ColumnDoc> {
        vec![
            ColumnDoc::key("trace", "trace index, from 0"),
            ColumnDoc::key("trace_name", "trace name as recorded"),
            ColumnDoc::key("sweep", "sweep (episode, segment) index, from 0"),
            ColumnDoc::key("channel", "channel index, from 0"),
            ColumnDoc::key("channel_name", "channel name as recorded"),
            ColumnDoc::value("unit", "unit of min, max, mean and std (pA, mV, mAU, …)"),
            ColumnDoc::unit("sample_rate_hz", "Hz", "samples per second"),
            ColumnDoc::value("samples", "samples in the window"),
            ColumnDoc::unit("start_s", "s", "window start from the sweep start"),
            ColumnDoc::unit("duration_s", "s", "window length (samples / rate)"),
            ColumnDoc::value("min", "smallest finite sample, in `unit`"),
            ColumnDoc::value("max", "largest finite sample, in `unit`"),
            ColumnDoc::value("mean", "mean of the finite samples"),
            ColumnDoc::value("std", "population standard deviation"),
            ColumnDoc::value("argmin", "sample index (within the sweep) of the minimum"),
            ColumnDoc::value("argmax", "sample index (within the sweep) of the maximum"),
            ColumnDoc::value(
                "x_at_max",
                "abscissa of the maximum in `x_unit` (time in s, or the spectrum's axis)",
            ),
            ColumnDoc::value("x_unit", "unit of x_at_max (s, ppm, min, …)"),
        ]
    }
    fn fingerprint(&self) -> String {
        format!(
            "trace:{:?}:{:?}:{:?}:{}:{:?}{}",
            self.trace,
            self.sweep,
            self.channels,
            self.first,
            self.count,
            self.x_range
                .map_or(String::new(), |[a, b]| format!(":x{a}:{b}"))
        )
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        if it.info.traces.is_empty() {
            return Err(super::not_applicable(
                "trace",
                it.info,
                "traces (sampled signals, chromatograms or spectra)",
                "Use `stats --tidy` for images and `table --tidy` for tables.",
            ));
        }
        let traces: Vec<_> = it
            .info
            .traces
            .iter()
            .filter(|t| self.trace.is_none_or(|w| w == t.index))
            .cloned()
            .collect();
        if traces.is_empty() {
            return Err(openreadout_core::Error::Usage(format!(
                "trace {} out of range (the file has {} traces)",
                self.trace.unwrap_or(0),
                it.info.traces.len()
            )));
        }
        let mut rows = Vec::new();
        for t in &traces {
            let sweeps: Vec<u32> = match self.sweep {
                Some(s) => vec![s],
                None => (0..t.sweep_count).collect(),
            };
            for s in sweeps {
                let req = {
                    let mut r = TraceRequest::default();
                    r.trace = t.index;
                    r.sweep = s;
                    r.channels = self.channels.clone();
                    r.first_sample = self.first;
                    r.count = self.count;
                    r.x_range = self.x_range;
                    r.max_samples = 0;
                    r
                };
                let sl = slice_trace(it.dataset, it.info, &req)?;
                let rate = sl.sample_rate_hz;
                let axis = sl.axis.as_ref();
                let x_of = |i: u64| -> Option<f64> {
                    if let Some(a) = axis {
                        let first = a.get("first").and_then(serde_json::Value::as_f64)?;
                        let step = a.get("step").and_then(serde_json::Value::as_f64)?;
                        return Some(first + i as f64 * step);
                    }
                    (rate > 0.0).then(|| i as f64 / rate)
                };
                let x_unit = axis
                    .and_then(|a| a.get("unit").and_then(|u| u.as_str()).map(str::to_string))
                    .or_else(|| (rate > 0.0).then(|| "s".to_string()));
                for ch in &sl.channels {
                    let st = &ch.stats;
                    let mut r = Row::new().with("trace", t.index);
                    if let Some(n) = &t.name {
                        r.set("trace_name", n.as_str());
                    }
                    r.set("sweep", s);
                    r.set("channel", ch.index);
                    r.set("channel_name", Value::text_opt(Some(&ch.name)));
                    r.set("unit", Value::text_opt(ch.unit.as_deref()));
                    r.set("sample_rate_hz", Value::float_opt(Some(rate)));
                    r.set("samples", sl.sample_count);
                    r.set("start_s", Value::float_opt(Some(sl.start_s)));
                    r.set(
                        "duration_s",
                        Value::float_opt((rate > 0.0).then(|| sl.sample_count as f64 / rate)),
                    );
                    r.set("min", Value::float_opt(st.min));
                    r.set("max", Value::float_opt(st.max));
                    r.set("mean", Value::float_opt(st.mean));
                    r.set("std", Value::float_opt(st.std));
                    r.set("argmin", st.argmin);
                    r.set("argmax", st.argmax);
                    r.set("x_at_max", Value::float_opt(st.argmax.and_then(x_of)));
                    r.set("x_unit", Value::text_opt(x_unit.as_deref()));
                    rows.push(r);
                }
            }
        }
        Ok(rows)
    }
}

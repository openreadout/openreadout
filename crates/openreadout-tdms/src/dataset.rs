//! An open TDMS file: channels as traces (one trace per group when its channels share length and
//! sample interval, else one per channel), properties as metadata, string channels listed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Plane, Result};

use crate::parse::{FMT, File, Object, Prop, T_DAQMX, T_STRING, T_TIMESTAMP, type_name};
use crate::values::{self, Scale};

/// Most samples returned by one `read_trace`.
const MAX_SAMPLES: u64 = 1 << 24;

/// A trace: channels of one group sharing length and sample interval.
#[derive(Debug, Clone)]
struct TraceDef {
    name: String,
    group: String,
    channels: Vec<usize>,
    count: u64,
    increment: Option<f64>,
}

/// An open TDMS file.
#[derive(Debug)]
pub struct TdmsDataset {
    path: PathBuf,
    file: SourceFile,
    len: u64,
    descriptor: FormatDescriptor,
    parsed: File,
    traces: Vec<TraceDef>,
    scales: BTreeMap<usize, Vec<Scale>>,
    notes: Vec<String>,
}

/// A property as JSON (timestamps as ISO 8601 UTC).
fn prop_json(p: &Prop) -> Value {
    match p {
        Prop::Int(v) => json!(v),
        Prop::Float(v) if v.is_finite() => json!(v),
        Prop::Float(v) => json!(v.to_string()),
        Prop::Str(s) => json!(s),
        Prop::Bool(b) => json!(b),
        Prop::Time(t) => json!(iso(*t)),
        Prop::Other(k) => json!(format!("({k})")),
    }
}

/// Seconds since 1970 as ISO 8601 UTC with microseconds.
pub(crate) fn iso(t: f64) -> String {
    if !t.is_finite() {
        return "invalid".into();
    }
    let secs = t.floor();
    let micros = ((t - secs) * 1e6).round().min(999_999.0) as u32;
    let base = openreadout_core::time::unix_to_iso8601(secs as i64, 0);
    // `YYYY-MM-DDTHH:MM:SS.000Z` → microseconds
    format!("{}.{micros:06}Z", &base[..base.len().saturating_sub(5)])
}

fn props_json(o: &Object) -> Value {
    Value::Object(
        o.props
            .iter()
            .map(|(k, v)| (k.clone(), prop_json(v)))
            .collect(),
    )
}

fn increment(o: &Object) -> Option<f64> {
    o.props
        .get("wf_increment")
        .and_then(Prop::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
}

impl TdmsDataset {
    pub(crate) fn new(
        path: PathBuf,
        file: SourceFile,
        len: u64,
        descriptor: FormatDescriptor,
        parsed: File,
    ) -> Self {
        let mut notes = parsed.notes.clone();
        let mut scales = BTreeMap::new();
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, o) in parsed.objects.iter().enumerate() {
            let (Some(g), Some(_)) = (&o.group, &o.channel) else {
                continue;
            };
            if o.count == 0 && o.strings.is_empty() {
                continue;
            }
            if !values::numeric(o) {
                if let Some(p) = &o.problem {
                    notes.push(format!("channel {}: {p}; its values are not read", o.path));
                } else if o.dtype == Some(T_STRING) {
                    // listed in the group's `string_channels`
                } else if o.digital_line {
                    notes.push(format!(
                        "channel {}: DAQmx digital-line data is not read",
                        o.path
                    ));
                } else {
                    notes.push(format!(
                        "channel {}: {} values are not read",
                        o.path,
                        o.dtype.map_or("unknown", type_name)
                    ));
                }
                continue;
            }
            match values::scaling(&o.props) {
                Ok(Some(c)) => {
                    scales.insert(i, c);
                }
                Ok(None) => {}
                Err(e) => {
                    notes.push(format!(
                        "channel {}: {e} is not applied, so its values are the raw stored numbers",
                        o.path
                    ));
                }
            }
            match groups.iter_mut().find(|(n, _)| n == g) {
                Some((_, v)) => v.push(i),
                None => groups.push((g.clone(), vec![i])),
            }
        }
        let mut traces = Vec::new();
        for (g, chans) in groups {
            let o0 = &parsed.objects[chans[0]];
            let same = chans.iter().all(|&c| {
                let o = &parsed.objects[c];
                o.count == o0.count && increment(o) == increment(o0)
            });
            if same {
                traces.push(TraceDef {
                    name: g.clone(),
                    group: g,
                    count: o0.count,
                    increment: increment(o0),
                    channels: chans,
                });
            } else {
                for c in chans {
                    let o = &parsed.objects[c];
                    traces.push(TraceDef {
                        name: format!("{g}/{}", o.channel.clone().unwrap_or_default()),
                        group: g.clone(),
                        count: o.count,
                        increment: increment(o),
                        channels: vec![c],
                    });
                }
            }
        }
        TdmsDataset {
            path,
            file,
            len,
            descriptor,
            parsed,
            traces,
            scales,
            notes,
        }
    }

    fn channel_info(&self, k: usize, id: usize) -> SignalChannelInfo {
        let o = &self.parsed.objects[id];
        let stored = match (o.dtype, o.daqmx_scaler) {
            (Some(T_DAQMX), Some(s)) => values::daqmx_type(s.daqmx_type).map_or_else(
                || "DAQmx raw".to_string(),
                |(_, t)| type_name(t).to_string(),
            ),
            (Some(t), _) => type_name(t).to_string(),
            _ => "unknown".into(),
        };
        let (mut scale, mut offset) = (1.0, 0.0);
        let mut extra = BTreeMap::new();
        extra.insert("path".into(), json!(o.path));
        extra.insert("value_count".into(), json!(o.count));
        if o.dtype == Some(T_DAQMX) {
            extra.insert("storage".into(), json!("DAQmx raw data"));
        }
        if let Some(c) = self.scales.get(&id) {
            if let [Scale::Linear { slope, intercept }] = c.as_slice() {
                scale = *slope;
                offset = *intercept;
            }
            extra.insert(
                "scaling".into(),
                json!(
                    c.iter()
                        .map(|s| match s {
                            Scale::Linear { slope, intercept } =>
                                json!({"type": "linear", "slope": slope, "intercept": intercept}),
                            Scale::Polynomial(k) =>
                                json!({"type": "polynomial", "coefficients": k}),
                        })
                        .collect::<Vec<_>>()
                ),
            );
        }
        if !o.props.is_empty() {
            extra.insert("properties".into(), props_json(o));
        }
        let unit = if o.dtype == Some(T_TIMESTAMP) {
            Some("s since 1970-01-01T00:00:00Z".to_string())
        } else {
            o.props
                .get("unit_string")
                .and_then(Prop::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        };
        SignalChannelInfo {
            index: k as u32,
            name: o.channel.clone().unwrap_or_default(),
            unit,
            dtype: stored,
            scale,
            offset,
            extra,
        }
    }

    fn trace_infos(&self) -> Vec<TraceInfo> {
        self.traces
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let o0 = &self.parsed.objects[t.channels[0]];
                let mut extra = BTreeMap::new();
                extra.insert("group".into(), json!(t.group));
                if let Some(v) = t.increment {
                    extra.insert("wf_increment".into(), json!(v));
                }
                if let Some(Prop::Time(v)) = o0.props.get("wf_start_time") {
                    extra.insert("wf_start_time".into(), json!(iso(*v)));
                }
                if let Some(g) = self
                    .parsed
                    .objects
                    .iter()
                    .find(|o| o.channel.is_none() && o.group.as_deref() == Some(&t.group))
                    && !g.props.is_empty()
                {
                    extra.insert("group_properties".into(), props_json(g));
                }
                TraceInfo {
                    index: i as u32,
                    name: Some(t.name.clone()),
                    sample_rate_hz: t.increment.map_or(0.0, |d| 1.0 / d),
                    sample_count: t.count,
                    sweep_count: 1,
                    channels: t
                        .channels
                        .iter()
                        .enumerate()
                        .map(|(k, &id)| self.channel_info(k, id))
                        .collect(),
                    start_s: o0
                        .props
                        .get("wf_start_offset")
                        .and_then(Prop::as_f64)
                        .filter(|v| v.is_finite()),
                    extra,
                }
            })
            .collect()
    }

    fn file_props(&self) -> BTreeMap<String, Prop> {
        self.parsed
            .objects
            .iter()
            .find(|o| o.group.is_none())
            .map(|o| o.props.clone())
            .unwrap_or_default()
    }
}

/// The software a file names as its writer, from the file properties some writers set.
pub(crate) fn writer_of(props: &BTreeMap<String, Prop>) -> Option<String> {
    for k in ["WriterName", "Writer", "NI_ExpSoftwareName"] {
        if let Some(v) = props
            .get(k)
            .and_then(Prop::as_str)
            .filter(|v| !v.trim().is_empty())
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

impl Dataset for TdmsDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut traces = self.trace_infos();
        let root = self.file_props();
        let strings: Vec<Value> = self
            .parsed
            .objects
            .iter()
            .filter(|o| o.dtype == Some(T_STRING) && o.channel.is_some())
            .map(|o| json!({"path": o.path, "values": o.count}))
            .collect();
        if let Some(t) = traces.first_mut() {
            let e = &mut t.extra;
            e.insert("segments".into(), json!(self.parsed.segments));
            e.insert(
                "tdms_versions".into(),
                json!(
                    self.parsed
                        .versions
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                ),
            );
            if !root.is_empty() {
                e.insert(
                    "file_properties".into(),
                    Value::Object(
                        root.iter()
                            .map(|(k, v)| (k.clone(), prop_json(v)))
                            .collect(),
                    ),
                );
            }
            if !strings.is_empty() {
                e.insert("string_channels".into(), json!(strings));
            }
            let mut layout = Vec::new();
            if self.parsed.interleaved {
                layout.push("interleaved");
            }
            if self.parsed.big_endian {
                layout.push("big-endian");
            }
            if self.parsed.daqmx {
                layout.push("DAQmx raw data");
            }
            if !layout.is_empty() {
                e.insert("layout".into(), json!(layout));
            }
            if let Some(w) = writer_of(&root) {
                e.insert("writer".into(), json!(w));
            }
        }
        let mut notes = self.notes.clone();
        if traces.is_empty() && !strings.is_empty() {
            notes.push(
                "the file holds string channels only: they are listed in `vendor`, not returned as traces"
                    .into(),
            );
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.len,
            format: self.descriptor.clone(),
            format_version: self.parsed.versions.iter().max().map(u32::to_string),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut objects = serde_json::Map::new();
        for o in &self.parsed.objects {
            let mut e = json!({"properties": props_json(o)});
            if let Some(t) = o.dtype {
                e["data_type"] = json!(type_name(t));
                e["value_count"] = json!(o.count);
            }
            if o.dtype == Some(T_STRING) {
                let first = values::strings(&self.file, o, 0, o.count.min(20)).unwrap_or_default();
                e["first_values"] = json!(first);
            }
            objects.insert(o.path.clone(), e);
        }
        Ok(json!({"objects": objects, "segments": self.parsed.segments}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for k in ["traces", "format_version"] {
            m.insert(k.into(), Source::VendorImpl);
        }
        m.insert("traces[].sample_rate_hz".into(), Source::VendorImpl);
        m.insert("traces[].channels[].scale".into(), Source::Inferred);
        m.insert("traces[].channels[].extra.scaling".into(), Source::Inferred);
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .parsed
            .objects
            .iter()
            .take(2000)
            .map(|o| LsEntry {
                kind: if o.channel.is_some() {
                    "channel"
                } else if o.group.is_some() {
                    "group"
                } else {
                    "file"
                }
                .into(),
                name: o.path.clone(),
                offset: o.pieces.first().map(|p| p.at),
                size: None,
                image: None,
                details: json!({
                    "data_type": o.dtype.map(type_name),
                    "values": o.count,
                    "properties": o.props.len(),
                }),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FMT,
            "image planes",
            "TDMS files hold channels of values: use `openreadout trace` or `export --format csv --trace N`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let t = self.traces.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.traces.len()
            ))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (TDMS traces have one sweep)"
            )));
        }
        if first_sample > t.count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of trace {index} ({} samples)",
                t.count
            )));
        }
        let n = max_samples.min(t.count - first_sample).min(MAX_SAMPLES);
        let mut channels = Vec::with_capacity(t.channels.len());
        for &id in &t.channels {
            let o = &self.parsed.objects[id];
            let mut v = values::raw(&self.file, o, first_sample, n)
                .map_err(|e| Error::corrupt(FMT, format!("channel {}: {e}", o.path)))?;
            if let Some(c) = self.scales.get(&id) {
                values::apply(c, &mut v);
            }
            channels.push(v);
        }
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut rep = self.check_headers()?;
        rep.performed("read every value of every numeric channel and every string");
        for o in &self.parsed.objects {
            if o.channel.is_none() {
                continue;
            }
            let got = if o.dtype == Some(T_STRING) {
                values::strings(&self.file, o, 0, o.count).map(|v| v.len() as u64)
            } else if values::numeric(o) {
                let mut total = 0u64;
                let mut first = 0u64;
                let mut res = Ok(0);
                while first < o.count {
                    let n = (o.count - first).min(MAX_SAMPLES);
                    match values::raw(&self.file, o, first, n) {
                        Ok(v) => total += v.len() as u64,
                        Err(e) => {
                            res = Err(e);
                            break;
                        }
                    }
                    first += n;
                }
                res.map(|_: u64| total)
            } else {
                continue;
            };
            match got {
                Ok(n) if n == o.count => {}
                Ok(n) => rep.push(Finding::error(
                    "value_count",
                    format!("channel {}: {n} values read, {} indexed", o.path, o.count),
                )),
                Err(e) => rep.push(Finding::error(
                    "bad_values",
                    format!("channel {}: {e}", o.path),
                )),
            }
        }
        Ok(rep)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), FMT);
        rep.performed("read every segment's lead-in and metadata");
        if self.parsed.truncated {
            rep.push(Finding::error(
                "truncated",
                "the file ends inside a segment, or a segment is damaged: the values before it are read",
            ));
        }
        for o in &self.parsed.objects {
            if let Some(p) = &o.problem {
                rep.push(Finding::warning(
                    "channel_not_read",
                    format!("channel {}: {p}", o.path),
                ));
            }
        }
        Ok(rep)
    }
}

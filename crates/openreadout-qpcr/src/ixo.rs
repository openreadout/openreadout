//! Roche LightCycler 480 experiment files (`.ixo`): an XML object stream
//! (`<objectstream signature="IXOS">`) of `<obj name class>` / `<prop name>` / `<list name
//! count>` elements, followed by one checksum line. The run's fluorescence readings are an
//! embedded object stream (base64 of a zlib stream); the analyses keep the vendor's crossing
//! points (Cp) and calls per plate position. Provenance: `docs/provenance/qpcr.md` (2026-09-26).

use std::collections::BTreeMap;

use roxmltree::Node;
use serde_json::{Value, json};

use openreadout_core::{Error, Result};

use crate::model::{
    Assay, Curve, Dialect, Melt, Program, QpcrData, Reaction, Run, Sample, Stage, Step, Target, num,
};
use crate::xml::{node_text, parse};

pub(crate) const FORMAT_ID: &str = "roche-lightcycler-ixo";

/// Largest decoded acquisition store accepted (the corpus's are 0.3 MB for 96 wells × 136 reads).
const MAX_STORE: usize = 256 << 20;

/// The `<prop name="…">` child's trimmed text.
fn prop(n: Node<'_, '_>, name: &str) -> Option<String> {
    n.children()
        .find(|c| {
            c.is_element() && c.tag_name().name() == "prop" && c.attribute("name") == Some(name)
        })
        .and_then(node_text)
}

fn prop_num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    prop(n, name).as_deref().and_then(num)
}

/// The `<obj name="…">` child.
fn obj<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| {
        c.is_element() && c.tag_name().name() == "obj" && c.attribute("name") == Some(name)
    })
}

/// The element children of the `<list name="…">` child (none when absent).
fn items<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Vec<Node<'a, 'i>> {
    n.children()
        .find(|c| {
            c.is_element() && c.tag_name().name() == "list" && c.attribute("name") == Some(name)
        })
        .map(|l| l.children().filter(Node::is_element).collect())
        .unwrap_or_default()
}

fn class<'a>(n: Node<'a, '_>) -> &'a str {
    n.attribute("class").unwrap_or("")
}

/// Strict base64 (standard alphabet, `=` padding; whitespace ignored).
fn base64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        Some(u32::from(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        }))
    }
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for (i, q) in clean.chunks(4).enumerate() {
        let last = i + 1 == clean.len() / 4;
        let pad = q.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut v = 0u32;
        for &b in &q[..4 - pad] {
            v = (v << 6) | val(b)?;
        }
        v <<= 6 * u32::try_from(pad).ok()?;
        let bytes = v.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - pad]);
    }
    Some(out)
}

/// `$` + 16 hex digits: a big-endian f64 (acquisition temperatures).
fn hex_f64(s: &str) -> Option<f64> {
    let h = s.trim().strip_prefix('$')?;
    if h.len() != 16 {
        return None;
    }
    u64::from_str_radix(h, 16)
        .ok()
        .map(f64::from_bits)
        .filter(|v| v.is_finite())
}

/// One detection channel (`<excitation>-<emission>` nm).
#[derive(Debug, Clone)]
struct Channel {
    name: String,
    excitation: Option<f64>,
    emission: Option<f64>,
}

/// One `THTCFloAcquisition`.
struct Read {
    channel: usize,
    temp: Option<f64>,
    /// `ScalingFactor / (RefValue × IntgrTime)`: the LightCycler 480 software's raw-data export
    /// is the stored reading times this factor.
    export_scale: Option<f64>,
    values: Vec<f64>,
}

/// One `TCycle` of the acquisition store.
struct CycleReads {
    program: usize,
    cycle: u32,
    reads: Vec<Read>,
}

/// Decode the embedded acquisition store.
fn acquisitions(b64: &str, wells: usize) -> Result<(Vec<CycleReads>, usize)> {
    let packed =
        base64(b64).ok_or_else(|| Error::corrupt(FORMAT_ID, "AcquisitionStore is not base64"))?;
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&packed, MAX_STORE).map_err(
        |e| Error::corrupt(FORMAT_ID, format!("AcquisitionStore does not inflate: {e}")),
    )?;
    let text = openreadout_core::zip::text(&raw);
    let doc = parse(&text, FORMAT_ID, "AcquisitionStore")?;
    let root = doc.root_element();
    if class(root) != "HTCAcquisitionStore" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "AcquisitionStore holds a {:?}, not an HTCAcquisitionStore",
                class(root)
            ),
        ));
    }
    let count = prop_num(root, "SampleCount").map_or(wells, |v| v as usize);
    let mut out = Vec::new();
    let mut invalid = 0usize;
    for c in items(root, "Cycles") {
        let (Some(program), Some(cycle)) = (prop_num(c, "Program"), prop_num(c, "Cycle")) else {
            return Err(Error::corrupt(
                FORMAT_ID,
                "a TCycle without Program or Cycle",
            ));
        };
        let mut reads = Vec::new();
        for a in items(c, "Acquisitions") {
            let bytes = prop(a, "FloPoints")
                .as_deref()
                .and_then(base64)
                .ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, "an acquisition without readable FloPoints")
                })?;
            if bytes.len() != count * 4 {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "an acquisition holds {} bytes of readings for {count} plate positions (4 each)",
                        bytes.len()
                    ),
                ));
            }
            let valid = prop(a, "Valid").as_deref() == Some("1");
            if !valid {
                invalid += 1;
            }
            let export_scale = match (
                prop_num(a, "ScalingFactor"),
                prop(a, "RefValue").as_deref().and_then(hex_f64),
                prop_num(a, "IntgrTime"),
            ) {
                (Some(s), Some(r), Some(t)) => Some(s / (r * t)).filter(|v| v.is_finite()),
                _ => None,
            };
            reads.push(Read {
                channel: prop_num(a, "Channel").map_or(usize::MAX, |v| v as usize),
                temp: prop(a, "Temp").as_deref().and_then(hex_f64),
                export_scale,
                values: bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| {
                        let v = f64::from(f32::from_le_bytes(*b));
                        if valid { v } else { f64::NAN }
                    })
                    .collect(),
            });
        }
        out.push(CycleReads {
            program: program as usize,
            cycle: cycle as u32,
            reads,
        });
    }
    Ok((out, invalid))
}

/// Our task for a LightCycler 480 quantification sample type.
fn task(t: &str) -> String {
    match t {
        "qsUnknown" => "unknown".into(),
        "qsStandard" => "standard".into(),
        "qsNegative" => "negative".into(),
        "qsPositive" => "positive".into(),
        other => other.to_string(),
    }
}

/// Per position: sample name; per channel index: (target, task, concentration).
type SampleInfo = BTreeMap<
    u32,
    (
        Option<String>,
        BTreeMap<usize, (Option<String>, Option<String>, Option<f64>)>,
    ),
>;

fn sample_info(root: Node<'_, '_>) -> SampleInfo {
    let mut out = SampleInfo::new();
    let Some(list) = obj(root, "AnlsAndSampleInfo").and_then(|a| obj(a, "SampleInfoList")) else {
        return out;
    };
    for s in items(list, "emlist") {
        let Some(pos) = prop_num(s, "ContainerPosition").map(|v| v as u32) else {
            continue;
        };
        let mut name = None;
        if let Some(sp) = obj(s, "SampleProperties") {
            for p in items(sp, "emlist") {
                if class(p) == "GenSampleEditName" {
                    name = prop(p, "name");
                }
            }
        }
        let mut per = BTreeMap::new();
        if let Some(cp) = obj(s, "ChannelProperties").and_then(|c| obj(c, "ChannelProperties")) {
            for pl in items(cp, "emlist") {
                let mut idx = None;
                let (mut target, mut kind, mut conc) = (None, None, None);
                for p in items(pl, "emlist") {
                    if let Some(i) = obj(p, "ChannelProp").and_then(|c| prop_num(c, "ChannelIdx")) {
                        idx = Some(i as usize);
                    }
                    match class(p) {
                        "TargetName" => target = prop(p, "name"),
                        "QuantSampleTypeProperty" => kind = prop(p, "SampleType").map(|t| task(&t)),
                        "QuantConcProperty" => conc = prop_num(p, "Concentration"),
                        _ => {}
                    }
                }
                if let Some(i) = idx {
                    per.insert(i, (target, kind, conc));
                }
            }
        }
        out.insert(pos, (name, per));
    }
    out
}

/// One vendor analysis result per plate position.
#[derive(Debug, Clone)]
struct Called {
    included: bool,
    call: Option<i64>,
    cp: Option<f64>,
}

/// Parse a `.ixo` document.
pub(crate) fn parse_ixo(bytes: &[u8]) -> Result<QpcrData> {
    let text = openreadout_core::zip::text(bytes);
    let end = text
        .rfind("</objectstream>")
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no </objectstream> end tag"))?;
    let (body, trailer) = text.split_at(end + "</objectstream>".len());
    let doc = parse(body, FORMAT_ID, ".ixo object stream")?;
    let os = doc.root_element();
    if os.tag_name().name() != "objectstream" || os.attribute("signature") != Some("IXOS") {
        return Err(Error::corrupt(FORMAT_ID, "not an IXOS object stream"));
    }
    let root = obj(os, "root")
        .filter(|r| class(*r) == "HTCExperiment")
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no HTCExperiment root object"))?;
    let run = obj(root, "run").ok_or_else(|| {
        Error::unsupported(
            FORMAT_ID,
            "a LightCycler 480 file without a run (a template or macro)",
            "Only experiment files of a completed run hold readings; open the experiment in the LightCycler 480 software.",
        )
    })?;
    let mut d = QpcrData::new(Dialect::Ixo);
    d.name = prop(root, "name");
    d.created_at = prop(root, "Created");
    d.operator = prop(run, "Technician").or_else(|| prop(root, "CreatedByName"));
    d.started_at = prop(run, "StartTime");
    d.ended_at = prop(run, "EndTime");
    d.instrument.manufacturer = Some("Roche".into());
    // the run's instrument name: a LightCycler model as the software names it, or whatever the
    // lab typed (one lab's runs hold a serial number); only a LightCycler name is a model
    let instrument_name = prop(run, "InstrumentName");
    d.instrument.model = instrument_name
        .clone()
        .filter(|n| n.to_ascii_lowercase().contains("lightcycler"));
    d.instrument.serial_number = prop(run, "InstrumentID");
    d.instrument.software = Some("LightCycler 480 software".into());
    d.instrument.software_version =
        prop(root, "SWVersion").map(|v| v.trim_start_matches("LCS480").trim().to_string());
    let protocol = obj(run, "Protocol");
    // the program list also holds the plate (Container), the detection formats and the volume
    let plist = protocol.and_then(|p| obj(p, "Programs"));
    let programs = plist.map(|p| items(p, "emlist")).unwrap_or_default();
    let (rows, cols) = plist
        .and_then(|p| obj(p, "Container"))
        .map_or((8, 12), |c| {
            (
                prop_num(c, "RowCount").map_or(8, |v| v as u32),
                prop_num(c, "ColCount").map_or(12, |v| v as u32),
            )
        });
    let wells = (rows as usize).saturating_mul(cols as usize);
    if wells == 0 || wells > 1536 {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("a {rows} × {cols} plate"),
        ));
    }
    // detection channels of the format in use
    let formats: Vec<Node<'_, '_>> = plist
        .and_then(|p| obj(p, "DetectionFormats"))
        .map(|f| items(f, "emlist"))
        .unwrap_or_default();
    let def = plist
        .and_then(|p| obj(p, "DetectionFormats"))
        .and_then(|f| prop(f, "DefFormatName"));
    let format = formats
        .iter()
        .find(|f| prop(**f, "name") == def)
        .or_else(|| (formats.len() == 1).then(|| &formats[0]))
        .ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("{} detection formats and none named by DefFormatName", formats.len()),
                "The file's detection format could not be chosen; export the run from the LightCycler 480 software (text or RDML).",
            )
        })?;
    let channels: Vec<Channel> = items(*format, "emlist")
        .into_iter()
        .filter(|c| class(*c) == "HTCChannel")
        .map(|c| Channel {
            name: prop(c, "name").unwrap_or_default(),
            excitation: prop_num(c, "ExcitationWL"),
            emission: prop_num(c, "EmissionWL"),
        })
        .collect();
    // thermal protocol: one stage per LightCycler program
    let mut stages = Vec::new();
    let mut modes = Vec::new();
    for p in &programs {
        let mode = prop(*p, "AnalysisMode").unwrap_or_default();
        let repeats = prop_num(*p, "Cycles").map_or(1, |v| v as u32);
        let mut steps = Vec::new();
        let mut prev: Option<f64> = None;
        for s in obj(*p, "Segments")
            .map(|s| items(s, "emlist"))
            .unwrap_or_default()
        {
            let target = prop_num(s, "Target");
            let acq = prop(s, "AcquisitionMode").unwrap_or_default();
            let mut st = Step {
                kind: "temperature".into(),
                temperature_c: target,
                hold_s: prop_num(s, "Hold"),
                ramp_c_per_s: prop_num(s, "Slope"),
                ..Step::default()
            };
            match acq.as_str() {
                "1" => st.measure = Some("real time".into()),
                "2" => {
                    st.kind = "gradient".into();
                    st.measure = Some("melt".into());
                    st.low_temperature_c = prev;
                    st.high_temperature_c = target;
                }
                _ => {}
            }
            prev = target;
            steps.push(st);
        }
        stages.push(Stage {
            kind: match mode.as_str() {
                "Quantification" => "cycling".into(),
                "Melting Curves" => "melt".into(),
                _ => "hold".into(),
            },
            repeats,
            steps,
        });
        modes.push((prop(*p, "name").unwrap_or_default(), mode));
    }
    d.programs.push(Program {
        name: "LightCycler 480 protocol".into(),
        sample_volume_ul: plist.and_then(|p| prop_num(p, "SampleVolume")),
        stages,
        ..Program::default()
    });
    // readings
    let store = prop(run, "AcquisitionStore");
    let (cycles, invalid) = match &store {
        Some(s) => acquisitions(s, wells)?,
        None => (Vec::new(), 0),
    };
    if invalid > 0 {
        d.notes.push(format!(
            "{invalid} acquisitions are marked not valid by the instrument: their readings are NaN"
        ));
    }
    if cycles
        .iter()
        .flat_map(|c| &c.reads)
        .any(|r| r.channel >= channels.len())
    {
        return Err(Error::corrupt(
            FORMAT_ID,
            "an acquisition names a channel the detection format does not have",
        ));
    }
    let quant: Vec<usize> = modes
        .iter()
        .enumerate()
        .filter(|(_, m)| m.1 == "Quantification")
        .map(|(i, _)| i)
        .collect();
    let melts: Vec<usize> = modes
        .iter()
        .enumerate()
        .filter(|(_, m)| m.1 == "Melting Curves")
        .map(|(i, _)| i)
        .collect();
    // amplification: per quantification program, per channel, per position, one value per cycle
    let mut amp: BTreeMap<usize, (Vec<f64>, Vec<Vec<Vec<f64>>>)> = BTreeMap::new();
    // per quantification program and channel, each cycle's export scale
    let mut amp_scale: BTreeMap<usize, Vec<Vec<Option<f64>>>> = BTreeMap::new();
    for &p in &quant {
        let mut these: Vec<&CycleReads> = cycles.iter().filter(|c| c.program == p).collect();
        if these.is_empty() {
            continue;
        }
        these.sort_by_key(|c| c.cycle);
        let per_cycle_ok = these.iter().all(|c| {
            (0..channels.len()).all(|ch| c.reads.iter().filter(|r| r.channel == ch).count() == 1)
        }) && these.windows(2).all(|w| w[1].cycle == w[0].cycle + 1);
        if !per_cycle_ok {
            d.notes.push(format!(
                "program {:?}: its cycles do not hold exactly one reading per channel in consecutive cycles; its readings are not read",
                modes[p].0
            ));
            continue;
        }
        let xs: Vec<f64> = these.iter().map(|c| f64::from(c.cycle) + 1.0).collect();
        let mut data = vec![vec![Vec::with_capacity(xs.len()); wells]; channels.len()];
        let mut scale = vec![Vec::with_capacity(xs.len()); channels.len()];
        for c in &these {
            for r in &c.reads {
                for (w, v) in r.values.iter().enumerate().take(wells) {
                    data[r.channel][w].push(*v);
                }
                scale[r.channel].push(r.export_scale);
            }
        }
        amp.insert(p, (xs, data));
        amp_scale.insert(p, scale);
    }
    // melt: per program, per channel, (temperatures, per position readings)
    let mut melt: BTreeMap<usize, Vec<(Vec<f64>, Vec<Vec<f64>>)>> = BTreeMap::new();
    for &p in &melts {
        let reads: Vec<&Read> = cycles
            .iter()
            .filter(|c| c.program == p)
            .flat_map(|c| &c.reads)
            .collect();
        if reads.is_empty() {
            continue;
        }
        let mut per = vec![(Vec::new(), vec![Vec::new(); wells]); channels.len()];
        for r in reads {
            let Some(t) = r.temp else {
                continue;
            };
            per[r.channel].0.push(t);
            for (w, v) in r.values.iter().enumerate().take(wells) {
                per[r.channel].1[w].push(*v);
            }
        }
        melt.insert(p, per);
    }
    // vendor analyses
    let info = sample_info(root);
    let mut results: BTreeMap<(usize, u32), Called> = BTreeMap::new();
    let mut analyses = Vec::new();
    let mut analysed_channel: Option<usize> = None;
    for a in obj(root, "analyses")
        .map(|a| items(a, "emlist"))
        .unwrap_or_default()
    {
        let name = prop(a, "name");
        let mut summary = json!({"class": class(a), "name": name});
        let samples: Vec<Node<'_, '_>> = obj(a, "AnaSamples")
            .map(|s| items(s, "emlist"))
            .unwrap_or_default()
            .into_iter()
            .filter(|s| class(*s) == "QuantSampleB")
            .collect();
        let ratio = obj(a, "ChannelRatio");
        let num_ch = ratio.and_then(|r| {
            let (x, e) = (prop_num(r, "NumeratorXWL"), prop_num(r, "NumeratorEWL"));
            channels
                .iter()
                .position(|c| c.excitation == x && c.emission == e && x.is_some())
        });
        let denom = ratio
            .and_then(|r| prop_num(r, "DenominatorEWL"))
            .unwrap_or(0.0);
        let program = obj(a, "ProgramInfo")
            .and_then(|p| prop_num(p, "SelectedProgram"))
            .map(|v| v as usize);
        summary["positions"] = json!(samples.len());
        if let Some(ch) = num_ch {
            summary["channel"] = json!(channels[ch].name);
        }
        if let Some(p) = program.and_then(|p| modes.get(p)) {
            summary["program"] = json!(p.0);
        }
        let usable = class(a) == "Legacy Absolute Quantification Analysis"
            && denom == 0.0
            && program.is_some_and(|p| quant.contains(&p))
            && num_ch.is_some();
        if !samples.is_empty() && usable && analysed_channel.is_none_or(|c| Some(c) == num_ch) {
            let Some(ch) = num_ch else { continue };
            if results.keys().any(|k| k.0 == ch) {
                summary["read"] = json!(false);
                d.notes.push(format!(
                    "analysis {name:?}: a second absolute-quantification analysis of channel {} is kept in the vendor tree only",
                    channels[ch].name
                ));
            } else {
                analysed_channel = Some(ch);
                summary["read"] = json!(true);
                for s in samples {
                    let Some(pos) = prop_num(s, "Pos").map(|v| v as u32) else {
                        continue;
                    };
                    results.insert(
                        (ch, pos),
                        Called {
                            included: prop(s, "IsIncluded").as_deref() != Some("0"),
                            call: prop(s, "Call").and_then(|c| c.parse().ok()),
                            cp: prop_num(s, "CrossingPoint"),
                        },
                    );
                }
            }
        } else {
            summary["read"] = json!(false);
            if !samples.is_empty() || class(a) != "Legacy Absolute Quantification Analysis" {
                d.notes.push(format!(
                    "analysis {name:?} ({}) is listed in the vendor tree, not read: only absolute quantification of one channel (no ratio) of a quantification program is read",
                    class(a)
                ));
            }
        }
        analyses.push(summary);
    }
    // reactions
    let single_melt = (melts.len() == 1).then(|| melts[0]);
    let main_amp = (quant.len() == 1)
        .then(|| quant[0])
        .and_then(|p| amp.get(&p));
    let export_scale: Option<Value> = (quant.len() == 1)
        .then(|| quant[0])
        .and_then(|p| amp_scale.get(&p))
        .map(|per| {
            Value::Object(
                channels
                    .iter()
                    .zip(per)
                    .map(|(c, v)| (c.name.clone(), json!(v)))
                    .collect(),
            )
        });
    if quant.len() > 1 {
        d.notes.push(format!(
            "{} quantification programs: readings are not attached (the model holds one amplification curve per well and channel)",
            quant.len()
        ));
    }
    // (target, the channel measuring it)
    let mut targets: Vec<(String, String)> = Vec::new();
    let mut reactions = Vec::new();
    let mut other_calls = 0usize;
    for pos in 0..wells as u32 {
        let (name, per) = info.get(&pos).cloned().unwrap_or_default();
        let mut rx = Reaction {
            position: pos,
            row: pos / cols,
            column: pos % cols,
            sample: name.clone(),
            ..Reaction::default()
        };
        for (ch, c) in channels.iter().enumerate() {
            let (target, kind, conc) = per.get(&ch).cloned().unwrap_or_default();
            // a channel without a target name is measured under the channel's own name
            let target = target.or_else(|| Some(c.name.clone()));
            if let Some(t) = &target
                && !targets.iter().any(|x| &x.0 == t)
            {
                targets.push((t.clone(), c.name.clone()));
            }
            let mut a = Assay {
                target,
                dye: Some(c.name.clone()),
                quantity: conc.filter(|q| *q > 0.0 && kind.as_deref() == Some("standard")),
                task: kind,
                ..Assay::default()
            };
            if let Some((xs, data)) = main_amp
                && let Some(v) = data.get(ch).and_then(|d| d.get(pos as usize))
            {
                a.amplification = Some(Curve {
                    cycles: xs.clone(),
                    fluorescence: v.clone(),
                    corrected: None,
                    quantity: "fluorescence",
                });
            }
            if let Some(p) = single_melt
                && let Some((t, v)) = melt.get(&p).and_then(|m| m.get(ch))
                && let Some(v) = v.get(pos as usize)
                && !t.is_empty()
            {
                a.melt = Some(Melt {
                    temperature: t.clone(),
                    fluorescence: v.clone(),
                    derivative: None,
                });
            }
            if let Some(r) = results.get(&(ch, pos)) {
                if !r.included {
                    a.excluded = Some("not included in the analysis".into());
                }
                match r.call {
                    Some(2) => {
                        a.cq = r.cp.filter(|v| *v > 0.0);
                        a.amp_status = Some("amplified".into());
                    }
                    Some(0) => {
                        a.cq_undetermined = true;
                        a.amp_status = Some("not amplified".into());
                        a.cq_stored = r.cp.filter(|v| *v != 0.0);
                    }
                    other => {
                        other_calls += 1;
                        a.cq_stored = r.cp;
                        a.flags.push(format!(
                            "lc480_call_{}",
                            other.map_or_else(|| "missing".to_string(), |c| c.to_string())
                        ));
                    }
                }
            }
            if a.amplification.is_some() || a.melt.is_some() || results.contains_key(&(ch, pos)) {
                rx.assays.push(a);
            }
        }
        if rx.assays.is_empty() && name.is_none() {
            continue;
        }
        reactions.push(rx);
    }
    if other_calls > 0 {
        d.notes.push(format!(
            "{other_calls} positions have an analysis call other than 0 (negative) or 2 (positive): no Cq is reported for them (the Cp is kept as cq_stored)"
        ));
    }
    for (name, _) in info.values() {
        if let Some(n) = name
            && !d.samples.iter().any(|s| &s.name == n)
        {
            d.samples.push(Sample {
                name: n.clone(),
                ..Sample::default()
            });
        }
    }
    for (t, dye) in targets {
        d.targets.push(Target {
            name: t,
            dye: Some(dye),
            ..Target::default()
        });
    }
    d.runs.push(Run {
        name: prop(run, "name").unwrap_or_else(|| "Run".into()),
        instrument: d.instrument.model.clone(),
        started_at: d.started_at.clone(),
        program: Some(0),
        rows,
        columns: cols,
        row_label: "ABC".into(),
        column_label: "123".into(),
        cq_method: analysed_channel.and_then(|_| {
            obj(root, "analyses")
                .map(|a| items(a, "emlist"))
                .unwrap_or_default()
                .into_iter()
                .find(|a| class(*a) == "Legacy Absolute Quantification Analysis")
                .and_then(|a| prop(a, "name"))
        }),
        reactions,
        ..Run::default()
    });
    // melt programs beyond one: each its own run of melt curves
    if melts.len() > 1 {
        for &p in &melts {
            let Some(per) = melt.get(&p) else { continue };
            let mut reactions = Vec::new();
            for pos in 0..wells as u32 {
                let mut rx = Reaction {
                    position: pos,
                    row: pos / cols,
                    column: pos % cols,
                    sample: info.get(&pos).and_then(|i| i.0.clone()),
                    ..Reaction::default()
                };
                for (ch, c) in channels.iter().enumerate() {
                    let Some((t, v)) = per.get(ch) else { continue };
                    let Some(v) = v.get(pos as usize).filter(|_| !t.is_empty()) else {
                        continue;
                    };
                    rx.assays.push(Assay {
                        target: info
                            .get(&pos)
                            .and_then(|i| i.1.get(&ch))
                            .and_then(|x| x.0.clone())
                            .or_else(|| Some(c.name.clone())),
                        dye: Some(c.name.clone()),
                        melt: Some(Melt {
                            temperature: t.clone(),
                            fluorescence: v.clone(),
                            derivative: None,
                        }),
                        ..Assay::default()
                    });
                }
                if !rx.assays.is_empty() {
                    reactions.push(rx);
                }
            }
            d.runs.push(Run {
                name: modes[p].0.clone(),
                instrument: d.instrument.model.clone(),
                started_at: d.started_at.clone(),
                program: Some(0),
                rows,
                columns: cols,
                row_label: "ABC".into(),
                column_label: "123".into(),
                reactions,
                ..Run::default()
            });
        }
        d.notes.push(format!(
            "{} melting programs: each is its own run of melt curves ({})",
            melts.len(),
            melts
                .iter()
                .map(|p| modes[*p].0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    d.notes.push("LightCycler 480 fluorescence is reported as the instrument stores it (raw instrument units, no color compensation)".into());
    let calculators: Vec<Value> = obj(root, "CalcSrc")
        .and_then(|c| obj(c, "CalcList"))
        .map(|l| items(l, "Items"))
        .unwrap_or_default()
        .into_iter()
        .map(|c| json!({"class": class(c), "version": prop(c, "CalcVersion")}))
        .collect();
    let trailer = trailer.trim();
    d.vendor = json!({
        "experiment": d.name,
        "software_version": prop(root, "SWVersion"),
        "macro": prop(root, "MacroName"),
        "instrument_id": prop(run, "InstrumentID"),
        "instrument_name": instrument_name,
        "plate_id": plist.and_then(|p| prop(p, "PlateID")),
        "detection_format": prop(*format, "name"),
        "channels": channels.iter().map(|c| json!({"name": c.name, "excitation_nm": c.excitation, "emission_nm": c.emission})).collect::<Vec<_>>(),
        "programs": modes.iter().map(|(n, m)| json!({"name": n, "analysis_mode": m})).collect::<Vec<_>>(),
        "analyses": analyses,
        "calculators": calculators,
        "trailer": (!trailer.is_empty()).then_some(trailer),
        "export_scale": export_scale,
    });
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in bytes.chunks(3) {
            let v = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() {
                    s.push(char::from(A[((v >> (18 - 6 * i)) & 63) as usize]));
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    fn floats(v: &[f32]) -> String {
        b64(&v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>())
    }

    #[test]
    fn base64_round_trips() {
        for n in 0..10u8 {
            let v: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(base64(&b64(&v)).unwrap(), v);
        }
        assert!(base64("abc").is_none());
        assert!(base64("a=bc").is_none());
        assert_eq!(
            hex_f64("$4051FAE147AE147B").map(|t| (t * 100.0).round()),
            Some(7192.0)
        );
    }

    fn ixo(calls: &[(u32, u32, f64)]) -> Vec<u8> {
        // 1 × 2 plate, one channel, 3 cycles and one melt with 2 reads
        let mut cyc = String::new();
        for (c, v) in [[1.0f32, 5.0], [1.1, 6.0], [1.2, 9.0]].iter().enumerate() {
            cyc.push_str(&format!(
                r#"<obj name="Item" class="TCycle" version="1"><prop name="Program">0</prop><prop name="Segment">1</prop><prop name="Cycle">{c}</prop><list name="Acquisitions" count="1"><obj name="Item" class="THTCFloAcquisition" version="3"><prop name="Temp">$404D000000000000</prop><prop name="Channel">0</prop><prop name="IntgrTime">400</prop><prop name="Valid">1</prop><prop name="RefValue">$40B3880000000000</prop><prop name="ScalingFactor">20000</prop><prop name="FloPoints">{}</prop></obj></list></obj>"#,
                floats(v)
            ));
        }
        cyc.push_str(&format!(
            r#"<obj name="Item" class="TCycle" version="1"><prop name="Program">1</prop><prop name="Segment">2</prop><prop name="Cycle">0</prop><list name="Acquisitions" count="2"><obj name="Item" class="THTCFloAcquisition" version="3"><prop name="Temp">$4050000000000000</prop><prop name="Channel">0</prop><prop name="Valid">1</prop><prop name="FloPoints">{}</prop></obj><obj name="Item" class="THTCFloAcquisition" version="3"><prop name="Temp">$4050400000000000</prop><prop name="Channel">0</prop><prop name="Valid">1</prop><prop name="FloPoints">{}</prop></obj></list></obj>"#,
            floats(&[3.0, 4.0]),
            floats(&[2.0, 3.0])
        ));
        let store = format!(
            r#"<obj name="root" class="HTCAcquisitionStore" version="1"><prop name="ChannelCount">1</prop><prop name="SampleCount">2</prop><list name="Cycles" count="4">{cyc}</list></obj>"#
        );
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(store.as_bytes(), 6);
        let mut qs = String::new();
        for (pos, call, cp) in calls {
            qs.push_str(&format!(r#"<obj name="item" class="QuantSampleB" version="2"><prop name="IsIncluded">1</prop><prop name="Pos">{pos}</prop><prop name="Call">{call}</prop><prop name="CrossingPoint">{cp}</prop></obj>"#));
        }
        format!(
            r#"<objectstream signature="IXOS" version="1">
<obj name="root" class="HTCExperiment" version="5"><prop name="name">T</prop><prop name="SWVersion">LCS480 1.5.1.62</prop>
<obj name="run" class="HTCRun" version="1"><prop name="name">Run</prop><prop name="StartTime">2023-06-26T09:20:12.455</prop><prop name="InstrumentName">LightCycler 480</prop>
<prop name="AcquisitionStore">{}</prop>
<obj name="Protocol" class="HTCProtocol" version="1"><obj name="Programs" class="HTCRunProgramList" version="1"><list name="emlist" count="2">
<obj name="item" class="HTCRunProgram" version="1"><prop name="name">Cycling</prop><prop name="Cycles">3</prop><prop name="AnalysisMode">Quantification</prop><obj name="Segments" class="TEMRunSegmentList" version="1"><list name="emlist" count="2">
<obj name="item" class="HTCRunSegment" version="2"><prop name="Hold">5</prop><prop name="Target">95</prop><prop name="AcquisitionMode">0</prop></obj>
<obj name="item" class="HTCRunSegment" version="2"><prop name="Hold">15</prop><prop name="Target">58</prop><prop name="AcquisitionMode">1</prop></obj></list></obj></obj>
<obj name="item" class="HTCRunProgram" version="1"><prop name="name">Melting</prop><prop name="Cycles">1</prop><prop name="AnalysisMode">Melting Curves</prop><obj name="Segments" class="TEMRunSegmentList" version="1"><list name="emlist" count="2">
<obj name="item" class="HTCRunSegment" version="2"><prop name="Target">65</prop><prop name="AcquisitionMode">0</prop></obj>
<obj name="item" class="HTCRunSegment" version="2"><prop name="Target">97</prop><prop name="AcquisitionMode">2</prop></obj></list></obj></obj>
</list>
<obj name="Container" class="HTCBlockType" version="1"><prop name="RowCount">1</prop><prop name="ColCount">2</prop></obj>
<obj name="DetectionFormats" class="DetectionFormats" version="2"><list name="emlist" count="1"><obj name="item" class="HTCDetectionFormat" version="1"><list name="emlist" count="1">
<obj name="item" class="HTCChannel" version="1"><prop name="name">498-640</prop><prop name="EmissionWL">640</prop><prop name="ExcitationWL">498</prop></obj></list><prop name="name">F</prop></obj></list><prop name="DefFormatName">F</prop></obj>
</obj></obj></obj>
<obj name="analyses" class="AnalysisList" version="1"><list name="emlist" count="1"><obj name="item" class="Legacy Absolute Quantification Analysis" version="1"><prop name="name">Abs Quant/2nd Derivative Max</prop>
<obj name="AnaSamples" class="TAnalysisSampleList" version="1"><list name="emlist" count="{}">{qs}</list></obj>
<obj name="ProgramInfo" class="ProgramInfo" version="1"><prop name="SelectedProgram">0</prop></obj>
<obj name="ChannelRatio" class="ChannelRatio" version="1"><prop name="NumeratorEWL">640</prop><prop name="NumeratorXWL">498</prop><prop name="DenominatorEWL">0</prop></obj></obj></list></obj>
</obj>
</objectstream>
$18E8ABDB-DA8A33BB-77973CA2-E2C7AE51
"#,
            b64(&packed),
            calls.len()
        )
        .into_bytes()
    }

    #[test]
    fn small_ixo() {
        let d = parse_ixo(&ixo(&[(0, 0, 0.0), (1, 2, 2.5)])).unwrap();
        assert_eq!(d.runs.len(), 1);
        let r = &d.runs[0];
        assert_eq!((r.rows, r.columns), (1, 2));
        let a0 = &r.reactions[0].assays[0];
        let a1 = &r.reactions[1].assays[0];
        assert_eq!(a0.dye.as_deref(), Some("498-640"));
        assert!(a0.cq.is_none() && a0.cq_undetermined);
        assert_eq!(a1.cq, Some(2.5));
        let amp = a1.amplification.as_ref().unwrap();
        assert_eq!(amp.cycles, vec![1.0, 2.0, 3.0]);
        assert_eq!(amp.fluorescence, vec![5.0, 6.0, 9.0]);
        let m = a1.melt.as_ref().unwrap();
        assert_eq!(m.temperature, vec![64.0, 65.0]);
        assert_eq!(m.fluorescence, vec![4.0, 3.0]);
        assert_eq!(d.programs[0].acquisition_temperature(), Some(58.0));
        assert_eq!(d.instrument.software_version.as_deref(), Some("1.5.1.62"));
        assert_eq!(d.vendor["trailer"], "$18E8ABDB-DA8A33BB-77973CA2-E2C7AE51");
        assert_eq!(d.instrument.model.as_deref(), Some("LightCycler 480"));
        // ScalingFactor 20000 / (RefValue 5000 × IntgrTime 400)
        assert_eq!(
            d.vendor["export_scale"]["498-640"],
            json!([0.01, 0.01, 0.01])
        );
    }

    #[test]
    fn an_instrument_name_that_is_not_a_model() {
        let f = String::from_utf8(ixo(&[])).unwrap().replace(
            "<prop name=\"InstrumentName\">LightCycler 480</prop>",
            "<prop name=\"InstrumentName\">29892</prop>",
        );
        let d = parse_ixo(f.as_bytes()).unwrap();
        assert_eq!(d.instrument.model, None);
        assert_eq!(d.runs[0].instrument, None);
        assert_eq!(d.vendor["instrument_name"], "29892");
    }

    #[test]
    fn unknown_calls_give_no_cq() {
        let d = parse_ixo(&ixo(&[(0, 1, 30.0)])).unwrap();
        let a = &d.runs[0].reactions[0].assays[0];
        assert!(a.cq.is_none() && !a.cq_undetermined);
        assert_eq!(a.cq_stored, Some(30.0));
        assert_eq!(a.flags, vec!["lc480_call_1".to_string()]);
    }

    #[test]
    fn malformed_files_are_errors() {
        assert!(parse_ixo(b"<objectstream signature=\"IXOS\">").is_err());
        assert!(parse_ixo(b"<x/></objectstream>").is_err());
        let mut f = ixo(&[]);
        assert!(parse_ixo(&f).is_ok());
        let s = String::from_utf8(f.clone()).unwrap();
        // a store that does not inflate
        let at = s.find("AcquisitionStore\">").unwrap() + 18;
        let bad = format!("{}AAAA{}", &s[..at], &s[at + 4..]);
        assert!(parse_ixo(bad.as_bytes()).is_err());
        f.truncate(f.len() / 2);
        assert!(parse_ixo(&f).is_err());
    }
}

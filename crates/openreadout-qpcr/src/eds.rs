//! Applied Biosystems experiment documents (`.eds`): the `apldbio/sds/` XML layout (QuantStudio,
//! ViiA 7), the 7500 / StepOne layout (`multicomponent_data.txt`) and the JSON layout (Design &
//! Analysis 2). Member names and meanings: `docs/formats/qpcr.md`; provenance:
//! `docs/provenance/qpcr.md`.

use std::collections::BTreeMap;

use roxmltree::Node;
use serde_json::{Value, json};

use openreadout_core::time::unix_to_iso8601;
use openreadout_core::{Error, Result};

use crate::model::{
    Assay, Curve, Dialect, DyeSignal, Melt, Program, QpcrData, Reaction, Run, Sample, Stage,
    StandardCurve, Step, Target, num, task_name,
};
use crate::xml::{child, children, node_text, parse, text};
use openreadout_core::zip::ZipIndex;

pub(crate) const FORMAT_ID: &str = "applied-biosystems-eds";

/// Which layout an archive uses, from its member names.
pub(crate) fn layout(z: &ZipIndex) -> Option<Dialect> {
    let json = z.has("setup/plate_setup.json")
        || z.has("primary/analysis_result.json")
        || z.has("primary/multicomponent_data.json");
    let sds = z.has("apldbio/sds/experiment.xml") || z.has("apldbio/sds/plate_setup.xml");
    if json {
        Some(Dialect::EdsJson)
    } else if z.has("apldbio/sds/multicomponent_data.txt") {
        Some(Dialect::Eds7500)
    } else if sds {
        Some(Dialect::EdsSds)
    } else {
        None
    }
}

/// Unix milliseconds → ISO-8601; `None` for zero or negative stamps.
fn ms_to_iso(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms <= 0.0 || ms > 1e15 {
        return None;
    }
    let secs = (ms / 1000.0).floor();
    let millis = (ms - secs * 1000.0).round().clamp(0.0, 999.0);
    Some(unix_to_iso8601(secs as i64, millis as u32))
}

/// JSON as the layout writes it: bare `NaN`/`Infinity` tokens (not valid JSON) become `null`.
pub(crate) fn lenient_json(text: &str, what: &str) -> Result<Value> {
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut in_str = false;
    let mut esc = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if let Some(tok) = [&b"-Infinity"[..], b"Infinity", b"NaN"]
            .into_iter()
            .find(|t| b[i..].starts_with(t))
        {
            out.extend_from_slice(b"null");
            i += tok.len();
            continue;
        }
        out.push(c);
        i += 1;
    }
    // Only ASCII tokens were replaced, so the bytes are still UTF-8.
    let fixed = String::from_utf8_lossy(&out);
    serde_json::from_str(&fixed)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("{what}: JSON does not parse: {e}")))
}

fn note(d: &mut QpcrData, what: &str, e: &Error) {
    d.notes.push(format!("{what} could not be read: {e}"));
}

fn js<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
fn jf(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite())
}
fn jarr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
fn jnums(v: &Value, k: &str) -> Option<Vec<f64>> {
    let a = v.get(k)?.as_array()?;
    Some(a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect())
}

/// `key: value` lines of a JAR-style manifest.
fn manifest(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Plate geometry from a block / plate type (`BLOCK_384W`, `TYPE_16X24`, `BLOCK_96W_02ML`).
fn geometry(kind: &str) -> Option<(u32, u32)> {
    let k = kind.to_ascii_uppercase();
    if let Some(rest) = k.strip_prefix("TYPE_")
        && let Some((r, c)) = rest.split_once('X')
    {
        let r: u32 = r.trim().parse().ok()?;
        let c: u32 = c
            .trim()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()?;
        return Some((r, c));
    }
    if k.contains("384") {
        Some((16, 24))
    } else if k.contains("96") {
        Some((8, 12))
    } else if k.contains("48") {
        Some((6, 8))
    } else if k.contains("1536") || k.contains("OA") && k.contains("3072") {
        Some((32, 48))
    } else {
        None
    }
}

/// Model name of a JSON-layout `instrumentType` (`QS7PRO` → `QuantStudio 7 Pro`).
fn json_instrument(t: &str) -> String {
    let u = t.to_ascii_uppercase();
    if let Some(rest) = u.strip_prefix("QS") {
        let (n, pro) = match rest.strip_suffix("PRO") {
            Some(n) => (n, true),
            None => (rest, false),
        };
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return format!("QuantStudio {n}{}", if pro { " Pro" } else { "" });
        }
    }
    t.to_string()
}

/// Per-well analysis settings from `analysis_protocol.xml`.
#[derive(Debug, Default, Clone)]
struct CtSettings {
    threshold: Option<f64>,
    auto_threshold: Option<bool>,
    baseline_start: Option<u32>,
    baseline_end: Option<u32>,
    auto_baseline: Option<bool>,
}

#[derive(Debug, Default)]
struct Protocol {
    /// Detector (target) name → settings; `""` = the defaults.
    detectors: BTreeMap<String, CtSettings>,
    /// (well index, detector) → settings that override the detector's (unless
    /// `UseDetectorDefaults`).
    wells: BTreeMap<(u32, String), CtSettings>,
    endogenous_control: Option<String>,
    calibrator: Option<String>,
    amp_stage: Option<u32>,
    melt_stage: Option<u32>,
}

fn setting_values(s: Node<'_, '_>) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for v in children(s, "JaxbSettingValue") {
        let Some(name) = text(v, "Name") else {
            continue;
        };
        let val = child(v, "JaxbValueItem")
            .and_then(|it| it.children().filter(Node::is_element).find_map(node_text));
        if let Some(val) = val {
            m.insert(name, val);
        }
    }
    m
}

fn parse_protocol(xml: &str) -> Result<Protocol> {
    let doc = parse(xml, FORMAT_ID, "analysis_protocol.xml")?;
    let mut p = Protocol::default();
    let b = |s: Option<&String>| s.map(|v| v.eq_ignore_ascii_case("true"));
    let u = |s: Option<&String>| s.and_then(|v| v.trim().parse::<u32>().ok());
    for s in children(doc.root_element(), "JaxbAnalysisSettings") {
        let ty = text(s, "Type").unwrap_or_default();
        let short = ty.rsplit('.').next().unwrap_or("").to_string();
        let v = setting_values(s);
        let object = v
            .get("ObjectName")
            .cloned()
            .filter(|o| !o.contains("DEFAULT_SETTINGS") && !o.contains("OBJECT_NAME"))
            .unwrap_or_default();
        let ct = CtSettings {
            threshold: v.get("Threshold").and_then(|x| num(x)),
            auto_threshold: b(v.get("AutoCt")),
            baseline_start: u(v.get("BaselineStart")),
            baseline_end: u(v.get("BaselineStop")),
            auto_baseline: b(v.get("AutoBaseline")),
        };
        match short.as_str() {
            "IDetectorSettings" => {
                p.detectors.insert(object, ct);
            }
            "IWellSettings" => {
                if let Some(w) = u(v.get("WellIndex"))
                    && !b(v.get("UseDetectorDefaults")).unwrap_or(false)
                {
                    p.wells.insert((w, object), ct);
                }
            }
            "IDDCtAnalysisSettings" => {
                p.endogenous_control = v.get("EndogenousControl").cloned();
                p.calibrator = v.get("Calibrator").cloned();
            }
            "IDataSelectSettings" => {
                p.amp_stage = u(v.get("StageNum"));
                p.melt_stage = u(v.get("MeltStageNum")).filter(|s| *s > 0);
            }
            _ => {}
        }
    }
    Ok(p)
}

impl Protocol {
    /// Settings in force for `target` in well `well`: the well's own, else the target's,
    /// else the defaults (fields merged in that order).
    fn for_well(&self, well: u32, target: &str) -> CtSettings {
        let mut out = CtSettings::default();
        let layers = [
            self.wells.get(&(well, target.to_string())),
            self.detectors.get(target),
            self.detectors.get(""),
        ];
        for l in layers.into_iter().flatten() {
            out.threshold = out.threshold.or(l.threshold);
            out.auto_threshold = out.auto_threshold.or(l.auto_threshold);
            out.baseline_start = out.baseline_start.or(l.baseline_start);
            out.baseline_end = out.baseline_end.or(l.baseline_end);
            out.auto_baseline = out.auto_baseline.or(l.auto_baseline);
        }
        out
    }
}

/// Plate setup of one well: sample and target assignments.
#[derive(Debug, Default, Clone)]
struct WellSetup {
    sample: Option<String>,
    omitted: bool,
    /// (target, reporter, task, quantity)
    targets: Vec<(String, Option<String>, Option<String>, Option<f64>)>,
}

/// The SDS `plate_setup.xml`.
fn parse_plate_xml(xml: &str, d: &mut QpcrData) -> Result<(u32, u32, BTreeMap<u32, WellSetup>)> {
    let doc = parse(xml, FORMAT_ID, "plate_setup.xml")?;
    let root = doc.root_element();
    let mut rows = crate::xml::number(root, "Rows").map_or(0, |v| v as u32);
    let mut cols = crate::xml::number(root, "Columns").map_or(0, |v| v as u32);
    if (rows == 0 || cols == 0)
        && let Some((r, c)) = child(root, "PlateKind")
            .and_then(|k| text(k, "Type"))
            .and_then(|t| geometry(&t))
    {
        rows = r;
        cols = c;
    }
    if let Some(p) = text(root, "PassiveReferenceDye").filter(|p| p != "NULL") {
        d.passive_reference = Some(p);
    }
    let mut wells: BTreeMap<u32, WellSetup> = BTreeMap::new();
    for fm in children(root, "FeatureMap") {
        let fid = child(fm, "Feature")
            .and_then(|f| text(f, "Id"))
            .unwrap_or_default();
        for fv in children(fm, "FeatureValue") {
            let Some(idx) = crate::xml::number(fv, "Index").map(|v| v as u32) else {
                continue;
            };
            let Some(item) = child(fv, "FeatureItem") else {
                continue;
            };
            match fid.as_str() {
                "sample" => {
                    if let Some(name) = child(item, "Sample").and_then(|s| text(s, "Name")) {
                        wells.entry(idx).or_default().sample = Some(name);
                    }
                }
                "detector-task" => {
                    for list in children(item, "DetectorTaskList") {
                        for dt in children(list, "DetectorTask") {
                            let det = child(dt, "Detector");
                            let Some(name) = det.and_then(|x| text(x, "Name")) else {
                                continue;
                            };
                            let reporter = det.and_then(|x| text(x, "Reporter"));
                            let quencher = det.and_then(|x| text(x, "Quencher"));
                            if !d.targets.iter().any(|t| t.name == name) {
                                d.targets.push(Target {
                                    name: name.clone(),
                                    dye: reporter.clone(),
                                    quencher: quencher.filter(|q| q != "None"),
                                    ..Target::default()
                                });
                            }
                            wells.entry(idx).or_default().targets.push((
                                name,
                                reporter,
                                text(dt, "Task").map(|t| task_name(&t)),
                                crate::xml::number(dt, "Concentration"),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(ws) = child(root, "Wells") {
        for w in children(ws, "Well") {
            if let Some(i) = crate::xml::number(w, "Index").map(|v| v as u32)
                && text(w, "IsOmit").is_some_and(|t| t.eq_ignore_ascii_case("true"))
            {
                wells.entry(i).or_default().omitted = true;
            }
        }
    }
    Ok((rows, cols, wells))
}

/// The JSON `setup/plate_setup.json`.
fn parse_plate_json(v: &Value, d: &mut QpcrData) -> (u32, u32, BTreeMap<u32, WellSetup>) {
    let (rows, cols) = js(v, "blockType").and_then(geometry).unwrap_or((0, 0));
    if let Some(p) = js(v, "passiveReference") {
        d.passive_reference = Some(p.to_string());
    }
    for s in jarr(v, "samples") {
        let Some(name) = js(s, "name") else { continue };
        if d.samples.iter().any(|x| x.name == name) {
            continue;
        }
        let mut smp = Sample {
            name: name.to_string(),
            kind: js(s, "type").map(task_name),
            quantity: jf(s, "quantity").filter(|q| *q != 0.0),
            ..Sample::default()
        };
        if let Some(g) = js(s, "biogroup") {
            smp.annotations.push(("biogroup".into(), g.to_string()));
        }
        d.samples.push(smp);
    }
    for t in jarr(v, "targets") {
        let Some(name) = js(t, "name") else { continue };
        if !d.targets.iter().any(|x| x.name == name) {
            d.targets.push(Target {
                name: name.to_string(),
                dye: js(t, "reporter").map(str::to_string),
                quencher: js(t, "quencher").map(str::to_string),
                ..Target::default()
            });
        }
    }
    let mut wells: BTreeMap<u32, WellSetup> = BTreeMap::new();
    for w in jarr(v, "wells") {
        let Some(i) = w.get("index").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(i) = u32::try_from(i) else { continue };
        let e = wells.entry(i).or_default();
        e.sample = js(w, "sampleName").map(str::to_string);
        e.omitted = w.get("omitted").and_then(Value::as_bool).unwrap_or(false);
        for ta in jarr(w, "targetAssignments") {
            let Some(name) = js(ta, "targetName") else {
                continue;
            };
            let reporter = d.target(name).and_then(|t| t.dye.clone());
            e.targets.push((
                name.to_string(),
                reporter,
                js(ta, "task").map(task_name),
                jf(ta, "quantity"),
            ));
        }
    }
    (rows, cols, wells)
}

/// The SDS `tcprotocol.xml` thermal profile.
fn parse_tcprotocol(xml: &str) -> Result<Program> {
    let doc = parse(xml, FORMAT_ID, "tcprotocol.xml")?;
    let root = doc.root_element();
    let mut p = Program {
        name: text(root, "ProtocolName").unwrap_or_default(),
        sample_volume_ul: crate::xml::number(root, "SampleVolume"),
        lid_temperature_c: crate::xml::number(root, "CoverTemperature"),
        run_mode: text(root, "RunMode"),
        ..Program::default()
    };
    for st in children(root, "TCStage") {
        let flag = text(st, "StageFlag").unwrap_or_default();
        let kind = match flag.as_str() {
            "CYCLING" => "cycling",
            "DISSOCIATION" | "MELT" | "MELT_CURVE" => "melt",
            "PRE_CYCLING" | "POST_CYCLING" | "HOLD" => "hold",
            "PRE_READ" => "pre-read",
            "POST_READ" => "post-read",
            "INFINITE_HOLD" => "infinite hold",
            _ => "other",
        };
        let repeats = crate::xml::number(st, "NumOfRepetitions").map_or(1, |v| v as u32);
        let mut steps = Vec::new();
        for s in children(st, "TCStep") {
            let collect = text(s, "CollectionFlag").unwrap_or_default();
            let temp = child(s, "Temperature")
                .and_then(node_text)
                .and_then(|t| num(&t));
            steps.push(Step {
                kind: "temperature".into(),
                temperature_c: temp,
                hold_s: crate::xml::number(s, "HoldTime"),
                ramp_c_per_s: crate::xml::number(s, "RampRate"),
                measure: match collect.as_str() {
                    "1" => Some(if kind == "melt" { "melt" } else { "real time" }.into()),
                    "2" => Some("melt".into()),
                    _ => None,
                },
                ..Step::default()
            });
        }
        p.stages.push(Stage {
            kind: kind.into(),
            repeats,
            steps,
        });
    }
    Ok(p)
}

/// The JSON `setup/run_method.json` thermal profile.
fn parse_run_method(v: &Value) -> Program {
    let mut p = Program {
        name: "run method".into(),
        sample_volume_ul: jf(v, "sampleVolume"),
        lid_temperature_c: jf(v, "coverTemperature"),
        run_mode: js(v, "runMode").map(str::to_string),
        ..Program::default()
    };
    for st in jarr(v, "stages") {
        let repeats = st.get("repeat").and_then(Value::as_u64).unwrap_or(1) as u32;
        let ty = js(st, "type").unwrap_or("").to_ascii_uppercase();
        let mut steps = Vec::new();
        let mut melt = ty.contains("MELT");
        for s in jarr(st, "steps") {
            let ramp = s.get("ramp").unwrap_or(&Value::Null);
            let hold = s.get("hold").unwrap_or(&Value::Null);
            let collection = hold
                .get("collectionProfile")
                .or_else(|| ramp.get("collectionProfile"));
            let processing = collection
                .and_then(|c| js(c, "processing"))
                .unwrap_or("")
                .to_ascii_uppercase();
            let is_melt = processing.contains("MELT")
                || (ramp.get("collectionProfile").is_some() && !processing.contains("QUANT"));
            melt |= is_melt;
            steps.push(Step {
                kind: "temperature".into(),
                temperature_c: jf(ramp, "temperature"),
                ramp_c_per_s: jf(ramp, "rate"),
                hold_s: jf(hold, "duration"),
                measure: collection.map(|_| if is_melt { "melt" } else { "real time" }.into()),
                ..Step::default()
            });
        }
        let kind = if melt {
            "melt"
        } else if repeats > 1 {
            "cycling"
        } else {
            "hold"
        };
        p.stages.push(Stage {
            kind: kind.into(),
            repeats,
            steps,
        });
    }
    p
}

/// A collection point `[Stg:2 Cyc:1 Stp:2 Pt:1]` → (stage, cycle, step, point).
fn collection_points(s: &str) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    for part in s.split('[').skip(1) {
        let body = part.split(']').next().unwrap_or("");
        let mut v = [0u32; 4];
        for kv in body.split_whitespace() {
            let Some((k, val)) = kv.split_once(':') else {
                continue;
            };
            let n = val.trim_end_matches(',').parse::<u32>().unwrap_or(0);
            match k {
                "Stg" => v[0] = n,
                "Cyc" => v[1] = n,
                "Stp" => v[2] = n,
                "Pt" => v[3] = n,
                _ => {}
            }
        }
        if body.contains("Stg") {
            out.push((v[0], v[1], v[2], v[3]));
        }
    }
    out
}

/// `[1.0, 2.5, NaN]` or tab-separated numbers.
fn number_list(s: &str) -> Vec<f64> {
    s.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split([',', '\t', '\n', '\r'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f64>().unwrap_or(f64::NAN))
        .collect()
}

/// Multicomponent data of one well: dye → values at every collection point.
#[derive(Debug, Default, Clone)]
struct WellSignals {
    dyes: Vec<(String, Vec<f64>)>,
    temperatures: Vec<f64>,
}

#[derive(Debug, Default)]
struct Multicomponent {
    /// (stage, cycle, step, point) per collection point; empty when unknown (7500 layout).
    points: Vec<(u32, u32, u32, u32)>,
    wells: BTreeMap<u32, WellSignals>,
}

fn parse_multicomponent_xml(xml: &str) -> Result<Multicomponent> {
    let doc = parse(xml, FORMAT_ID, "multicomponentdata.xml")?;
    let root = doc.root_element();
    let well_count = crate::xml::number(root, "WellCount").map_or(0, |v| v as usize);
    let points = text(root, "CollectionPoints").map_or_else(Vec::new, |s| collection_points(&s));
    let temps = text(root, "SampleTemperatures").map_or_else(Vec::new, |s| number_list(&s));
    let mut dye_lists: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for dd in children(root, "DyeData") {
        let Some(w) = dd
            .attribute("WellIndex")
            .and_then(|w| w.trim().parse::<u32>().ok())
        else {
            continue;
        };
        let list = text(dd, "DyeList").unwrap_or_default();
        let dyes: Vec<String> = list
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        dye_lists.insert(w, dyes);
    }
    let mut m = Multicomponent {
        points,
        wells: BTreeMap::new(),
    };
    let n = m.points.len();
    for sd in children(root, "SignalData") {
        let Some(w) = sd
            .attribute("WellIndex")
            .and_then(|w| w.trim().parse::<u32>().ok())
        else {
            continue;
        };
        let dyes = dye_lists.get(&w).cloned().unwrap_or_default();
        let mut ws = WellSignals::default();
        for (k, cd) in children(sd, "CycleData").enumerate() {
            let vals = node_text(cd).map_or_else(Vec::new, |s| number_list(&s));
            let name = dyes
                .get(k)
                .cloned()
                .unwrap_or_else(|| format!("dye {}", k + 1));
            ws.dyes.push((name, vals));
        }
        if n > 0 && well_count > 0 {
            let start = (w as usize).saturating_mul(n);
            if let Some(t) = temps.get(start..start.saturating_add(n)) {
                ws.temperatures = t.to_vec();
            }
        }
        m.wells.insert(w, ws);
    }
    Ok(m)
}

/// `multicomponent_data.txt` (7500 / StepOne): records `WELL CYCLE DYE MSE SIGNAL`, with
/// pure-dye coefficients on continuation lines in between.
fn parse_multicomponent_txt(text: &str) -> Multicomponent {
    let toks: Vec<&str> = text
        .split(['\t', '\n', '\r'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    let is_int = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let is_dye = |t: &str| {
        t.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || " _-".contains(c))
    };
    // (well, dye) → cycle → value
    let mut data: BTreeMap<u32, Vec<(String, BTreeMap<u32, f64>)>> = BTreeMap::new();
    let mut i = 0;
    while i + 4 < toks.len() {
        if is_int(toks[i]) && is_int(toks[i + 1]) && is_dye(toks[i + 2]) {
            let (Ok(w), Ok(c)) = (toks[i].parse::<u32>(), toks[i + 1].parse::<u32>()) else {
                i += 1;
                continue;
            };
            // Reads per well are hundreds at most; larger indices are corrupt records.
            if let (Ok(v), true) = (toks[i + 4].parse::<f64>(), c < 100_000 && w < 100_000) {
                let well = data.entry(w).or_default();
                let dye = toks[i + 2].to_string();
                let slot = well.iter().position(|(d, _)| *d == dye).unwrap_or_else(|| {
                    well.push((dye, BTreeMap::new()));
                    well.len() - 1
                });
                well[slot].1.insert(c, v);
                i += 5;
                continue;
            }
        }
        i += 1;
    }
    let mut m = Multicomponent::default();
    for (w, dyes) in data {
        let mut ws = WellSignals::default();
        for (dye, cycles) in dyes {
            let n = cycles.keys().max().map_or(0, |c| *c as usize + 1);
            let mut v = vec![f64::NAN; n];
            for (c, x) in cycles {
                v[c as usize] = x;
            }
            ws.dyes.push((dye, v));
        }
        m.wells.insert(w, ws);
    }
    m
}

/// The JSON `primary/multicomponent_data.json`.
fn parse_multicomponent_json(v: &Value) -> Multicomponent {
    let mut m = Multicomponent::default();
    for p in jarr(v, "collectionPoints") {
        let g = |k: &str| p.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        m.points
            .push((g("stage"), g("cycle"), g("step"), g("point")));
    }
    for w in jarr(v, "wellData") {
        let Some(i) = w.get("wellIndex").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(i) = u32::try_from(i) else { continue };
        let mut ws = WellSignals {
            temperatures: jnums(w, "temperatures").unwrap_or_default(),
            ..WellSignals::default()
        };
        for d in jarr(w, "dyeData") {
            if let (Some(name), Some(vals)) = (js(d, "dyeName"), jnums(d, "fluorescences")) {
                ws.dyes.push((name.to_string(), vals));
            }
        }
        m.wells.insert(i, ws);
    }
    m
}

/// One result of `analysis_result.txt` / `meltcuve_result.txt`: the result line's cells keyed
/// by header name, and the named value lines that follow it.
#[derive(Debug, Default, Clone)]
struct TextResult {
    cells: BTreeMap<String, String>,
    /// Lines after the result line: label and raw cells.
    series: Vec<(String, Vec<String>)>,
}

impl TextResult {
    fn get(&self, keys: &[&str]) -> Option<&str> {
        keys.iter()
            .find_map(|k| self.cells.get(&k.to_ascii_lowercase()))
            .map(String::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
    /// A value line as numbers (empty cells dropped); `None` when absent or not numeric.
    fn series(&self, name: &str) -> Option<Vec<f64>> {
        let (_, cells) = self
            .series
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))?;
        let v: Vec<f64> = cells
            .iter()
            .map(|c| c.trim())
            .filter(|c| !c.is_empty())
            .map(|c| c.parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        v.iter().any(|x| x.is_finite()).then_some(v)
    }

    /// A value line's raw cells.
    fn raw(&self, name: &str) -> Option<&[String]> {
        self.series
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_slice())
    }
}

/// Parse a tab-separated result file: a `Well …` header, result lines starting with a well
/// index, value lines starting with a label (`Rn values`, `Delta Rn values`, …).
fn parse_text_results(text: &str) -> (Vec<String>, Vec<TextResult>) {
    let mut header: Vec<String> = Vec::new();
    let mut out: Vec<TextResult> = Vec::new();
    for line in text.lines() {
        let cells: Vec<&str> = line.split('\t').collect();
        let first = cells.first().map_or("", |s| s.trim());
        if header.is_empty() {
            if first.eq_ignore_ascii_case("well") {
                header = cells
                    .iter()
                    .map(|c| c.trim().to_ascii_lowercase())
                    .collect();
            }
            continue;
        }
        if !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()) {
            let mut r = TextResult::default();
            for (k, v) in header.iter().zip(&cells) {
                r.cells.insert(k.clone(), (*v).to_string());
            }
            out.push(r);
        } else if let Some(r) = out.last_mut()
            && !first.is_empty()
        {
            r.series.push((
                first.to_string(),
                cells[1..].iter().map(|c| (*c).to_string()).collect(),
            ));
        }
    }
    (header, out)
}

/// A Ct/Cq cell: a number, or "Undetermined"/"NaN" (no amplification).
fn cq_cell(s: Option<&str>) -> (Option<f64>, bool) {
    match s {
        None => (None, false),
        Some(t) => match num(t) {
            Some(v) if v >= 0.0 => (Some(v), false),
            _ => (None, true),
        },
    }
}

/// Split `stage` points into amplification and melt ranges.
fn select_points(
    points: &[(u32, u32, u32, u32)],
    amp_stage: Option<u32>,
    melt_stage: Option<u32>,
) -> (Vec<usize>, Vec<usize>) {
    if points.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // stage → (distinct cycles, point count)
    let mut stages: BTreeMap<u32, (std::collections::BTreeSet<u32>, usize)> = BTreeMap::new();
    for p in points {
        let e = stages.entry(p.0).or_default();
        e.0.insert(p.1);
        e.1 += 1;
    }
    let amp = amp_stage.filter(|s| stages.contains_key(s)).or_else(|| {
        stages
            .iter()
            .filter(|(_, (c, _))| c.len() > 1)
            .max_by_key(|(_, (c, _))| c.len())
            .map(|(s, _)| *s)
    });
    let melt = melt_stage.filter(|s| stages.contains_key(s)).or_else(|| {
        stages
            .iter()
            .filter(|(s, (c, n))| Some(**s) != amp && c.len() == 1 && *n > 5)
            .max_by_key(|(_, (_, n))| *n)
            .map(|(s, _)| *s)
    });
    let idx = |st: Option<u32>| -> Vec<usize> {
        st.map_or_else(Vec::new, |st| {
            points
                .iter()
                .enumerate()
                .filter(|(_, p)| p.0 == st)
                .map(|(i, _)| i)
                .collect()
        })
    };
    (idx(amp), idx(melt))
}

/// Read an `.eds` archive.
pub(crate) fn parse_eds(zip: &ZipIndex) -> Result<QpcrData> {
    let dialect = layout(zip).ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            "the zip holds neither apldbio/sds/ experiment files nor setup/plate_setup.json",
        )
    })?;
    let mut out = QpcrData::new(dialect);
    out.instrument.manufacturer = Some("Applied Biosystems (Thermo Fisher Scientific)".into());
    let mut vendor = serde_json::Map::new();

    // ---- manifests
    for (name, key) in [
        ("Manifest.mf", "manifest"),
        ("apldbio/sds/Manifest.mf", "sds_manifest"),
    ] {
        if let Some(t) = zip.read_text(name)? {
            let m = manifest(&t);
            if out.format_version.is_none() {
                out.format_version = m.get("Specification-Version").cloned();
            }
            if let Some(title) = m
                .get("Implementation-Title")
                .filter(|t| !t.contains("File API"))
            {
                out.instrument.software.get_or_insert_with(|| title.clone());
            }
            if let Some(v) = m.get("Implementation-Version") {
                out.instrument
                    .software_version
                    .get_or_insert_with(|| v.clone());
            }
            if let Some(ct) = m.get("Content-Type") {
                out.experiment_type.get_or_insert_with(|| ct.clone());
            }
            vendor.insert(key.into(), json!(m));
        }
    }

    // ---- experiment metadata (SDS)
    let mut sds_samples: Vec<Sample> = Vec::new();
    if let Some(t) = zip.read_text("apldbio/sds/experiment.xml")? {
        match parse(&t, FORMAT_ID, "experiment.xml") {
            Ok(doc) => {
                let root = doc.root_element();
                out.name = text(root, "Name");
                out.description = text(root, "Description");
                out.operator = text(root, "Operator");
                out.run_state = text(root, "RunState");
                out.created_at = crate::xml::number(root, "CreatedTime").and_then(ms_to_iso);
                out.started_at = crate::xml::number(root, "RunStartTime").and_then(ms_to_iso);
                out.ended_at = crate::xml::number(root, "RunEndTime").and_then(ms_to_iso);
                if let Some(ty) = child(root, "Type") {
                    out.experiment_type = text(ty, "Name")
                        .or_else(|| text(ty, "Id"))
                        .or(out.experiment_type.take());
                }
                out.chemistry = text(root, "ChemistryType");
                if let Some(it) = text(root, "InstrumentTypeId") {
                    out.instrument.model = Some(it);
                }
                for p in children(root, "ExperimentProperty")
                    .filter(|p| p.attribute("type") == Some("RunInfo"))
                {
                    for pv in children(p, "PropertyValue") {
                        let val = pv.children().filter(Node::is_element).find_map(node_text);
                        match (pv.attribute("key"), val) {
                            (Some("softwareVersion"), Some(v)) => {
                                out.instrument.software = Some(v);
                            }
                            (Some("instrumentSerialNumber"), Some(v)) => {
                                out.instrument.serial_number = Some(v);
                            }
                            (Some("instrumentName"), Some(v)) => {
                                vendor.insert("instrument_name".into(), json!(v));
                            }
                            (Some("userId"), Some(v))
                                if out.operator.is_none() && v != "DEFAULT" =>
                            {
                                out.operator = Some(v);
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(ss) = child(root, "Samples") {
                    for s in children(ss, "Sample") {
                        if let Some(name) = text(s, "Name") {
                            sds_samples.push(Sample {
                                name,
                                ..Sample::default()
                            });
                        }
                    }
                }
                if let Some(ds) = child(root, "Detectors") {
                    for det in children(ds, "Detector") {
                        if let Some(name) = text(det, "Name")
                            && !out.targets.iter().any(|t| t.name == name)
                        {
                            out.targets.push(Target {
                                name,
                                dye: text(det, "Reporter"),
                                quencher: text(det, "Quencher").filter(|q| q != "None"),
                                ..Target::default()
                            });
                        }
                    }
                }
                vendor.insert(
                    "experiment".into(),
                    openreadout_core::xmljson::xml_to_json(&strip_plot_properties(&t))
                        .unwrap_or(Value::Null),
                );
            }
            Err(e) => note(&mut out, "apldbio/sds/experiment.xml", &e),
        }
    }

    // ---- JSON summaries
    if let Some(t) = zip.read_text("summary.json")? {
        match lenient_json(&t, "summary.json") {
            Ok(v) => {
                out.name = js(&v, "name").map(str::to_string).or(out.name.take());
                if let Some(it) = js(&v, "instrumentType") {
                    out.instrument.model = Some(json_instrument(it));
                }
                if let Some(s) = js(&v, "runStatus") {
                    out.run_state.get_or_insert_with(|| s.to_string());
                }
                if let Some(c) = jf(&v, "createdTime").and_then(ms_to_iso) {
                    out.created_at.get_or_insert(c);
                }
                vendor.insert("summary".into(), v);
            }
            Err(e) => note(&mut out, "summary.json", &e),
        }
    }
    if let Some(t) = zip.read_text("run/run_summary.json")? {
        match lenient_json(&t, "run/run_summary.json") {
            Ok(v) => {
                if let Some(s) = js(&v, "instrumentSerialNumber") {
                    out.instrument.serial_number = Some(s.to_string());
                }
                if let Some(s) = js(&v, "firmwareVersion") {
                    out.instrument.firmware_version = Some(s.to_string());
                }
                if let Some(s) = js(&v, "operator") {
                    out.operator = Some(s.to_string());
                }
                if let Some(s) = jf(&v, "startTime").and_then(ms_to_iso) {
                    out.started_at = Some(s);
                }
                if let Some(s) = jf(&v, "endTime").and_then(ms_to_iso) {
                    out.ended_at = Some(s);
                }
                vendor.insert("run_summary".into(), v);
            }
            Err(e) => note(&mut out, "run/run_summary.json", &e),
        }
    }
    if dialect == Dialect::EdsJson && out.instrument.software.is_none() {
        out.instrument.software = Some("QuantStudio Design & Analysis".into());
    }

    // ---- plate setup
    let mut rows = 0;
    let mut cols = 0;
    let mut wells: BTreeMap<u32, WellSetup> = BTreeMap::new();
    let json_setup = match zip.read_text("setup/plate_setup.json")? {
        Some(t) => match lenient_json(&t, "setup/plate_setup.json") {
            Ok(v) => Some(v),
            Err(e) => {
                note(&mut out, "setup/plate_setup.json", &e);
                None
            }
        },
        None => None,
    };
    if let Some(v) = &json_setup {
        (rows, cols, wells) = parse_plate_json(v, &mut out);
    } else if let Some(t) = zip.read_text("apldbio/sds/plate_setup.xml")? {
        (rows, cols, wells) = parse_plate_xml(&t, &mut out)?;
    } else {
        out.notes.push(
            "no plate setup in the file (setup/plate_setup.json or apldbio/sds/plate_setup.xml)"
                .into(),
        );
    }
    // samples named in the setup but not listed yet
    for s in sds_samples {
        if !out.samples.iter().any(|x| x.name == s.name) {
            out.samples.push(s);
        }
    }
    for w in wells.values() {
        if let Some(s) = &w.sample
            && !out.samples.iter().any(|x| &x.name == s)
        {
            out.samples.push(Sample {
                name: s.clone(),
                ..Sample::default()
            });
        }
    }
    if rows == 0 || cols == 0 {
        (rows, cols) = (8, 12);
        out.notes
            .push("plate geometry not recorded; assumed 8 x 12".into());
    }

    // ---- thermal profile
    if let Some(t) = zip.read_text("setup/run_method.json")? {
        match lenient_json(&t, "setup/run_method.json") {
            Ok(v) => out.programs.push(parse_run_method(&v)),
            Err(e) => note(&mut out, "setup/run_method.json", &e),
        }
    } else if let Some(t) = zip.read_text("apldbio/sds/tcprotocol.xml")? {
        match parse_tcprotocol(&t) {
            Ok(p) => out.programs.push(p),
            Err(e) => note(&mut out, "apldbio/sds/tcprotocol.xml", &e),
        }
    }

    // ---- analysis settings
    let protocol = match zip.read_text("apldbio/sds/analysis_protocol.xml")? {
        Some(t) => parse_protocol(&t).unwrap_or_else(|e| {
            note(&mut out, "apldbio/sds/analysis_protocol.xml", &e);
            Protocol::default()
        }),
        None => Protocol::default(),
    };
    if let Some(c) = &protocol.endogenous_control {
        out.reference_targets.push(c.clone());
    }
    out.calibrator_sample.clone_from(&protocol.calibrator);
    let mut json_defaults = CtSettings::default();
    if let Some(t) = zip.read_text("primary/analysis_setting.json")?
        && let Ok(v) = lenient_json(&t, "primary/analysis_setting.json")
    {
        if let Some(c) = v.get("defaultCtSetting") {
            json_defaults = CtSettings {
                threshold: jf(c, "threshold"),
                auto_threshold: c.get("autoThreshold").and_then(Value::as_bool),
                baseline_start: jf(c, "baselineStart").map(|x| x as u32),
                baseline_end: jf(c, "baselineEnd").map(|x| x as u32),
                auto_baseline: c.get("autoBaseline").and_then(Value::as_bool),
            };
        }
        vendor.insert("analysis_setting".into(), v);
    }
    if let Some(t) = zip.read_text("extensions/am.rq/relative_quantification_setting.json")?
        && let Ok(v) = lenient_json(&t, "relative_quantification_setting.json")
    {
        for k in ["referenceTargets", "endogenousControls"] {
            for r in jarr(&v, k) {
                if let Some(s) = r.as_str() {
                    out.reference_targets.push(s.to_string());
                }
            }
        }
        for k in ["referenceSample", "calibratorSample"] {
            if let Some(s) = js(&v, k) {
                out.calibrator_sample = Some(s.to_string());
            }
        }
        vendor.insert("relative_quantification_setting".into(), v);
    }

    // ---- multicomponent data
    let mc = if let Some(t) = zip.read_text("primary/multicomponent_data.json")? {
        match lenient_json(&t, "primary/multicomponent_data.json") {
            Ok(v) => Some(parse_multicomponent_json(&v)),
            Err(e) => {
                note(&mut out, "primary/multicomponent_data.json", &e);
                None
            }
        }
    } else if let Some(t) = zip.read_text("apldbio/sds/multicomponentdata.xml")? {
        match parse_multicomponent_xml(&t) {
            Ok(m) => Some(m),
            Err(e) => {
                note(&mut out, "apldbio/sds/multicomponentdata.xml", &e);
                None
            }
        }
    } else {
        zip.read_text("apldbio/sds/multicomponent_data.txt")?
            .map(|t| parse_multicomponent_txt(&t))
    };
    if mc.is_none() {
        let images = zip.with_prefix("apldbio/sds/images/").count();
        out.notes.push(if images > 0 {
            format!("no multicomponent (dye) data: the file keeps {images} raw optical images only, which are not decoded")
        } else {
            "no fluorescence data in the file (the run was set up but not run, or the data were removed)".into()
        });
    }
    let (amp_idx, melt_idx) = mc.as_ref().map_or((Vec::new(), Vec::new()), |m| {
        select_points(&m.points, protocol.amp_stage, protocol.melt_stage)
    });
    let program_cycles = out.programs.first().and_then(Program::cycles);

    // ---- vendor results
    let (_, amp_results) = match zip.read_text("apldbio/sds/analysis_result.txt")? {
        Some(t) => parse_text_results(&t),
        None => (Vec::new(), Vec::new()),
    };
    let genotyping = amp_results.iter().any(|r| r.cells.contains_key("call"));
    if genotyping {
        let alleles = match zip.read_text("apldbio/sds/plate_setup.xml")? {
            Some(t) => marker_alleles(&t),
            None => BTreeMap::new(),
        };
        for r in &amp_results {
            let get = |k: &str| r.cells.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
            let Some(position) = get("well").and_then(|w| w.parse::<u32>().ok()) else {
                continue;
            };
            let marker = get("marker name").map(str::to_string);
            out.genotypes.push(crate::model::GenotypeCall {
                position,
                sample: get("sample name").map(str::to_string),
                task: get("task").map(task_name),
                code: get("call").and_then(|c| c.parse::<i64>().ok()),
                rn_x: get("rnx").and_then(num),
                rn_y: get("rny").and_then(num),
                reference: get("ref").and_then(num),
                confidence: get("confidence").and_then(num),
                method: get("method").map(str::to_string),
                alleles: marker.as_ref().and_then(|m| alleles.get(m).cloned()),
                marker,
            });
        }
        out.notes.push(format!(
            "genotyping (allelic discrimination) results: {} calls are the table `genotypes` (the call codes are read as docs/formats/qpcr.md says: 1, 3 homozygous allele 1, 2; 2 heterozygous; 0 negative control; -1 undetermined)",
            out.genotypes.len()
        ));
        vendor.insert(
            "genotype_calls".into(),
            json!(
                amp_results
                    .iter()
                    .map(|r| json!(r.cells))
                    .collect::<Vec<_>>()
            ),
        );
    }
    let (_, melt_results) = match zip.read_text("apldbio/sds/meltcuve_result.txt")? {
        Some(t) => parse_text_results(&t),
        None => (Vec::new(), Vec::new()),
    };
    let json_results = match zip.read_text("primary/analysis_result.json")? {
        Some(t) => match lenient_json(&t, "primary/analysis_result.json") {
            Ok(v) => Some(v),
            Err(e) => {
                note(&mut out, "primary/analysis_result.json", &e);
                None
            }
        },
        None => None,
    };
    let std_curve = match zip.read_text("extensions/am.sc/standard_curve_result.json")? {
        Some(t) => lenient_json(&t, "standard_curve_result.json").ok(),
        None => None,
    };
    if let Some(sc) = &std_curve {
        for c in jarr(sc, "standardCurves") {
            if let Some(t) = js(c, "targetName") {
                out.standard_curves.push(StandardCurve {
                    target: t.to_string(),
                    dye: js(c, "dye").map(str::to_string),
                    slope: jf(c, "slope"),
                    intercept: jf(c, "yIntercept"),
                    r2: jf(c, "r2"),
                    efficiency_percent: jf(c, "efficiency"),
                });
            }
        }
    }

    // index vendor results by (well, target)
    let mut txt_by: BTreeMap<(u32, String), &TextResult> = BTreeMap::new();
    if !genotyping {
        for r in &amp_results {
            if let (Some(w), Some(t)) = (
                r.get(&["well"]).and_then(|w| w.parse::<u32>().ok()),
                r.get(&["detector", "target name", "target"]),
            ) {
                txt_by.insert((w, t.to_string()), r);
            }
        }
    }
    let mut melt_by: BTreeMap<(u32, String), &TextResult> = BTreeMap::new();
    for r in &melt_results {
        if let (Some(w), Some(t)) = (
            r.get(&["well"]).and_then(|w| w.parse::<u32>().ok()),
            r.get(&["detector", "target name", "target"]),
        ) {
            melt_by.insert((w, t.to_string()), r);
        }
    }
    let mut json_by: BTreeMap<(u32, String), &Value> = BTreeMap::new();
    let mut json_well_omitted: BTreeMap<u32, bool> = BTreeMap::new();
    if let Some(v) = &json_results {
        for w in jarr(v, "wellResults") {
            let Some(i) = w
                .get("wellIndex")
                .and_then(Value::as_u64)
                .and_then(|i| u32::try_from(i).ok())
            else {
                continue;
            };
            for rr in jarr(w, "reactionResults") {
                if let Some(t) = js(rr, "targetName") {
                    json_by.insert((i, t.to_string()), rr);
                    if rr.get("omitted").and_then(Value::as_bool) == Some(true) {
                        json_well_omitted.insert(i, true);
                    }
                }
            }
        }
    }
    let mut sc_qty: BTreeMap<(u32, String), f64> = BTreeMap::new();
    if let Some(sc) = &std_curve {
        for r in jarr(sc, "reactions") {
            if let (Some(i), Some(t), Some(q)) = (
                r.get("wellIndex")
                    .and_then(Value::as_u64)
                    .and_then(|i| u32::try_from(i).ok()),
                js(r, "targetName"),
                jf(r, "quantity").filter(|q| *q >= 0.0),
            ) {
                sc_qty.insert((i, t.to_string()), q);
            }
        }
    }

    // ---- assemble reactions
    let mut positions: std::collections::BTreeSet<u32> = wells.keys().copied().collect();
    if let Some(m) = &mc {
        positions.extend(m.wells.keys().copied());
    }
    for (w, _) in txt_by.keys() {
        positions.insert(*w);
    }
    for (w, _) in json_by.keys() {
        positions.insert(*w);
    }
    let well_total = rows.saturating_mul(cols);
    let mut reactions = Vec::new();
    for pos in positions {
        if well_total > 0 && pos >= well_total {
            out.notes.push(format!(
                "well index {pos} lies outside the {rows} x {cols} plate; skipped"
            ));
            continue;
        }
        let setup = wells.get(&pos).cloned().unwrap_or_default();
        let signals = mc.as_ref().and_then(|m| m.wells.get(&pos));
        let mut rx = Reaction {
            position: pos,
            row: pos / cols,
            column: pos % cols,
            sample: setup.sample.clone(),
            omitted: setup.omitted || json_well_omitted.get(&pos).copied().unwrap_or(false),
            ..Reaction::default()
        };
        if let Some(sig) = signals {
            for (dye, vals) in &sig.dyes {
                let v: Vec<f64> = if amp_idx.is_empty() {
                    let n = program_cycles.map_or(vals.len(), |c| (c as usize).min(vals.len()));
                    vals[..n].to_vec()
                } else {
                    amp_idx
                        .iter()
                        .map(|&i| vals.get(i).copied().unwrap_or(f64::NAN))
                        .collect()
                };
                rx.signals.push(DyeSignal {
                    dye: dye.clone(),
                    values: v,
                });
            }
        }
        // targets: setup first, then any result that names a target not in the setup
        let mut targets = setup.targets.clone();
        for key in txt_by.keys().chain(json_by.keys()).filter(|k| k.0 == pos) {
            if !targets.iter().any(|t| t.0 == key.1) {
                let reporter = out.target(&key.1).and_then(|t| t.dye.clone());
                targets.push((key.1.clone(), reporter, None, None));
            }
        }
        for (target, reporter, task, qty) in targets {
            let key = (pos, target.clone());
            let mut assay = Assay {
                dye: reporter.clone(),
                task,
                quantity: qty.filter(|q| *q > 0.0),
                target: Some(target.clone()),
                ..Assay::default()
            };
            if let Some(r) = txt_by.get(&key) {
                let (cq, und) = cq_cell(r.get(&["ct", "cт", "cq", "crt"]));
                assay.cq = cq;
                assay.cq_undetermined = und;
                // The vendor writes "Undetermined" as Ct = the cycle count (40.0 of 40 cycles);
                // its exports show those wells as Undetermined (docs/provenance/qpcr.md).
                if let (Some(v), Some(n)) = (cq, program_cycles)
                    && v >= f64::from(n)
                {
                    assay.cq = None;
                    assay.cq_undetermined = true;
                    assay.cq_stored = Some(v);
                    assay.flags.push("ct_at_cycle_count".into());
                }
                assay.cq_mean = r.get(&["avg ct", "ct mean", "cq mean"]).and_then(num);
                assay.cq_sd = r.get(&["ct sd", "cq sd"]).and_then(num);
                assay.calculated_quantity = r.get(&["qty", "quantity"]).and_then(num);
                // codes paired with the vendor export's Amp / Inconclusive / No Amp
                assay.amp_status = r.get(&["amp status"]).map(|s| match s {
                    "1" => "amplified".to_string(),
                    "0" => "inconclusive".to_string(),
                    "-1" => "not amplified".to_string(),
                    o => o.to_string(),
                });
                assay.cq_confidence = r.get(&["cq conf"]).and_then(num);
                if assay.task.is_none() {
                    assay.task = r.get(&["task"]).map(task_name);
                }
                if let Some(rn) = r.series("Rn values") {
                    let n = rn.len();
                    assay.amplification = Some(Curve {
                        cycles: (1..=n).map(|c| c as f64).collect(),
                        fluorescence: rn,
                        corrected: r.series("Delta Rn values").filter(|v| v.len() == n),
                        quantity: "Rn",
                    });
                }
                // ΔΔCt experiments: `DDCT Values` = well, -, sample, target, task, Ct, -, ΔCt,
                // ΔCt SD, ΔCt SE, -, RQ, RQ min, RQ max, outlier, ΔΔCt.
                if let Some(cells) = r.raw("DDCT Values") {
                    let at = |i: usize| cells.get(i).and_then(|c| num(c));
                    assay.vendor_delta_cq = at(7);
                    assay.vendor_rq = at(11);
                    assay.vendor_delta_delta_cq = at(15);
                }
                let s = protocol.for_well(pos, &target);
                assay.threshold = s.threshold;
                assay.threshold_used = s.threshold.is_some() && s.auto_threshold == Some(false);
                assay.auto_threshold = s.auto_threshold;
                assay.baseline_start = s.baseline_start;
                assay.baseline_end = s.baseline_end;
                assay.auto_baseline = s.auto_baseline;
            }
            if let Some(rr) = json_by.get(&key) {
                if assay.task.is_none() {
                    assay.task = js(rr, "task").map(task_name);
                }
                if let Some(ar) = rr.get("amplificationResult") {
                    let c = jf(ar, "cq");
                    assay.cq = c.filter(|v| *v >= 0.0);
                    assay.cq_undetermined = ar.get("cq").is_some() && assay.cq.is_none();
                    assay.cq_confidence = jf(ar, "cqConf");
                    assay.threshold = jf(ar, "ctThreshold");
                    assay.threshold_used = assay.threshold.is_some();
                    assay.baseline_start = jf(ar, "ctBaselineStart").map(|v| v as u32);
                    assay.baseline_end = jf(ar, "ctBaselineEnd").map(|v| v as u32);
                    assay.auto_threshold = json_defaults.auto_threshold;
                    assay.auto_baseline = json_defaults.auto_baseline;
                    assay.amp_status =
                        js(ar, "ampStatus").map(|s| s.to_ascii_lowercase().replace('_', " "));
                    assay.flags = jarr(ar, "flags")
                        .iter()
                        .filter_map(|f| f.as_str().map(str::to_string))
                        .collect();
                    if let Some(rn) = jnums(ar, "rn").filter(|v| !v.is_empty()) {
                        let n = rn.len();
                        assay.amplification = Some(Curve {
                            cycles: (1..=n).map(|c| c as f64).collect(),
                            fluorescence: rn,
                            corrected: jnums(ar, "deltaRn").filter(|v| v.len() == n),
                            quantity: "Rn",
                        });
                    }
                }
                if let Some(mr) = rr.get("meltResult").or_else(|| rr.get("meltCurveResult")) {
                    for k in ["tm", "tms", "meltTemperatures"] {
                        match mr.get(k) {
                            Some(Value::Array(a2)) => {
                                assay.tm.extend(a2.iter().filter_map(Value::as_f64));
                            }
                            Some(Value::Number(n)) => assay.tm.extend(n.as_f64()),
                            _ => {}
                        }
                    }
                    if let (Some(t), Some(f)) = (jnums(mr, "temperatures"), jnums(mr, "rn")) {
                        assay.melt = Some(Melt {
                            derivative: jnums(mr, "derivative")
                                .or_else(|| jnums(mr, "derivatives"))
                                .map(|dv| (t.clone(), dv)),
                            temperature: t,
                            fluorescence: f,
                        });
                    }
                }
                if rr.get("omitted").and_then(Value::as_bool) == Some(true) {
                    assay.excluded = Some("omitted".into());
                }
            }
            if let Some(q) = sc_qty.get(&key) {
                assay.calculated_quantity = Some(*q);
            }
            if let Some(r) = melt_by.get(&key) {
                if let Some(tm) = r.get(&["tm", "tm1"]) {
                    assay.tm = tm.split([',', ';']).filter_map(num).collect();
                }
                if let (Some(t), Some(f)) = (r.series("Sample Temperatures"), r.series("Rn values"))
                {
                    let n = t.len().min(f.len());
                    let deriv = match (
                        r.series("Delta Rn Sample Temperatures"),
                        r.series("Delta Rn values")
                            .or_else(|| r.series("Derivative values")),
                    ) {
                        (Some(dt), Some(dv)) if dt.len() == dv.len() => Some((dt, dv)),
                        _ => None,
                    };
                    assay.melt = Some(Melt {
                        temperature: t[..n].to_vec(),
                        fluorescence: f[..n].to_vec(),
                        derivative: deriv,
                    });
                }
            }
            // Without vendor results: the reporter's multicomponent signal.
            if assay.amplification.is_none()
                && let (Some(dye), Some(sig)) = (&reporter, signals)
                && let Some((_, vals)) = sig.dyes.iter().find(|(n, _)| n.eq_ignore_ascii_case(dye))
            {
                let v: Vec<f64> = if amp_idx.is_empty() {
                    let n = program_cycles.map_or(vals.len(), |c| (c as usize).min(vals.len()));
                    vals[..n].to_vec()
                } else {
                    amp_idx
                        .iter()
                        .map(|&i| vals.get(i).copied().unwrap_or(f64::NAN))
                        .collect()
                };
                if !v.is_empty() {
                    let cycles = if amp_idx.is_empty() {
                        (1..=v.len()).map(|c| c as f64).collect()
                    } else {
                        let pts = &mc.as_ref().map(|m| m.points.clone()).unwrap_or_default();
                        amp_idx
                            .iter()
                            .map(|&i| pts.get(i).map_or(f64::NAN, |p| f64::from(p.1)))
                            .collect()
                    };
                    assay.amplification = Some(Curve {
                        cycles,
                        fluorescence: v,
                        corrected: None,
                        quantity: "multicomponent",
                    });
                }
            }
            if assay.melt.is_none()
                && !melt_idx.is_empty()
                && let (Some(dye), Some(sig)) = (&reporter, signals)
                && let Some((_, vals)) = sig.dyes.iter().find(|(n, _)| n.eq_ignore_ascii_case(dye))
                && sig.temperatures.len() >= vals.len()
            {
                assay.melt = Some(Melt {
                    temperature: melt_idx
                        .iter()
                        .map(|&i| sig.temperatures.get(i).copied().unwrap_or(f64::NAN))
                        .collect(),
                    fluorescence: melt_idx
                        .iter()
                        .map(|&i| vals.get(i).copied().unwrap_or(f64::NAN))
                        .collect(),
                    derivative: None,
                });
            }
            if setup.omitted && assay.excluded.is_none() {
                assay.excluded = Some("omitted".into());
            }
            rx.assays.push(assay);
        }
        if rx.assays.is_empty() && rx.signals.is_empty() && rx.sample.is_none() {
            continue;
        }
        reactions.push(rx);
    }
    if let Some(v) = &json_results {
        let groups: Vec<Value> = jarr(v, "replicateGroupResults").to_vec();
        // replicate means by (sample, target)
        for rx in &mut reactions {
            for assay in &mut rx.assays {
                if assay.cq_mean.is_some() {
                    continue;
                }
                if let Some(g) = groups.iter().find(|g| {
                    js(g, "sampleName") == rx.sample.as_deref()
                        && js(g, "targetName") == assay.target.as_deref()
                }) {
                    assay.cq_mean = jf(g, "cqMean");
                    assay.cq_sd = jf(g, "cqSD");
                }
            }
        }
    }
    estimate_thresholds(&mut reactions);
    if matches!(out.dialect, Dialect::EdsSds | Dialect::Eds7500) {
        let (found, unknown) = automatic_baseline_windows(&mut reactions);
        if unknown > 0 {
            out.notes.push(format!(
                "{unknown} wells with an automatic baseline: the window the vendor used could not be recovered from Rn − ΔRn (baseline_start/end left empty; {found} recovered)"
            ));
        }
    }
    out.runs.push(Run {
        name: out.name.clone().unwrap_or_else(|| "run".into()),
        instrument: out.instrument.model.clone(),
        software: out.instrument.software.clone(),
        started_at: out.started_at.clone(),
        program: (!out.programs.is_empty()).then_some(0),
        rows,
        columns: cols,
        row_label: "ABC".into(),
        column_label: "123".into(),
        reactions,
        ..Run::default()
    });
    if amp_results.is_empty() && json_results.is_none() && mc.is_some() {
        out.notes.push("the file holds no analysis results (no Cq values): the run was not analysed in the vendor software; curves are the reporter dyes' multicomponent signals".into());
    }
    out.vendor = Value::Object(vendor);
    Ok(out)
}

/// Linear interpolation of `values` (on `cycles`) at cycle `x`.
#[allow(clippy::many_single_char_names)] // interpolation in its textbook notation
fn interpolate(cycles: &[f64], values: &[f64], x: f64) -> Option<f64> {
    let n = cycles.len().min(values.len());
    (1..n).find_map(|i| {
        let (c0, c1) = (cycles[i - 1], cycles[i]);
        (c0 <= x && x <= c1 && c1 > c0).then(|| {
            let f = (x - c0) / (c1 - c0);
            let (a, b) = (values[i - 1], values[i]);
            // log-linear between two positive reads (exponential growth), else linear
            if a > 0.0 && b > 0.0 {
                (a.ln() + f * (b.ln() - a.ln())).exp()
            } else {
                a + f * (b - a)
            }
        })
    })
}

/// The vendor's baseline of a curve: `Rn − ΔRn` is a straight line (the fitted baseline); keep
/// it when the stored values are one (within a relative 1e-5), as (intercept, slope).
fn vendor_baseline(c: &Curve) -> Option<(f64, f64)> {
    let corr = c.corrected.as_ref()?;
    let base: Vec<f64> = c
        .fluorescence
        .iter()
        .zip(corr)
        .map(|(f, d)| f - d)
        .collect();
    let (a, b, _) = crate::analysis::linear_fit(&c.cycles, &base)?;
    let scale = base.iter().fold(0f64, |m, v| m.max(v.abs())).max(1e-12);
    let resid = c
        .cycles
        .iter()
        .zip(&base)
        .map(|(x, y)| (y - (a + b * x)).abs())
        .fold(0f64, f64::max);
    (resid <= 1e-5 * scale).then_some((a, b))
}

/// Marker name → (allele 1, allele 2) names from `plate_setup.xml` (`Marker` with `Allele1` and
/// `Allele2` children).
fn marker_alleles(xml: &str) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    let Ok(doc) = parse(xml, FORMAT_ID, "plate_setup.xml") else {
        return out;
    };
    for m in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Marker")
    {
        if let (Some(name), Some(a), Some(b)) = (
            text(m, "Name"),
            child(m, "Allele1").and_then(|a| text(a, "Name")),
            child(m, "Allele2").and_then(|a| text(a, "Name")),
        ) {
            out.entry(name).or_insert((a, b));
        }
    }
    out
}

/// The baseline window of an automatic baseline (SDS/7500 layouts store only the setting): the
/// cycles `s..=e` (1-based, at least two) whose least-squares line through the stored Rn equals
/// the vendor's baseline line `Rn − ΔRn` (intercept and slope within 1e-6 of the Rn scale). `None`
/// when no window or more than one does (`docs/provenance/qpcr.md`, 2026-09-26).
#[allow(clippy::many_single_char_names)] // least-squares sums in their textbook notation
fn baseline_window(c: &Curve, line: (f64, f64)) -> Option<(u32, u32)> {
    let (a, b) = line;
    let n = c.fluorescence.len().min(c.cycles.len());
    let tol = 1e-6 * a.abs().max(1e-3);
    // prefix sums of x, y, x², xy
    let mut ps = vec![[0f64; 4]; n + 1];
    for i in 0..n {
        let (x, y) = (c.cycles[i], c.fluorescence[i]);
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let p = ps[i];
        ps[i + 1] = [p[0] + x, p[1] + y, p[2] + x * x, p[3] + x * y];
    }
    let mut hit = None;
    for s in 0..n {
        for e in s + 1..n {
            let k = (e - s + 1) as f64;
            let (sx, sy, sxx, sxy) = (
                ps[e + 1][0] - ps[s][0],
                ps[e + 1][1] - ps[s][1],
                ps[e + 1][2] - ps[s][2],
                ps[e + 1][3] - ps[s][3],
            );
            let den = k * sxx - sx * sx;
            if den.abs() < 1e-12 {
                continue;
            }
            let slope = (k * sxy - sx * sy) / den;
            let icpt = (sy - slope * sx) / k;
            if (icpt - a).abs() <= tol && (slope - b).abs() <= tol {
                if hit.is_some() {
                    return None;
                }
                hit = Some((u32::try_from(s + 1).ok()?, u32::try_from(e + 1).ok()?));
            }
        }
    }
    hit
}

/// SDS/7500 layouts: replace the baseline setting of wells with an automatic baseline by the
/// window the vendor used, or nothing. Returns (recovered, not recovered).
fn automatic_baseline_windows(reactions: &mut [Reaction]) -> (usize, usize) {
    let (mut found, mut unknown) = (0, 0);
    for rx in reactions.iter_mut() {
        for a in &mut rx.assays {
            if a.auto_baseline != Some(true) {
                continue;
            }
            let w = match (&a.amplification, a.background, a.background_slope) {
                (Some(c), Some(i), Some(s)) => baseline_window(c, (i, s)),
                _ => None,
            };
            if w.is_some() {
                found += 1;
            } else if a.baseline_start.is_some() || a.baseline_end.is_some() {
                unknown += 1;
            }
            a.baseline_start = w.map(|w| w.0);
            a.baseline_end = w.map(|w| w.1);
        }
    }
    (found, unknown)
}

/// The SDS and 7500 layouts do not store the automatic threshold the vendor used. Recover it
/// per target as the median of the vendor's own ΔRn at the vendor's Cq over the wells of that
/// target, for our Cq to default to.
fn estimate_thresholds(reactions: &mut [Reaction]) {
    let mut by_target: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for rx in reactions.iter() {
        for a in &rx.assays {
            if let (Some(t), Some(cq), Some(c)) = (&a.target, a.cq, &a.amplification)
                && let Some(corr) = &c.corrected
                && let Some(v) = interpolate(&c.cycles, corr, cq)
                && v.is_finite()
            {
                by_target.entry(t.clone()).or_default().push(v);
            }
        }
    }
    let medians: BTreeMap<String, f64> = by_target
        .into_iter()
        .filter_map(|(t, mut v)| {
            v.sort_by(f64::total_cmp);
            let n = v.len();
            (n > 0).then(|| {
                let m = if n % 2 == 1 {
                    v[n / 2]
                } else {
                    f64::midpoint(v[n / 2 - 1], v[n / 2])
                };
                (t, m)
            })
        })
        .collect();
    for rx in reactions.iter_mut() {
        for a in &mut rx.assays {
            if let Some(t) = &a.target {
                a.estimated_threshold = medians.get(t).copied().filter(|m| *m > 0.0);
            }
            if a.background.is_none()
                && let Some((i, s)) = a.amplification.as_ref().and_then(vendor_baseline)
            {
                a.background = Some(i);
                a.background_slope = Some(s);
            }
        }
    }
}

/// `experiment.xml` without the plot-appearance properties (fonts, colours, axis ranges):
/// dozens of `ExperimentProperty` blocks that say nothing about the experiment.
fn strip_plot_properties(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(i) = rest.find("<ExperimentProperty") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let is_plot = tail
            .split('>')
            .next()
            .is_some_and(|open| open.contains("Plot") || open.contains("PLOT"));
        if let Some(end) = tail.find("</ExperimentProperty>") {
            let block = &tail[..end + "</ExperimentProperty>".len()];
            if !is_plot {
                out.push_str(block);
            }
            rest = &tail[end + "</ExperimentProperty>".len()..];
        } else {
            out.push_str(tail);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_baseline_window_is_recovered_or_refused() {
        // a noisy baseline for 30 cycles, then growth; the vendor's line fits cycles 3-13
        let noise = [
            0.3, -0.2, 0.5, -0.4, 0.1, 0.25, -0.35, 0.2, -0.1, 0.4, -0.3, 0.15,
        ];
        let cycles: Vec<f64> = (1..=40).map(f64::from).collect();
        let rn: Vec<f64> = cycles
            .iter()
            .enumerate()
            .map(|(i, x)| {
                1.0 + 0.01 * x
                    + 1e-3 * noise[i % noise.len()]
                    + if *x > 30.0 {
                        (x - 30.0).powi(2) * 0.05
                    } else {
                        0.0
                    }
            })
            .collect();
        let (a, b, _) = crate::analysis::linear_fit(&cycles[2..13], &rn[2..13]).unwrap();
        let c = Curve {
            cycles: cycles.clone(),
            fluorescence: rn,
            corrected: None,
            quantity: "Rn",
        };
        assert_eq!(baseline_window(&c, (a, b)), Some((3, 13)));
        // a line no window gives
        assert_eq!(baseline_window(&c, (a + 0.1, b)), None);
        // a perfectly straight curve: every window gives the line, so none is chosen
        let flat = Curve {
            cycles: cycles.clone(),
            fluorescence: cycles.iter().map(|x| 2.0 + 0.5 * x).collect(),
            corrected: None,
            quantity: "Rn",
        };
        assert_eq!(baseline_window(&flat, (2.0, 0.5)), None);
    }

    #[test]
    fn lenient_json_nan() {
        let v = lenient_json(
            r#"{"a": NaN, "b": "NaN", "c": [1, -Infinity], "d": "é"}"#,
            "t",
        )
        .unwrap();
        assert!(v["a"].is_null());
        assert_eq!(v["b"], "NaN");
        assert!(v["c"][1].is_null());
        assert_eq!(v["d"], "é");
        assert!(lenient_json("{", "t").is_err());
    }

    #[test]
    fn points_and_lists() {
        let p = collection_points(
            "[[Stg:2 Cyc:1 Stp:2 Pt:1], [Stg:2 Cyc:2 Stp:2 Pt:1], [Stg:3 Cyc:1 Stp:3 Pt:1]]",
        );
        assert_eq!(p, vec![(2, 1, 2, 1), (2, 2, 2, 1), (3, 1, 3, 1)]);
        assert_eq!(number_list("[1.5, 2, NaN]").len(), 3);
        assert_eq!(number_list("1\t2\t3"), vec![1.0, 2.0, 3.0]);
        let pts: Vec<_> = (1..=40)
            .map(|c| (2, c, 2, 1))
            .chain((1..=30).map(|k| (3, 1, 3, k)))
            .collect();
        let (a, m) = select_points(&pts, None, None);
        assert_eq!((a.len(), m.len()), (40, 30));
    }

    #[test]
    fn text_results() {
        let t = "Session Name\t\nWell\tSample Name\tDetector\tTask\tCt\n0\tS\tGAPDH\tUNKNOWN\t21.5\nRn values\t1\t2\t3\nDelta Rn values\t0\t1\t2\n1\tS\tGAPDH\tNTC\tUndetermined\n";
        let (h, r) = parse_text_results(t);
        assert_eq!(h[2], "detector");
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].get(&["ct"]), Some("21.5"));
        assert_eq!(r[0].series("Rn values").unwrap().len(), 3);
        assert_eq!(cq_cell(r[1].get(&["ct"])), (None, true));
    }

    #[test]
    fn multicomponent_txt_records() {
        let t = "StepOne v2.0 MulticomponentData\n\nWELL\tCYCLE\tDYE LIST\tMSE\tSIGNAL DATA\tPURE_DYE_DATA\n0\t0\tFAM\t66.1\t842.7\t1.0\n\t\t\t\t\t0.06\n0\t0\tROX\t66.1\t363.0\t0.01\n0\t0\t66.1\t0\t0\t66.1\t0\t1\tFAM\t68.8\t844.7\t1.0\n0\t1\tROX\t68.8\t364.0\t0.0\n";
        let m = parse_multicomponent_txt(t);
        let w = &m.wells[&0];
        assert_eq!(w.dyes[0].0, "FAM");
        assert_eq!(w.dyes[0].1, vec![842.7, 844.7]);
        assert_eq!(w.dyes[1].1, vec![363.0, 364.0]);
    }

    #[test]
    fn instruments_and_geometry() {
        assert_eq!(json_instrument("QS7PRO"), "QuantStudio 7 Pro");
        assert_eq!(json_instrument("QS1"), "QuantStudio 1");
        assert_eq!(json_instrument("X"), "X");
        assert_eq!(geometry("TYPE_16X24"), Some((16, 24)));
        assert_eq!(geometry("BLOCK_96W_02ML"), Some((8, 12)));
        assert_eq!(
            ms_to_iso(1_535_469_164_418.0).as_deref(),
            Some("2018-08-28T15:12:44.418Z")
        );
        assert_eq!(ms_to_iso(0.0), None);
    }
}

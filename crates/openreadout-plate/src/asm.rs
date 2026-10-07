//! Allotrope Simple Model (ASM) plate-reader JSON output (`export --format asm`).
//!
//! Target: the public ASM plate-reader model `REC/2025/03`. The shape follows allotropy's
//! MIT-licensed output for the same exports (docs/provenance/plate-readers.md); the schema
//! itself is not copied into this project (its licence forbids distributing modified copies)
//! and validation happens in the oracle environment (`oracle/asm_validate.py`).
//!
//! One `plate reader document` per plate and well; one measurement document per measured read
//! (endpoint: a point value; kinetic: a profile cube over elapsed time; spectrum: a spectrum
//! cube over wavelength). Reads calculated by the vendor software become calculated-data
//! documents that cite their source measurements where one exists.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use xxhash_rust::xxh3::Xxh3;

use crate::dataset::PlateDataset;
use crate::grid::{parse_well, well_name};
use crate::model::{Block, Channel, Export, Mode, ReadType};

/// The ASM manifest our documents declare.
pub const ASM_MANIFEST: &str =
    "http://purl.allotrope.org/manifests/plate-reader/REC/2025/03/plate-reader.manifest";

/// Output of `export --format asm`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AsmExportReport {
    /// The plate-reader export that was converted.
    pub input: String,
    /// Where the ASM JSON document was written.
    pub output: String,
    /// Always `asm`.
    pub format: String,
    /// The ASM manifest IRI the document declares.
    pub manifest: String,
    /// `plate reader document` entries (one per plate and well).
    pub documents: u64,
    /// Measurement documents (one per well and measured read).
    pub measurements: u64,
    /// Numeric values carried by the measurement documents (cube points counted singly).
    pub values: u64,
    /// Non-numeric cells written as error documents.
    pub errors: u64,
    /// Values of reads calculated by the vendor software, written as calculated-data documents.
    pub calculated: u64,
    /// Size of the written document in bytes.
    pub bytes_written: u64,
    /// True when the written file was parsed back and its measured values matched the source.
    pub verified: bool,
    /// Caveats of the conversion (e.g. a time zone assumed because the export records none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn qty(v: f64, unit: &str) -> Value {
    json!({"value": v, "unit": unit})
}

/// ASM date-time: RFC 3339 needs a zone; exports record none, so UTC is stated (as allotropy
/// does) and the report says so.
fn asm_time(t: &str) -> String {
    let has_zone = t.len() > 19 && (t.ends_with('Z') || t[19..].contains(['+', '-']));
    if has_zone {
        t.to_string()
    } else if t.len() == 10 {
        format!("{t}T00:00:00+00:00")
    } else {
        format!("{t}+00:00")
    }
}

/// (point field, profile cube field, spectrum cube field, measure concept, unit, detection type)
fn mode_fields(
    m: Mode,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    match m {
        Mode::Absorbance => (
            "absorbance",
            "absorption profile data cube",
            "absorption spectrum data cube",
            "absorbance",
            "mAU",
            "Absorbance",
        ),
        Mode::Luminescence => (
            "luminescence",
            "luminescence profile data cube",
            "luminescence spectrum data cube",
            "luminescence",
            "RLU",
            "Luminescence",
        ),
        Mode::Alpha => (
            "luminescence",
            "luminescence profile data cube",
            "luminescence spectrum data cube",
            "luminescence",
            "RLU",
            "Alpha",
        ),
        Mode::Fluorescence | Mode::Unknown => (
            "fluorescence",
            "fluorescence emission profile data cube",
            "fluorescence emission spectrum data cube",
            "fluorescence",
            "RFU",
            "Fluorescence",
        ),
    }
}

fn device_control(ch: &Channel, b: &Block) -> Value {
    let (_, _, _, _, _, detection) = mode_fields(ch.mode);
    let mut d = Map::new();
    d.insert("device type".into(), json!("plate reader"));
    d.insert("detection type".into(), json!(detection));
    let det = if ch.mode == Mode::Absorbance {
        ch.wavelength_nm
    } else {
        ch.emission_nm.or(ch.wavelength_nm)
    };
    if let Some(w) = det.filter(|_| b.read_type != Some(ReadType::Spectrum)) {
        d.insert("detector wavelength setting".into(), qty(w, "nm"));
    }
    if let Some(w) = ch.excitation_nm {
        d.insert("excitation wavelength setting".into(), qty(w, "nm"));
    }
    let num = |k: &str| ch.settings.get(k).and_then(Value::as_f64);
    if let Some(n) = num("measurements_per_point")
        .or_else(|| num("flashes"))
        .or_else(|| {
            num("reads_per_well").or_else(|| {
                ch.settings
                    .get("reads_per_well")
                    .and_then(Value::as_str)
                    .and_then(crate::sheet::parse_number)
            })
        })
    {
        d.insert("number of averages".into(), qty(n, "#"));
    }
    if let Some(g) = ch
        .settings
        .get("gain")
        .or_else(|| ch.settings.get("pmt_gain"))
    {
        let g = match g {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        d.insert("detector gain setting".into(), json!(g));
    }
    if let Some(h) = num("read_height_mm") {
        d.insert(
            "detector distance setting (plate reader)".into(),
            qty(h, "mm"),
        );
    }
    if let Some(s) = ch.settings.get("read_speed").and_then(Value::as_str) {
        d.insert("detector carriage speed setting".into(), json!(s));
    }
    if let Some(o) = ch.settings.get("optics").and_then(Value::as_str) {
        let pos = if o.starts_with("Bottom") {
            "bottom scan position (plate reader)"
        } else {
            "top scan position (plate reader)"
        };
        d.insert("scan position setting (plate reader)".into(), json!(pos));
    }
    json!({"device control document": [Value::Object(d)]})
}

struct Ids {
    stem: String,
    n: u64,
}

impl Ids {
    fn next(&mut self) -> String {
        self.n += 1;
        format!("{}_{}", self.stem, self.n)
    }
}

/// Build the ASM document for an export. Returns (document, counts).
pub(crate) fn build(ex: &Export, input: &Path, output_name: &str) -> (Value, Counts) {
    let file_name = input
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut ids = Ids {
        stem: format!(
            "OPENREADOUT_{}",
            file_name
                .split('.')
                .next()
                .unwrap_or("export")
                .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        ),
        n: 0,
    };
    let mut counts = Counts::default();
    let mut docs = Vec::new();
    let mut calculated_docs = Vec::new();
    for b in &ex.blocks {
        let measured: Vec<usize> = (0..b.channels.len())
            .filter(|&i| !b.channels[i].calculated)
            .collect();
        // group observations: (row, col) -> channel -> obs
        let mut wells: BTreeMap<(u32, u32), BTreeMap<u32, Vec<&crate::model::Obs>>> =
            BTreeMap::new();
        for o in &b.obs {
            wells
                .entry((o.row, o.col))
                .or_default()
                .entry(o.channel)
                .or_default()
                .push(o);
        }
        // sample names: a layout of well ids (Gen5 `Well ID`, BMG `Layout`, SkanIt samples)
        let layout_ids: Option<&Map<String, Value>> = b.extra.get("layout").and_then(|l| {
            let m = l.as_object()?;
            if m.values().all(Value::is_string) {
                return Some(m); // a single layout: well -> name
            }
            m.iter()
                .find(|(k, _)| {
                    k.contains("Well ID")
                        || k.contains("Layout")
                        || k.contains("Sample")
                        || k.contains("Content")
                })
                .and_then(|(_, v)| v.as_object())
        });
        let time = b.started_at.clone().or_else(|| ex.acquired_at.clone());
        for ((r, c), by_ch) in &wells {
            let well = well_name(*r, *c);
            let sample_id = layout_ids
                .and_then(|m| m.get(&well))
                .and_then(Value::as_str)
                .map_or_else(|| format!("{} {well}", b.plate), str::to_string);
            let sample = json!({"sample identifier": sample_id, "location identifier": well, "well plate identifier": b.plate});
            let mut mdocs = Vec::new();
            let mut source_ids: BTreeMap<u32, String> = BTreeMap::new();
            for &ci in &measured {
                let Some(obs) = by_ch.get(&(ci as u32)) else {
                    continue;
                };
                let ch = &b.channels[ci];
                let (point, profile, spectrum, concept, unit, _) = mode_fields(ch.mode);
                let mut base = |ids: &mut Ids| {
                    let id = ids.next();
                    source_ids.entry(ci as u32).or_insert_with(|| id.clone());
                    let mut m = Map::new();
                    m.insert("measurement identifier".into(), json!(id));
                    m.insert("sample document".into(), sample.clone());
                    m.insert(
                        "device control aggregate document".into(),
                        device_control(ch, b),
                    );
                    if let Some(t) = ch
                        .settings
                        .get("temperature_c")
                        .and_then(Value::as_f64)
                        .or(b.temperature_c)
                    {
                        m.insert("compartment temperature".into(), qty(t, "degC"));
                    }
                    m
                };
                let cube = match b.read_type {
                    Some(ReadType::Kinetic) if obs.iter().any(|o| o.time_s.is_some()) => {
                        Some((profile, "elapsed time", "s", true))
                    }
                    Some(ReadType::Spectrum) if obs.iter().any(|o| o.wavelength_nm.is_some()) => {
                        Some((spectrum, "wavelength", "nm", false))
                    }
                    _ => None,
                };
                if cube.is_none() {
                    // endpoint: one document per value (repeats of one well and read stay separate)
                    for o in obs {
                        let mut m = base(&mut ids);
                        let v = if let Some(t) = &o.text {
                            m.insert(
                                "error aggregate document".into(),
                                json!({"error document": [{"error": t, "error feature": concept}]}),
                            );
                            counts.errors += 1;
                            -0.0
                        } else {
                            counts.values += 1;
                            counts.hash_value(*r, *c, o.value);
                            o.value
                        };
                        m.insert(point.into(), qty(v, unit));
                        counts.measurements += 1;
                        mdocs.push(Value::Object(m));
                    }
                    continue;
                }
                let mut m = base(&mut ids);
                let mut errors = Vec::new();
                if let Some((field, dim, dim_unit, by_time)) = cube {
                    let mut pts: Vec<(f64, f64)> = Vec::new();
                    for o in obs {
                        let x = if by_time { o.time_s } else { o.wavelength_nm };
                        let Some(x) = x else { continue };
                        if let Some(t) = &o.text {
                            let at =
                                format!("{concept} at {} {dim_unit}", crate::sheet::fmt_num(x));
                            errors.push(json!({"error": t, "error feature": at}));
                            counts.errors += 1;
                        } else {
                            pts.push((x, o.value));
                        }
                    }
                    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                    counts.values += pts.len() as u64;
                    for &(_, v) in &pts {
                        counts.hash_value(*r, *c, v);
                    }
                    m.insert(
                        field.into(),
                        json!({
                            "label": ch.label,
                            "cube-structure": {
                                "dimensions": [{"@componentDatatype": "double", "concept": dim, "unit": dim_unit}],
                                "measures": [{"@componentDatatype": "double", "concept": concept, "unit": unit}],
                            },
                            "data": {
                                "dimensions": [pts.iter().map(|p| p.0).collect::<Vec<_>>()],
                                "measures": [pts.iter().map(|p| p.1).collect::<Vec<_>>()],
                            },
                        }),
                    );
                }
                if !errors.is_empty() {
                    m.insert(
                        "error aggregate document".into(),
                        json!({"error document": errors}),
                    );
                }
                counts.measurements += 1;
                mdocs.push(Value::Object(m));
            }
            // calculated reads of this well
            for (ci, obs) in by_ch {
                let ch = &b.channels[*ci as usize];
                if !ch.calculated {
                    continue;
                }
                for o in obs.iter().filter(|o| o.text.is_none()) {
                    let mut d = Map::new();
                    d.insert("calculated data name".into(), json!(ch.label));
                    d.insert("calculated result".into(), qty(o.value, "(unitless)"));
                    d.insert("calculated data identifier".into(), json!(ids.next()));
                    if let Some(src) = source_ids.values().next() {
                        d.insert(
                            "data source aggregate document".into(),
                            json!({"data source document": [{"data source identifier": src, "data source feature": mode_fields(ch.mode).3}]}),
                        );
                    }
                    counts.calculated += 1;
                    calculated_docs.push(Value::Object(d));
                }
            }
            if mdocs.is_empty() {
                continue;
            }
            let mut agg = Map::new();
            agg.insert("measurement document".into(), json!(mdocs));
            if let Some(t) = &time {
                agg.insert("measurement time".into(), json!(asm_time(t)));
            }
            agg.insert(
                "plate well count".into(),
                qty(f64::from(b.rows * b.cols), "#"),
            );
            agg.insert("container type".into(), json!("well plate"));
            if let Some(p) = &ex.protocol {
                agg.insert("analytical method identifier".into(), json!(p));
            }
            if let Some(e) = &ex.experiment {
                agg.insert("experimental data identifier".into(), json!(e));
            }
            if let Some(o) = &ex.operator {
                agg.insert("analyst".into(), json!(o));
            }
            docs.push(json!({"measurement aggregate document": Value::Object(agg)}));
            counts.documents += 1;
        }
    }
    let mut device = Map::new();
    device.insert(
        "device identifier".into(),
        json!(ex.model.clone().unwrap_or_else(|| "N/A".into())),
    );
    device.insert(
        "model number".into(),
        json!(ex.model.clone().unwrap_or_else(|| "N/A".into())),
    );
    if let Some(s) = &ex.serial {
        device.insert("equipment serial number".into(), json!(s));
    }
    if let Some(m) = ex.kind.manufacturer() {
        device.insert("product manufacturer".into(), json!(m));
    }
    let mut data_system = Map::new();
    data_system.insert("ASM file identifier".into(), json!(output_name));
    data_system.insert("data system instance identifier".into(), json!("N/A"));
    data_system.insert("file name".into(), json!(file_name));
    data_system.insert("UNC path".into(), json!(input.display().to_string()));
    data_system.insert("ASM converter name".into(), json!("openreadout"));
    data_system.insert(
        "ASM converter version".into(),
        json!(env!("CARGO_PKG_VERSION")),
    );
    if let Some(s) = ex.kind.software() {
        data_system.insert("software name".into(), json!(s));
    }
    if let Some(v) = &ex.software_version {
        data_system.insert("software version".into(), json!(v));
    }
    let mut agg = Map::new();
    agg.insert("plate reader document".into(), json!(docs));
    agg.insert("device system document".into(), Value::Object(device));
    agg.insert("data system document".into(), Value::Object(data_system));
    if !calculated_docs.is_empty() {
        agg.insert(
            "calculated data aggregate document".into(),
            json!({"calculated data document": calculated_docs}),
        );
    }
    (
        json!({"$asm.manifest": ASM_MANIFEST, "plate reader aggregate document": Value::Object(agg)}),
        counts,
    )
}

/// Tallies of what the document holds, and a hash of its measured values.
#[derive(Debug, Default)]
pub(crate) struct Counts {
    pub(crate) documents: u64,
    pub(crate) measurements: u64,
    pub(crate) values: u64,
    pub(crate) errors: u64,
    pub(crate) calculated: u64,
    pairs: Vec<(u32, u32, u64)>,
}

impl Counts {
    fn hash_value(&mut self, r: u32, c: u32, v: f64) {
        let v = if v == 0.0 { 0.0 } else { v };
        self.pairs.push((r, c, v.to_bits()));
    }
    /// Order-independent digest of (well, value) pairs.
    pub(crate) fn digest(&mut self) -> u128 {
        self.pairs.sort_unstable();
        let mut h = Xxh3::new();
        for (r, c, v) in &self.pairs {
            h.update(&r.to_le_bytes());
            h.update(&c.to_le_bytes());
            h.update(&v.to_le_bytes());
        }
        h.digest128()
    }
}

/// Walk a parsed ASM document and tally its measured values the same way `build` does.
fn read_back(doc: &Value) -> Counts {
    let mut counts = Counts::default();
    let docs = doc["plate reader aggregate document"]["plate reader document"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for d in &docs {
        let agg = &d["measurement aggregate document"];
        counts.documents += 1;
        for m in agg["measurement document"].as_array().into_iter().flatten() {
            counts.measurements += 1;
            let Some((r, c)) = m["sample document"]["location identifier"]
                .as_str()
                .and_then(parse_well)
            else {
                continue;
            };
            let has_error = m.get("error aggregate document").is_some();
            for (k, v) in m.as_object().into_iter().flatten() {
                if k.ends_with("data cube") {
                    for x in v["data"]["measures"][0].as_array().into_iter().flatten() {
                        if let Some(x) = x.as_f64() {
                            counts.values += 1;
                            counts.hash_value(r, c, x);
                        }
                    }
                } else if matches!(k.as_str(), "absorbance" | "fluorescence" | "luminescence")
                    && let Some(x) = v["value"].as_f64()
                    && !(has_error && x == 0.0 && x.is_sign_negative())
                {
                    counts.values += 1;
                    counts.hash_value(r, c, x);
                }
            }
        }
    }
    counts
}

/// Write `ds` as ASM JSON to `output` (temporary file, read back and verified, then renamed).
pub fn export_asm(ds: &PlateDataset, output: &Path, overwrite: bool) -> Result<AsmExportReport> {
    if output.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let input = ds.path();
    if output == input {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    let out_name = output.file_name().map_or_else(
        || "export.json".into(),
        |n| n.to_string_lossy().into_owned(),
    );
    let (doc, mut counts) = build(ds.export(), input, &out_name);
    let tmp = output.with_file_name(format!(".{out_name}.partial-{}", std::process::id()));
    let result = (|| -> Result<u64> {
        let f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut w = BufWriter::new(f);
        serde_json::to_writer_pretty(&mut w, &doc)
            .map_err(|e| Error::Other(format!("ASM serialization failed: {e}")))?;
        w.write_all(b"\n").map_err(|e| Error::io(&tmp, e))?;
        w.flush().map_err(|e| Error::io(&tmp, e))?;
        drop(w);
        let text = std::fs::read_to_string(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let back: Value = serde_json::from_str(&text)
            .map_err(|e| Error::Other(format!("ASM read-back is not valid JSON: {e}")))?;
        let mut again = read_back(&back);
        if again.documents != counts.documents
            || again.measurements != counts.measurements
            || again.values != counts.values
            || again.digest() != counts.digest()
        {
            return Err(Error::Other(
                "ASM read-back did not match the source values".into(),
            ));
        }
        Ok(text.len() as u64)
    })();
    let bytes = match result {
        Ok(b) => b,
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            return Err(e);
        }
    };
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    let mut notes = Vec::new();
    if ds
        .export()
        .acquired_at
        .as_deref()
        .is_some_and(|t| t.len() <= 19 || !t[19..].contains(['+', '-', 'Z']))
    {
        notes.push(
            "the export records no time zone; ASM requires one, so measurement times state +00:00"
                .into(),
        );
    }
    if ds
        .export()
        .blocks
        .iter()
        .flat_map(|b| &b.channels)
        .any(|c| !c.calculated && c.mode == Mode::Absorbance)
    {
        notes.push("absorbance values are written as exported (OD) under the unit the ASM schema prescribes (mAU)".into());
    }
    Ok(AsmExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "asm".into(),
        manifest: ASM_MANIFEST.into(),
        documents: counts.documents,
        measurements: counts.measurements,
        values: counts.values,
        errors: counts.errors,
        calculated: counts.calculated,
        bytes_written: bytes,
        verified: true,
        notes,
    })
}

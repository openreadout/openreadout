//! RDML (Real-time PCR Data Markup Language) 1.0-1.4: a zip holding `rdml_data.xml`, or the bare
//! XML. Element names are the standard's (`docs/formats/qpcr.md`, provenance in
//! `docs/provenance/qpcr.md`).

use std::collections::BTreeMap;

use roxmltree::Node;
use serde_json::{Map, Value, json};

use openreadout_core::{Error, Result};

use crate::model::{
    Assay, Curve, Dialect, Dye, Melt, Person, Program, QpcrData, Reaction, Run, Sample, Stage,
    Step, Target, task_name,
};
use crate::xml::{child, children, id, node_text, number, parse, text};

pub(crate) const FORMAT_ID: &str = "rdml";

/// Pick the RDML document among zip members: `rdml_data.xml`, else the first `.xml` member whose
/// root element is `rdml` (instruments write other names and extra members).
pub(crate) fn main_member(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| n.eq_ignore_ascii_case("rdml_data.xml"))
        .cloned()
        .collect();
    out.extend(
        names
            .iter()
            .filter(|n| {
                !n.eq_ignore_ascii_case("rdml_data.xml")
                    && n.to_ascii_lowercase().ends_with(".xml")
                    && !n.contains('/')
            })
            .cloned(),
    );
    out
}

/// Does this text look like an RDML document (root element `rdml`)?
pub(crate) fn looks_like_rdml(head: &str) -> bool {
    let h = head.trim_start_matches('\u{feff}');
    let mut rest = h;
    // skip the declaration, comments and whitespace before the root element
    loop {
        rest = rest.trim_start();
        if rest.starts_with("<?") {
            match rest.find("?>") {
                Some(i) => rest = &rest[i + 2..],
                None => return false,
            }
        } else if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(i) => rest = &rest[i + 3..],
                None => return false,
            }
        } else {
            break;
        }
    }
    let r = rest.strip_prefix('<').unwrap_or("");
    let name: String = r
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect();
    name == "rdml" || name.ends_with(":rdml")
}

fn f(n: Node<'_, '_>, name: &str) -> Option<f64> {
    number(n, name)
}

/// RDML uses -1.0 for "not available" in `cq`, `N0`, `corrCq`, `corrP`.
fn not_available(v: Option<f64>) -> (Option<f64>, bool) {
    match v {
        Some(x) if (x + 1.0).abs() < 1e-12 => (None, true),
        other => (other, false),
    }
}

fn step(n: Node<'_, '_>) -> Step {
    let mut s = Step::default();
    if let Some(t) = child(n, "temperature") {
        s.kind = "temperature".into();
        s.temperature_c = f(t, "temperature");
        s.hold_s = f(t, "duration");
        s.ramp_c_per_s = f(t, "ramp");
        s.measure = text(t, "measure").map(|m| measure(&m));
    } else if let Some(g) = child(n, "gradient") {
        s.kind = "gradient".into();
        s.high_temperature_c = f(g, "highTemperature");
        s.low_temperature_c = f(g, "lowTemperature");
        s.hold_s = f(g, "duration");
        s.ramp_c_per_s = f(g, "ramp");
        s.measure = text(g, "measure").map(|m| measure(&m));
    } else if let Some(l) = child(n, "loop") {
        s.kind = "loop".into();
        s.goto = f(l, "goto").map(|v| v as u32);
        s.repeat = f(l, "repeat").map(|v| v as u32);
    } else if let Some(p) = child(n, "pause") {
        s.kind = "pause".into();
        s.temperature_c = f(p, "temperature");
    } else if child(n, "lidOpen").is_some() {
        s.kind = "lid open".into();
    } else {
        s.kind = "unknown".into();
    }
    s
}

fn measure(m: &str) -> String {
    match m.trim() {
        "meltcurve" => "melt".into(),
        other => other.to_string(),
    }
}

/// Position of a reaction id under a run's plate layout: 1-based integers (RDML 1.1+), or
/// labels such as `A1`, `P24` (RDML 1.0 and free formats). Returns zero-based (row, column).
pub(crate) fn react_position(rid: &str, rows: u32, columns: u32) -> Option<(u32, u32)> {
    let rid = rid.trim();
    if let Ok(n) = rid.parse::<u32>() {
        let n = n.checked_sub(1)?;
        let cols = columns.max(1);
        return Some((n / cols, n % cols));
    }
    let letters: String = rid.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits: String = rid[letters.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if letters.is_empty() || digits.is_empty() || letters.len() > 2 {
        return None;
    }
    let mut row = 0u32;
    for c in letters.to_ascii_uppercase().bytes() {
        row = row.checked_mul(26)?.checked_add(u32::from(c - b'A') + 1)?;
    }
    let col: u32 = digits.parse().ok()?;
    let _ = rows;
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

/// The v1.0 `pcrFormat` string (`96-well plate; A1-H12`, `72-well rotor; 1-72`, `free format`).
fn v10_format(s: &str) -> (u32, u32, &'static str, &'static str) {
    let s = s.trim();
    match s {
        _ if s.starts_with("single-well") => (1, 1, "123", "123"),
        _ if s.starts_with("48-well") => (6, 8, "ABC", "123"),
        _ if s.starts_with("96-well") => (8, 12, "ABC", "123"),
        _ if s.starts_with("384-well") => (16, 24, "ABC", "123"),
        _ if s.starts_with("32-well rotor") => (32, 1, "123", "123"),
        _ if s.starts_with("72-well rotor") => (72, 1, "123", "123"),
        _ if s.starts_with("100-well rotor") => (100, 1, "123", "123"),
        _ => (0, 0, "ABC", "123"),
    }
}

/// Parse an RDML document.
pub(crate) fn parse_rdml(xml: &str) -> Result<QpcrData> {
    let doc = parse(xml, FORMAT_ID, "RDML document")?;
    let root = doc.root_element();
    if root.tag_name().name() != "rdml" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "the XML root element is <{}>, not <rdml>",
                root.tag_name().name()
            ),
        ));
    }
    let mut d = QpcrData::new(Dialect::Rdml);
    d.format_version = root.attribute("version").map(str::to_string);
    d.created_at = text(root, "dateMade");
    for e in children(root, "experimenter") {
        d.experimenters.push(Person {
            id: id(e).unwrap_or_default(),
            first_name: text(e, "firstName"),
            last_name: text(e, "lastName"),
            email: text(e, "email"),
            lab: text(e, "labName"),
        });
    }
    for y in children(root, "dye") {
        d.dyes.push(Dye {
            name: id(y).unwrap_or_default(),
            chemistry: text(y, "dyeChemistry"),
        });
    }
    for s in children(root, "sample") {
        let mut smp = Sample {
            name: id(s).unwrap_or_default(),
            description: text(s, "description"),
            ..Sample::default()
        };
        for t in children(s, "type") {
            let kind = node_text(t).map(|k| task_name(&k));
            match t.attribute("targetId") {
                Some(tid) => smp.per_target.push((tid.to_string(), kind, None)),
                None => {
                    if smp.kind.is_none() {
                        smp.kind = kind;
                    }
                }
            }
        }
        for q in children(s, "quantity") {
            let v = number(q, "value");
            match q.attribute("targetId") {
                Some(tid) => {
                    if let Some(p) = smp.per_target.iter_mut().find(|p| p.0 == tid) {
                        p.2 = v;
                    } else {
                        smp.per_target.push((tid.to_string(), None, v));
                    }
                }
                None => {
                    if smp.quantity.is_none() {
                        smp.quantity = v;
                        smp.quantity_unit = text(q, "unit");
                    }
                }
            }
        }
        if smp.kind.is_none() && smp.per_target.iter().all(|p| p.1.is_none()) {
            // RDML: an absent type means "unkn".
            smp.kind = Some("unknown".into());
        }
        for a in children(s, "annotation") {
            if let (Some(p), Some(v)) = (text(a, "property"), text(a, "value")) {
                smp.annotations.push((p, v));
            }
        }
        d.samples.push(smp);
    }
    for t in children(root, "target") {
        let mut tg = Target {
            name: id(t).unwrap_or_default(),
            description: text(t, "description"),
            kind: text(t, "type").map(|k| match k.as_str() {
                "ref" => "reference".to_string(),
                "toi" => "of interest".to_string(),
                o => o.to_string(),
            }),
            efficiency: f(t, "amplificationEfficiency"),
            efficiency_se: f(t, "amplificationEfficiencySE"),
            efficiency_method: text(t, "amplificationEfficiencyMethod"),
            melting_temperature: f(t, "meltingTemperature"),
            dye: child(t, "dyeId").and_then(|n| id(n).or_else(|| node_text(n))),
            ..Target::default()
        };
        // Efficiency is a fold per cycle (1.0-2.x); some exporters write a percentage.
        if let Some(e) = tg.efficiency
            && e > 10.0
        {
            d.notes.push(format!(
                "target {}: amplificationEfficiency {e} read as a percentage ({:.4} fold)",
                tg.name,
                1.0 + e / 100.0
            ));
            tg.efficiency = Some(1.0 + e / 100.0);
        }
        if let Some(sq) = child(t, "sequences") {
            for kind in [
                "forwardPrimer",
                "reversePrimer",
                "probe1",
                "probe2",
                "amplicon",
            ] {
                if let Some(seq) = child(sq, kind).and_then(|n| text(n, "sequence")) {
                    tg.sequences.push((kind.to_string(), seq));
                }
            }
        }
        d.targets.push(tg);
    }
    for c in children(root, "thermalCyclingConditions") {
        let steps: Vec<Step> = children(c, "step").map(step).collect();
        d.programs.push(Program {
            name: id(c).unwrap_or_default(),
            description: text(c, "description"),
            lid_temperature_c: f(c, "lidTemperature"),
            stages: vec![Stage {
                kind: "program".into(),
                repeats: 1,
                steps,
            }],
            ..Program::default()
        });
    }
    let target_dye: BTreeMap<String, Option<String>> = d
        .targets
        .iter()
        .map(|t| (t.name.clone(), t.dye.clone()))
        .collect();
    let mut partitions = 0usize;
    for e in children(root, "experiment") {
        let exp = id(e);
        for r in children(e, "run") {
            let mut run = Run {
                name: id(r).unwrap_or_default(),
                experiment: exp.clone(),
                description: text(r, "description"),
                instrument: text(r, "instrument"),
                software: child(r, "dataCollectionSoftware").map(|s| {
                    [text(s, "name"), text(s, "version")]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                }),
                started_at: text(r, "runDate"),
                cq_method: text(r, "cqDetectionMethod"),
                background_method: text(r, "backgroundDeterminationMethod"),
                row_label: "ABC".into(),
                column_label: "123".into(),
                ..Run::default()
            };
            if let Some(tc) = child(r, "thermalCyclingConditions").and_then(id) {
                run.program = d.programs.iter().position(|p| p.name == tc);
            }
            if let Some(pf) = child(r, "pcrFormat") {
                if child(pf, "rows").is_some() {
                    run.rows = number(pf, "rows").map_or(0, |v| v as u32);
                    run.columns = number(pf, "columns").map_or(0, |v| v as u32);
                    run.row_label = text(pf, "rowLabel").unwrap_or_else(|| "ABC".into());
                    run.column_label = text(pf, "columnLabel").unwrap_or_else(|| "123".into());
                } else if let Some(s) = node_text(pf) {
                    let (rows, cols, rl, cl) = v10_format(&s);
                    run.rows = rows;
                    run.columns = cols;
                    run.row_label = rl.into();
                    run.column_label = cl.into();
                }
            }
            let mut by_pos: BTreeMap<(u32, u32), Reaction> = BTreeMap::new();
            let mut unplaced = 0usize;
            for rx in children(r, "react") {
                let rid = rx.attribute("id").unwrap_or("");
                let Some((row, col)) = react_position(rid, run.rows, run.columns) else {
                    unplaced += 1;
                    continue;
                };
                let sample = child(rx, "sample").and_then(id);
                if child(rx, "partitions").is_some() {
                    partitions += 1;
                }
                let reaction = by_pos.entry((row, col)).or_insert_with(|| Reaction {
                    row,
                    column: col,
                    sample: sample.clone(),
                    ..Reaction::default()
                });
                let smp = sample
                    .as_deref()
                    .and_then(|s| d.samples.iter().find(|x| x.name == s));
                for data in children(rx, "data") {
                    let target = child(data, "tar").and_then(id);
                    let (cq, undetermined) = not_available(f(data, "cq"));
                    let (n0, _) = not_available(f(data, "N0"));
                    let mut a = Assay {
                        dye: target
                            .as_ref()
                            .and_then(|t| target_dye.get(t).cloned().flatten()),
                        task: smp.and_then(|s| s.kind_for(target.as_deref())),
                        quantity: smp.and_then(|s| s.quantity_for(target.as_deref())),
                        target,
                        cq,
                        cq_undetermined: undetermined,
                        n0,
                        efficiency: f(data, "ampEff"),
                        efficiency_se: f(data, "ampEffSE"),
                        tm: f(data, "meltTemp").into_iter().collect(),
                        excluded: text(data, "excl"),
                        note: text(data, "note"),
                        background: f(data, "bgFluor"),
                        background_slope: f(data, "bgFluorSlp"),
                        threshold: f(data, "quantFluor"),
                        threshold_used: child(data, "quantFluor").is_some(),
                        // RDML 1.0 keeps a calculated quantity in the data element.
                        calculated_quantity: child(data, "quantity")
                            .and_then(|q| number(q, "value")),
                        ..Assay::default()
                    };
                    let mut cyc = Vec::new();
                    let mut flu = Vec::new();
                    for p in children(data, "adp") {
                        if let (Some(c), Some(v)) = (f(p, "cyc"), f(p, "fluor")) {
                            cyc.push(c);
                            flu.push(v);
                        }
                    }
                    if !cyc.is_empty() {
                        a.amplification = Some(Curve {
                            cycles: cyc,
                            fluorescence: flu,
                            corrected: None,
                            quantity: "fluorescence",
                        });
                    }
                    let mut tmp = Vec::new();
                    let mut mf = Vec::new();
                    for p in children(data, "mdp") {
                        if let (Some(t), Some(v)) = (f(p, "tmp"), f(p, "fluor")) {
                            tmp.push(t);
                            mf.push(v);
                        }
                    }
                    if !tmp.is_empty() {
                        a.melt = Some(Melt {
                            temperature: tmp,
                            fluorescence: mf,
                            derivative: None,
                        });
                    }
                    reaction.assays.push(a);
                }
            }
            if unplaced > 0 {
                d.notes.push(format!(
                    "run {}: {unplaced} reactions have ids that give no plate position and were skipped",
                    run.name
                ));
            }
            // Free formats: size the plate from the positions used.
            if run.rows == 0 || run.columns == 0 {
                run.rows = by_pos.keys().map(|k| k.0 + 1).max().unwrap_or(0);
                run.columns = by_pos.keys().map(|k| k.1 + 1).max().unwrap_or(0);
            }
            let mut reactions: Vec<Reaction> = by_pos.into_values().collect();
            for rx in &mut reactions {
                rx.position = rx.row.saturating_mul(run.columns).saturating_add(rx.column);
            }
            run.reactions = reactions;
            d.runs.push(run);
        }
    }
    if partitions > 0 {
        d.notes.push(format!(
            "{partitions} reactions hold digital-PCR partition data, which is not read (the counts are in the RDML file)"
        ));
    }
    name_guid_samples(&mut d);
    d.vendor = vendor_tree(root);
    Ok(d)
}

/// Whether an identifier is a GUID (`8-4-4-4-12` hexadecimal digits).
pub(crate) fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// LightCycler 96 names samples by a GUID `id` and keeps the name its software shows in the
/// sample's `description`. Such samples (with a non-empty description) take the description as
/// their name, the GUID kept as `id`, when all sample names stay unique; otherwise nothing is
/// renamed.
fn name_guid_samples(d: &mut QpcrData) {
    let renames: Vec<(usize, String)> = d
        .samples
        .iter()
        .enumerate()
        .filter(|(_, s)| is_guid(&s.name))
        .filter_map(|(i, s)| {
            s.description
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| (i, t.to_string()))
        })
        .collect();
    if renames.is_empty() {
        return;
    }
    let mut names: Vec<&str> = d
        .samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            renames
                .iter()
                .find(|r| r.0 == i)
                .map_or(s.name.as_str(), |r| r.1.as_str())
        })
        .collect();
    names.sort_unstable();
    let unique = names.windows(2).all(|w| w[0] != w[1]);
    if !unique {
        d.notes.push(
            "sample ids are GUIDs whose descriptions are not unique: samples keep their ids".into(),
        );
        return;
    }
    let map: BTreeMap<String, String> = renames
        .iter()
        .map(|(i, n)| (d.samples[*i].name.clone(), n.clone()))
        .collect();
    for (i, n) in renames {
        let s = &mut d.samples[i];
        s.id = Some(std::mem::replace(&mut s.name, n));
    }
    for run in &mut d.runs {
        for rx in &mut run.reactions {
            if let Some(n) = rx.sample.as_ref().and_then(|s| map.get(s)) {
                rx.sample = Some(n.clone());
            }
        }
    }
}

/// The RDML tree as JSON without the data points (`adp`/`mdp` are counted, not copied).
fn vendor_tree(root: Node<'_, '_>) -> Value {
    fn conv(n: Node<'_, '_>, depth: u32) -> Value {
        if depth > 64 {
            return Value::String("<nested too deep>".into());
        }
        let mut obj = Map::new();
        for a in n.attributes() {
            obj.insert(format!("@{}", a.name()), Value::String(a.value().into()));
        }
        let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
        for c in n.children().filter(Node::is_element) {
            let name = c.tag_name().name();
            if matches!(name, "adp" | "mdp") {
                *counts.entry(name).or_default() += 1;
                continue;
            }
            let v = conv(c, depth + 1);
            match obj.get_mut(name) {
                Some(Value::Array(a)) => a.push(v),
                Some(prev) => {
                    let p = prev.take();
                    *prev = Value::Array(vec![p, v]);
                }
                None => {
                    obj.insert(name.to_string(), v);
                }
            }
        }
        for (k, v) in counts {
            obj.insert(format!("{k}_count"), json!(v));
        }
        if obj.is_empty() {
            return node_text(n).map_or(Value::Null, Value::String);
        }
        if let Some(t) = node_text(n) {
            obj.insert("#text".into(), Value::String(t));
        }
        Value::Object(obj)
    }
    let mut top = Map::new();
    top.insert("rdml".into(), conv(root, 0));
    Value::Object(top)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rdml xmlns="http://www.rdml.org" version="1.3">
  <dateMade>2024-01-02T03:04:05</dateMade>
  <experimenter id="AB"><firstName>Ada</firstName><lastName>B</lastName></experimenter>
  <dye id="FAM"/>
  <sample id="S1"><type>unkn</type></sample>
  <sample id="NTC"><type>ntc</type></sample>
  <sample id="Std1"><type>std</type><quantity><value>1000</value><unit>cop</unit></quantity></sample>
  <target id="GAPDH"><type>ref</type><amplificationEfficiency>1.95</amplificationEfficiency><dyeId id="FAM"/></target>
  <thermalCyclingConditions id="p">
    <step><nr>1</nr><temperature><temperature>95</temperature><duration>600</duration></temperature></step>
    <step><nr>2</nr><temperature><temperature>95</temperature><duration>15</duration></temperature></step>
    <step><nr>3</nr><temperature><temperature>60</temperature><duration>60</duration><measure>real time</measure></temperature></step>
    <step><nr>4</nr><loop><goto>2</goto><repeat>39</repeat></loop></step>
    <step><nr>5</nr><gradient><highTemperature>95</highTemperature><lowTemperature>60</lowTemperature><duration>300</duration><measure>meltcurve</measure></gradient></step>
  </thermalCyclingConditions>
  <experiment id="E"><run id="R1">
    <thermalCyclingConditions id="p"/>
    <pcrFormat><rows>8</rows><columns>12</columns><rowLabel>ABC</rowLabel><columnLabel>123</columnLabel></pcrFormat>
    <react id="1"><sample id="S1"/><data><tar id="GAPDH"/><cq>21.5</cq><meltTemp>82.1</meltTemp>
      <adp><cyc>1</cyc><fluor>1.0</fluor></adp><adp><cyc>2</cyc><fluor>1.5</fluor></adp>
      <mdp><tmp>60</tmp><fluor>9</fluor></mdp><mdp><tmp>61</tmp><fluor>8</fluor></mdp></data></react>
    <react id="14"><sample id="NTC"/><data><tar id="GAPDH"/><cq>-1.0</cq><excl>no amplification</excl></data></react>
    <react id="13"><sample id="Std1"/><data><tar id="GAPDH"/><cq>18</cq></data></react>
  </run></experiment>
</rdml>"#;

    #[test]
    fn parses_a_small_document() {
        let d = parse_rdml(DOC).unwrap();
        assert_eq!(d.format_version.as_deref(), Some("1.3"));
        assert_eq!(d.samples.len(), 3);
        assert_eq!(d.targets[0].dye.as_deref(), Some("FAM"));
        let p = &d.programs[0];
        assert_eq!(p.cycles(), Some(40));
        assert_eq!(p.acquisition_temperature(), Some(60.0));
        let run = &d.runs[0];
        assert_eq!(run.reactions.len(), 3);
        let a1 = &run.reactions[0];
        assert_eq!((a1.row, a1.column, a1.position), (0, 0, 0));
        assert_eq!(a1.assays[0].cq, Some(21.5));
        assert_eq!(a1.assays[0].tm, vec![82.1]);
        assert_eq!(
            a1.assays[0].amplification.as_ref().unwrap().fluorescence,
            vec![1.0, 1.5]
        );
        assert_eq!(
            a1.assays[0].melt.as_ref().unwrap().temperature,
            vec![60.0, 61.0]
        );
        let b1 = &run.reactions[1];
        assert_eq!((b1.row, b1.column), (1, 0));
        assert_eq!(b1.assays[0].task.as_deref(), Some("standard"));
        assert_eq!(b1.assays[0].quantity, Some(1000.0));
        let b2 = &run.reactions[2];
        assert_eq!(b2.assays[0].cq, None);
        assert!(b2.assays[0].cq_undetermined);
        assert_eq!(b2.assays[0].task.as_deref(), Some("ntc"));
        assert_eq!(run.well_name(b2.row, b2.column), "B2");
    }

    #[test]
    fn v10_ids_and_formats() {
        assert_eq!(react_position("A1", 8, 12), Some((0, 0)));
        assert_eq!(react_position("H12", 8, 12), Some((7, 11)));
        assert_eq!(react_position("13", 8, 12), Some((1, 0)));
        assert_eq!(react_position("0", 8, 12), None);
        assert_eq!(react_position("x", 8, 12), None);
        assert_eq!(v10_format("96-well plate; A1-H12").0, 8);
        assert!(looks_like_rdml(
            "<?xml version='1.0'?>\n<rdml version=\"1.2\">"
        ));
        assert!(!looks_like_rdml("<html>"));
    }

    #[test]
    fn rejects_other_xml() {
        assert_eq!(parse_rdml("<foo/>").unwrap_err().exit_code(), 4);
        assert_eq!(parse_rdml("<rdml").unwrap_err().exit_code(), 4);
    }
}

//! Qiagen Rotor-Gene run files (`.rex`, XML): samples by tube, groups (targets), raw channel
//! readings (cycling and melt) and the thermal profile. No analysis results are stored.
//! Provenance: `docs/provenance/qpcr.md`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use openreadout_core::{Error, Result};

use crate::model::{
    Assay, Curve, Dialect, Melt, Program, QpcrData, Reaction, Run, Sample, Stage, Step, Target, num,
};
use crate::xml::{child, children, node_text, number, parse, text};

pub(crate) const FORMAT_ID: &str = "rotor-gene-rex";

/// Sample type codes (RDML R package `fromRotorGene`, MIT): 1 standard, 3 NTC, 5 positive.
fn sample_kind(code: &str) -> &'static str {
    match code.trim() {
        "1" => "standard",
        "3" => "ntc",
        "5" => "positive",
        _ => "unknown",
    }
}

/// One raw channel of a run: cycling or melt readings of one colour, one list per tube.
struct Channel {
    melt: bool,
    color: String,
    start: f64,
    step: f64,
    readings: Vec<Vec<f64>>,
}

/// Parse a `.rex` document.
pub(crate) fn parse_rex(xml: &str) -> Result<QpcrData> {
    let doc = parse(xml, FORMAT_ID, ".rex document")?;
    let root = doc.root_element();
    if root.tag_name().name() != "Experiment" || child(root, "RexHeader").is_none() {
        return Err(Error::corrupt(
            FORMAT_ID,
            "not a Rotor-Gene run file (no <Experiment> root with a <RexHeader>)",
        ));
    }
    let mut d = QpcrData::new(Dialect::Rex);
    d.format_version = text(root, "RexHeader");
    d.instrument.manufacturer = Some("Qiagen".into());
    d.instrument.software = Some("Rotor-Gene Q Series Software".into());
    d.operator = text(root, "Operator");
    d.name = text(root, "RunId");
    d.description = text(root, "Notes");
    d.started_at = text(root, "StartTime");
    d.ended_at = text(root, "FinishTime");
    let mut vendor = serde_json::Map::new();
    // samples: tube position → (sample id, name, kind, given concentration)
    let mut tubes: BTreeMap<u32, (String, Option<String>, &'static str, Option<f64>)> =
        BTreeMap::new();
    let mut groups: BTreeMap<String, String> = BTreeMap::new();
    if let Some(ss) = child(root, "Samples") {
        for page in children(ss, "Page") {
            for s in children(page, "Sample") {
                let Some(pos) = number(s, "TubePosition").map(|v| v as u32) else {
                    continue;
                };
                let id = text(s, "ID").unwrap_or_default();
                let name = text(s, "Name");
                let kind = sample_kind(&text(s, "Type").unwrap_or_default());
                let conc = number(s, "GivenConc").filter(|c| *c > 0.0);
                tubes.insert(pos, (id, name, kind, conc));
            }
        }
        if let Some(gs) = child(ss, "Groups") {
            for g in children(gs, "Group") {
                let Some(name) = text(g, "Name") else {
                    continue;
                };
                for t in children(g, "Tube") {
                    if let Some(id) = node_text(t) {
                        groups.insert(id, name.clone());
                    }
                }
            }
        }
    }
    // fields of the run record (rotor type, audit trail)
    let mut rotor = None;
    if let Some(rec) = child(root, "Record") {
        let mut fields = Vec::new();
        for f in children(rec, "Field") {
            let (n, v) = (
                text(f, "Name").unwrap_or_default(),
                text(f, "Value").unwrap_or_default(),
            );
            if n == "Rotor Type" {
                rotor = Some(v.clone());
            }
            fields.push(json!({"name": n, "value": v, "group": text(f, "Group")}));
        }
        vendor.insert("record".into(), Value::Array(fields));
    }
    // thermal profile
    let mut p = Program {
        name: "profile".into(),
        sample_volume_ul: number(root, "ReactionVolume"),
        ..Program::default()
    };
    if let Some(pr) = child(root, "Profile") {
        for n in pr.children().filter(roxmltree::Node::is_element) {
            match n.tag_name().name() {
                "HoldCycle" => p.stages.push(Stage {
                    kind: "hold".into(),
                    repeats: 1,
                    steps: vec![Step {
                        kind: "temperature".into(),
                        temperature_c: number(n, "Temperature"),
                        hold_s: number(n, "HoldFor"),
                        ..Step::default()
                    }],
                }),
                "Cycle" => p.stages.push(Stage {
                    kind: "cycling".into(),
                    repeats: number(n, "RepeatCount").map_or(1, |v| v as u32),
                    steps: children(n, "NormalCyclePoint")
                        .map(|c| Step {
                            kind: "temperature".into(),
                            temperature_c: number(c, "Temperature"),
                            hold_s: number(c, "RemainFor"),
                            measure: child(c, "AcquireTo").map(|_| "real time".into()),
                            ..Step::default()
                        })
                        .collect(),
                }),
                "MeltCycle" => p.stages.push(Stage {
                    kind: "melt".into(),
                    repeats: 1,
                    steps: vec![Step {
                        kind: "gradient".into(),
                        low_temperature_c: number(n, "StartTemp"),
                        high_temperature_c: number(n, "EndTemp"),
                        hold_s: number(n, "HoldFor"),
                        measure: Some("melt".into()),
                        ..Step::default()
                    }],
                }),
                _ => {}
            }
        }
    }
    if !p.stages.is_empty() {
        d.programs.push(p);
    }
    // raw channels: "Cycling A.Green", "Melt A.Green"
    let mut channels = Vec::new();
    if let Some(rc) = child(root, "RawChannels") {
        for c in children(rc, "RawChannel") {
            let name = text(c, "Name").unwrap_or_default();
            let color = name.rsplit('.').next().unwrap_or(&name).trim().to_string();
            let melt = name.to_ascii_lowercase().starts_with("melt");
            let cycling = name.to_ascii_lowercase().starts_with("cycling");
            let readings: Vec<Vec<f64>> = children(c, "Reading")
                .map(|r| {
                    node_text(r)
                        .unwrap_or_default()
                        .split_whitespace()
                        .map(|v| num(v).unwrap_or(f64::NAN))
                        .collect()
                })
                .collect();
            if let Some(a) = vendor
                .entry("raw_channels")
                .or_insert_with(|| json!([]))
                .as_array_mut()
            {
                a.push(json!({"name": name, "readings": readings.len()}));
            }
            if !(melt || cycling) {
                continue;
            }
            channels.push(Channel {
                melt,
                color,
                start: number(c, "StartX").unwrap_or(1.0),
                step: number(c, "StepX").unwrap_or(1.0),
                readings,
            });
        }
    }
    let tube_count = channels
        .iter()
        .map(|c| c.readings.len())
        .max()
        .unwrap_or(0)
        .max(tubes.keys().max().map_or(0, |m| *m as usize));
    let rotor_size = rotor
        .as_deref()
        .and_then(|r| r.split('-').next())
        .and_then(|n| n.trim().parse::<u32>().ok())
        .unwrap_or(tube_count as u32);
    let mut reactions = Vec::new();
    for i in 0..tube_count {
        let pos = i as u32 + 1;
        let tube = tubes.get(&pos);
        let sample = tube.and_then(|t| t.1.clone());
        let target = tube.and_then(|t| groups.get(&t.0).cloned());
        let mut rx = Reaction {
            position: i as u32,
            row: i as u32,
            column: 0,
            sample: sample.clone(),
            ..Reaction::default()
        };
        for c in channels.iter().filter(|c| !c.melt) {
            let Some(vals) = c.readings.get(i).filter(|v| !v.is_empty()) else {
                continue;
            };
            let mut a = Assay {
                target: target.clone(),
                dye: Some(c.color.clone()),
                task: tube.map(|t| t.2.to_string()),
                quantity: tube.and_then(|t| t.3),
                amplification: Some(Curve {
                    cycles: (0..vals.len())
                        .map(|k| c.start + c.step * k as f64)
                        .collect(),
                    fluorescence: vals.clone(),
                    corrected: None,
                    quantity: "fluorescence",
                }),
                ..Assay::default()
            };
            if let Some(m) = channels.iter().find(|m| m.melt && m.color == c.color)
                && let Some(mv) = m.readings.get(i).filter(|v| !v.is_empty())
            {
                a.melt = Some(Melt {
                    temperature: (0..mv.len()).map(|k| m.start + m.step * k as f64).collect(),
                    fluorescence: mv.clone(),
                    derivative: None,
                });
            }
            rx.assays.push(a);
        }
        if rx.assays.is_empty() && sample.is_none() {
            continue;
        }
        reactions.push(rx);
    }
    for (_, name, kind, conc) in tubes.values() {
        if let Some(n) = name
            && !d.samples.iter().any(|s| &s.name == n)
        {
            d.samples.push(Sample {
                name: n.clone(),
                kind: Some((*kind).to_string()),
                quantity: *conc,
                ..Sample::default()
            });
        }
    }
    let mut group_names: Vec<&String> = groups.values().collect();
    group_names.sort();
    group_names.dedup();
    for g in group_names {
        d.targets.push(Target {
            name: g.clone(),
            ..Target::default()
        });
    }
    d.notes.push("Rotor-Gene run files keep raw readings only: no Cq, threshold or Tm from the vendor analysis (compute Cq with `analyze qpcr --compute-cq`)".into());
    d.runs.push(Run {
        name: d.name.clone().unwrap_or_else(|| "run".into()),
        instrument: Some("Rotor-Gene".into()),
        started_at: d.started_at.clone(),
        program: (!d.programs.is_empty()).then_some(0),
        rows: rotor_size.max(1),
        columns: 1,
        row_label: "123".into(),
        column_label: "123".into(),
        reactions,
        ..Run::default()
    });
    d.vendor = Value::Object(vendor);
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_rex() {
        let x = "<?xml version=\"1.0\"?><Experiment><RexHeader>REX 3.15</RexHeader><StartTime>2021-01-18T10:20:30</StartTime>\
            <Samples><Page><Sample><ID>1</ID><Name>S1</Name><Type>2</Type><GivenConc>0</GivenConc><TubePosition>1</TubePosition></Sample>\
            <Sample><ID>2</ID><Name>NTC</Name><Type>3</Type><TubePosition>2</TubePosition></Sample></Page></Samples>\
            <RawChannels><RawChannel><Name>Cycling A.Green</Name><StartX>1</StartX><StepX>1</StepX>\
            <Reading>1 2 3</Reading><Reading>4 5 6</Reading></RawChannel>\
            <RawChannel><Name>Melt A.Green</Name><StartX>55</StartX><StepX>1</StepX><Reading>9 8</Reading><Reading>7 6</Reading></RawChannel></RawChannels>\
            <Profile><Cycle><RepeatCount>40</RepeatCount><NormalCyclePoint><Temperature>95</Temperature><RemainFor>10</RemainFor></NormalCyclePoint>\
            <NormalCyclePoint><Temperature>60</Temperature><RemainFor>30</RemainFor><AcquireTo><ID>1</ID></AcquireTo></NormalCyclePoint></Cycle></Profile></Experiment>";
        let d = parse_rex(x).unwrap();
        let r = &d.runs[0];
        assert_eq!(r.reactions.len(), 2);
        assert_eq!(r.well_name(1, 0), "2");
        let a = &r.reactions[1].assays[0];
        assert_eq!(a.task.as_deref(), Some("ntc"));
        assert_eq!(
            a.amplification.as_ref().unwrap().fluorescence,
            vec![4.0, 5.0, 6.0]
        );
        assert_eq!(a.melt.as_ref().unwrap().temperature, vec![55.0, 56.0]);
        assert_eq!(d.programs[0].cycles(), Some(40));
        assert_eq!(d.programs[0].acquisition_temperature(), Some(60.0));
        assert!(parse_rex("<Experiment/>").is_err());
    }
}

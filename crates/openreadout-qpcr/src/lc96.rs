//! Roche LightCycler 96 experiment files (`.lc96p`): an RDML 1.1 zip (read by [`crate::rdml`])
//! with Roche members next to `rdml_data.xml`. `app_data.xml` maps each reaction to one
//! "fact graph" per target (and says whether it is excluded); `calculated_data.xml` holds the
//! LightCycler 96 software's analysis: a call per graph, and in the relative- or
//! absolute-quantification analysis each graph's Cq (none for a graph it shows without one)
//! and each replicate group's Cq mean and error (standard deviation).
//! Those are attached to the reactions as vendor results. Provenance: `docs/provenance/qpcr.md`
//! (2026-09-26).

use std::collections::BTreeMap;

use roxmltree::Node;
use serde_json::{Value, json};

use openreadout_core::Result;

use crate::model::{QpcrData, num};
use crate::xml::{child, children, node_text, parse, text};
use openreadout_core::zip::ZipIndex;

/// One reaction × target of the LightCycler 96 software.
#[derive(Debug, Clone, Default)]
struct Graph {
    run: String,
    react: String,
    target: Option<String>,
    excluded: bool,
    call: Option<String>,
    /// In an analysis of the software (relative or absolute quantification).
    analysed: bool,
    /// The analysis' Cq (none for a graph the software shows without one).
    cq: Option<f64>,
}

/// Our amplification status for a LightCycler 96 call.
fn status(call: &str) -> String {
    match call.trim() {
        "Positive" => "amplified".into(),
        "Negative" => "not amplified".into(),
        other => other.to_ascii_lowercase(),
    }
}

/// Every descendant element with one of these local names.
fn descendants<'a, 'i>(
    n: Node<'a, 'i>,
    names: &'a [&'a str],
) -> impl Iterator<Item = Node<'a, 'i>> {
    n.descendants()
        .filter(move |c| c.is_element() && names.contains(&c.tag_name().name()))
}

/// Per-graph rows of the software's analyses: relative (`relQuantDataSource`) and absolute
/// (`absQuantDataSource`) quantification.
const DATA_SOURCES: &[&str] = &["relQuantDataSource", "absQuantDataSource"];
/// Replicate-group rows of the same analyses.
const STATISTICAL_ROWS: &[&str] = &["relQuantStatisticalRow", "absQuantStatisticalRow"];

/// Attach the LightCycler 96 software's results when the zip holds its members. Returns whether
/// it did. A member that does not parse leaves the RDML data as it is, with a note.
pub(crate) fn apply(z: &ZipIndex, d: &mut QpcrData) -> Result<bool> {
    let (Some(app), Some(calc)) = (
        z.read_text("app_data.xml")?,
        z.read_text("calculated_data.xml")?,
    ) else {
        return Ok(false);
    };
    let (Ok(app), Ok(calc)) = (
        parse(&app, crate::rdml::FORMAT_ID, "app_data.xml"),
        parse(&calc, crate::rdml::FORMAT_ID, "calculated_data.xml"),
    ) else {
        d.notes
            .push("LightCycler 96 members app_data.xml / calculated_data.xml do not parse: the vendor analysis is not read".into());
        return Ok(false);
    };
    let (app, calc) = (app.root_element(), calc.root_element());
    if app.tag_name().name() != "rocheLC96AppExtension"
        || calc.tag_name().name() != "rocheLC96CalculatedData"
    {
        return Ok(false);
    }
    let software = app.attribute("softwareVersion").map(str::to_string);
    // graph id → reaction × target
    let mut graphs: BTreeMap<String, Graph> = BTreeMap::new();
    for e in children(app, "experiment") {
        for r in children(e, "run") {
            let run = r.attribute("id").unwrap_or_default().to_string();
            for rx in children(r, "react") {
                let react = rx.attribute("id").unwrap_or_default().to_string();
                for g in children(rx, "factGraph") {
                    let Some(id) = g.attribute("id") else {
                        continue;
                    };
                    graphs.insert(
                        id.to_string(),
                        Graph {
                            run: run.clone(),
                            react: react.clone(),
                            target: g.attribute("targetId").map(str::to_string),
                            excluded: g.attribute("isExcluded") == Some("true"),
                            ..Graph::default()
                        },
                    );
                }
            }
        }
    }
    // calls per graph
    for e in children(calc, "experiment") {
        for r in children(e, "run") {
            for rx in children(r, "react") {
                for g in children(rx, "factGraph") {
                    if let (Some(id), Some(c)) = (g.attribute("id"), text(g, "call"))
                        && let Some(gr) = graphs.get_mut(id)
                    {
                        gr.call = Some(c);
                    }
                }
            }
        }
    }
    // the analysis' Cq per graph (none when the software shows none) and replicate groups
    for s in descendants(calc, DATA_SOURCES) {
        if let Some(id) = text(s, "graphId")
            && let Some(gr) = graphs.get_mut(&id)
        {
            gr.analysed = true;
            if let Some(cq) = text(s, "cq").as_deref().and_then(num) {
                gr.cq = Some(cq);
            }
        }
    }
    let mut groups: Vec<(Vec<String>, Option<f64>, Option<f64>)> = Vec::new();
    for row in descendants(calc, STATISTICAL_ROWS) {
        let ids: Vec<String> = child(row, "graphIds")
            .map(|g| children(g, "guid").filter_map(node_text).collect())
            .unwrap_or_default();
        let mean = text(row, "cqMean").as_deref().and_then(num);
        let err = text(row, "cqError").as_deref().and_then(num);
        if !ids.is_empty() {
            groups.push((ids, mean, err));
        }
    }
    // apply
    let mut matched = 0usize;
    let mut cq_differs = 0usize;
    let mut unmatched = 0usize;
    let mut not_positive = 0usize;
    // of those, how many an analysis of the software lists without a Cq
    let mut analysed_without_cq = 0usize;
    // graph id → (run index, reaction index, assay index)
    let mut located: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for (id, g) in &graphs {
        let Some(ri) = d.runs.iter().position(|r| r.name == g.run) else {
            unmatched += 1;
            continue;
        };
        let run = &d.runs[ri];
        let Some((row, col)) = crate::rdml::react_position(&g.react, run.rows, run.columns) else {
            unmatched += 1;
            continue;
        };
        let Some(xi) = run
            .reactions
            .iter()
            .position(|x| x.row == row && x.column == col)
        else {
            // a well the RDML file has no data for (no target): nothing to attach
            continue;
        };
        let Some(ai) = run.reactions[xi]
            .assays
            .iter()
            .position(|a| a.target == g.target)
        else {
            continue;
        };
        located.insert(id.clone(), (ri, xi, ai));
    }
    for (id, &(ri, xi, ai)) in &located {
        let g = &graphs[id];
        let a = &mut d.runs[ri].reactions[xi].assays[ai];
        matched += 1;
        if let Some(c) = &g.call {
            a.amp_status = Some(status(c));
            // The RDML `cq` of a graph the software does not call positive is not a Cq: the
            // software's analyses list none for such graphs (rdml-lc96-bactxy: 8 of 8, stored
            // 29.57 to 100), and the curves barely rise (docs/provenance/qpcr.md). A Cq an
            // analysis does list is kept.
            if c.trim() != "Positive" && a.cq.is_some() && !(g.analysed && g.cq.is_some()) {
                if g.analysed {
                    analysed_without_cq += 1;
                }
                a.cq_stored = a.cq.take();
                if c.trim() == "Negative" {
                    a.cq_undetermined = true;
                    a.flags.push("lc96_negative_call".into());
                } else {
                    a.flags.push("lc96_call_not_positive".into());
                }
                not_positive += 1;
            }
        }
        if g.excluded && a.excluded.is_none() {
            a.excluded = Some("excluded in the LightCycler 96 software".into());
        }
        if let (Some(v), Some(r)) = (g.cq, a.cq)
            && (v - r).abs() > 0.005
        {
            cq_differs += 1;
        }
    }
    // replicate groups: one sample and one target each, or they are not attached
    let mut group_count = 0usize;
    let mut mixed = 0usize;
    for (ids, mean, err) in &groups {
        let members: Vec<(usize, usize, usize)> =
            ids.iter().filter_map(|i| located.get(i).copied()).collect();
        if members.len() != ids.len() {
            mixed += 1;
            continue;
        }
        let key = |&(ri, xi, ai): &(usize, usize, usize)| {
            let rx = &d.runs[ri].reactions[xi];
            (ri, rx.sample.clone(), rx.assays[ai].target.clone())
        };
        let first = key(&members[0]);
        if members.iter().any(|m| key(m) != first) {
            mixed += 1;
            continue;
        }
        if mean.is_some() {
            group_count += 1;
        }
        for &(ri, xi, ai) in &members {
            let a = &mut d.runs[ri].reactions[xi].assays[ai];
            a.cq_mean = *mean;
            a.cq_sd = *err;
        }
    }
    if d.instrument.manufacturer.is_none() {
        d.instrument.manufacturer = Some("Roche".into());
    }
    if d.instrument.model.is_none() {
        d.instrument.model = Some("LightCycler 96".into());
    }
    if d.instrument.software.is_none() {
        d.instrument.software = Some("LightCycler 96 software".into());
    }
    if d.instrument.software_version.is_none() {
        d.instrument.software_version.clone_from(&software);
    }
    d.notes.push(format!(
        "LightCycler 96 analysis (calculated_data.xml) read: calls of {matched} reactions, Cq mean and SD of {group_count} replicate groups"
    ));
    if not_positive > 0 {
        d.notes.push(format!(
            "{not_positive} reactions the LightCycler 96 software calls Negative or Invalid have an RDML cq, which is not a Cq (the software shows none{}): reported without a Cq (negative: undetermined), the stored number kept as cq_stored",
            if analysed_without_cq > 0 {
                format!("; its analysis lists {analysed_without_cq} of them, without a Cq")
            } else {
                String::new()
            }
        ));
    }
    if cq_differs > 0 {
        d.notes.push(format!(
            "{cq_differs} LightCycler 96 analysis Cqs differ from the RDML cq by more than 0.005: the RDML value is reported"
        ));
    }
    if mixed > 0 {
        d.notes.push(format!(
            "{mixed} LightCycler 96 replicate groups span several samples or targets, or wells without data: their Cq mean and SD are not attached"
        ));
    }
    if unmatched > 0 {
        d.notes.push(format!(
            "{unmatched} LightCycler 96 graphs name a run or reaction the RDML data does not have"
        ));
    }
    if let Value::Object(m) = &mut d.vendor {
        m.insert(
            "lightcycler96".into(),
            json!({
                "software_version": software,
                "graphs": graphs.len(),
                "calls": graphs.values().filter(|g| g.call.is_some()).count(),
                "replicate_groups": groups.len(),
            }),
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RDML: &str = r#"<rdml version="1.1" xmlns="http://www.rdml.org">
      <sample id="1c76448f-03d2-453f-8628-d764e1ca1d55"><description>DIP_1</description><type>unkn</type></sample>
      <target id="SYBR Green I@G"><type>ref</type><dyeId id="SYBR Green I"/></target>
      <experiment id="e"><run id="r"><pcrFormat><rows>8</rows><columns>12</columns><rowLabel>ABC</rowLabel><columnLabel>123</columnLabel></pcrFormat>
        <react id="1"><sample id="1c76448f-03d2-453f-8628-d764e1ca1d55"/><data><tar id="SYBR Green I@G"/><cq>17.69</cq></data></react>
        <react id="2"><sample id="1c76448f-03d2-453f-8628-d764e1ca1d55"/><data><tar id="SYBR Green I@G"/><cq>100</cq></data></react>
      </run></experiment></rdml>"#;
    const APP: &str = r#"<rocheLC96AppExtension softwareVersion="1.1.0.1320" xmlns="http://www.roche.ch/LC96AppExtensionSchema">
      <experiment id="e"><run id="r">
        <react id="1"><factGraph id="g1" targetId="SYBR Green I@G" isExcluded="false"/></react>
        <react id="2"><factGraph id="g2" targetId="SYBR Green I@G" isExcluded="true"/></react>
      </run></experiment></rocheLC96AppExtension>"#;
    const CALC: &str = r#"<rocheLC96CalculatedData xmlns="http://www.roche.ch/LC96CalculatedDataSchema">
      <experiment id="e"><run id="r">
        <react id="1"><factGraph id="g1" targetId="SYBR Green I@G"><call>Positive</call></factGraph></react>
        <react id="2"><factGraph id="g2" targetId="SYBR Green I@G"><call>Negative</call></factGraph></react>
        <moduleCalculatedData><module id="Analysis"><relQuantDataModel>
          <dataSource><relQuantDataSource><graphId>g1</graphId><cq>17.69</cq></relQuantDataSource></dataSource>
          <statisticalDataSource><relQuantStatisticalRow><graphIds><guid>g1</guid><guid>g2</guid></graphIds>
            <cqError>0.0777</cqError><cqMean>17.745</cqMean></relQuantStatisticalRow></statisticalDataSource>
        </relQuantDataModel></module></moduleCalculatedData>
      </run></experiment></rocheLC96CalculatedData>"#;

    #[test]
    fn lc96_members_attach_vendor_results() {
        let bytes = openreadout_core::zip::zip_bytes(&[
            ("rdml_data.xml", RDML.as_bytes()),
            ("app_data.xml", APP.as_bytes()),
            ("calculated_data.xml", CALC.as_bytes()),
        ])
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.lc96p");
        std::fs::write(&p, bytes).unwrap();
        let input = openreadout_core::source::Input::local(&p);
        let z = ZipIndex::open(input.fs(), input.path(), "rdml").unwrap();
        let mut d = crate::rdml::parse_rdml(RDML).unwrap();
        assert_eq!(d.samples[0].name, "DIP_1");
        assert_eq!(
            d.samples[0].id.as_deref(),
            Some("1c76448f-03d2-453f-8628-d764e1ca1d55")
        );
        assert!(apply(&z, &mut d).unwrap());
        let rx = &d.runs[0].reactions;
        assert_eq!(rx[0].sample.as_deref(), Some("DIP_1"));
        assert_eq!(rx[0].assays[0].amp_status.as_deref(), Some("amplified"));
        assert_eq!(rx[1].assays[0].amp_status.as_deref(), Some("not amplified"));
        assert!(rx[1].assays[0].excluded.is_some());
        assert_eq!(rx[1].assays[0].cq, None);
        assert!(rx[1].assays[0].cq_undetermined);
        assert_eq!(rx[1].assays[0].cq_stored, Some(100.0));
        assert_eq!(rx[0].assays[0].cq, Some(17.69));
        assert_eq!(rx[0].assays[0].cq_mean, Some(17.745));
        assert_eq!(rx[1].assays[0].cq_sd, Some(0.0777));
        assert_eq!(d.instrument.model.as_deref(), Some("LightCycler 96"));
    }

    /// Absolute quantification (as in `rdml-lc96-bactxy`): a Negative graph the analysis lists
    /// without a Cq loses its plausible stored number; one it lists with a Cq keeps it.
    #[test]
    fn abs_quant_analysis_decides_non_positive_calls() {
        let rdml = RDML.replace("<cq>17.69</cq>", "<cq>29.57</cq>");
        let calc = r#"<rocheLC96CalculatedData xmlns="http://www.roche.ch/LC96CalculatedDataSchema">
          <experiment id="e"><run id="r">
            <react id="1"><factGraph id="g1" targetId="SYBR Green I@G"><call>Negative</call></factGraph></react>
            <react id="2"><factGraph id="g2" targetId="SYBR Green I@G"><call>Invalid</call></factGraph></react>
            <moduleCalculatedData><module id="Analysis"><absQuantDataModel><dataSource>
              <absQuantDataSource><graphId>g1</graphId><call>Negative</call><editedCall>false</editedCall></absQuantDataSource>
              <absQuantDataSource><graphId>g2</graphId><call>Invalid</call><cq>100</cq></absQuantDataSource>
            </dataSource></absQuantDataModel></module></moduleCalculatedData>
          </run></experiment></rocheLC96CalculatedData>"#;
        let bytes = openreadout_core::zip::zip_bytes(&[
            ("rdml_data.xml", rdml.as_bytes()),
            ("app_data.xml", APP.as_bytes()),
            ("calculated_data.xml", calc.as_bytes()),
        ])
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.rdml");
        std::fs::write(&p, bytes).unwrap();
        let input = openreadout_core::source::Input::local(&p);
        let z = ZipIndex::open(input.fs(), input.path(), "rdml").unwrap();
        let mut d = crate::rdml::parse_rdml(&rdml).unwrap();
        assert!(apply(&z, &mut d).unwrap());
        let rx = &d.runs[0].reactions;
        assert_eq!(rx[0].assays[0].cq, None);
        assert_eq!(rx[0].assays[0].cq_stored, Some(29.57));
        assert!(rx[0].assays[0].cq_undetermined);
        assert_eq!(rx[1].assays[0].cq, Some(100.0));
        assert!(
            d.notes
                .iter()
                .any(|n| n.contains("its analysis lists 1 of them, without a Cq"))
        );
    }

    #[test]
    fn guid_detection() {
        assert!(crate::rdml::is_guid("1c76448f-03d2-453f-8628-d764e1ca1d55"));
        assert!(!crate::rdml::is_guid("Sample 1"));
        assert!(!crate::rdml::is_guid("1c76448f-03d2-453f-8628-d764e1ca1d5"));
    }
}

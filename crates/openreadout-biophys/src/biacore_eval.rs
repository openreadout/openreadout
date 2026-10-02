//! The evaluation items of a Biacore T200 evaluation file (`.bme`): XML streams
//! `Evaluation/EvaluationItemN` (ISO-8859-1). Their fits — the evaluation software's kinetic and
//! affinity models with parameters, standard errors and Chi², and the curves each was fitted to —
//! are read as stored. Layout: `docs/formats/cytiva-biacore.md` § Evaluation files.

use std::collections::BTreeMap;

use openreadout_core::bytes::{latin1, until_nul};
use roxmltree::{Document, Node};

/// One evaluation item (a report-point table, plot, sensorgram overlay or analysis).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EvalItem {
    /// `N` of `EvaluationItemN`.
    pub index: u32,
    /// The item's `ClassName` (`AffinityScreen`, `KineticsAffinity`, `Plot`, …).
    pub class: String,
    /// The item's `Name`.
    pub name: String,
    /// Fits the item holds.
    pub fits: usize,
}

/// One parameter of a fit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Param {
    /// What the value applies to (`0`, `0:1-32`): the text before the name.
    pub scope: String,
    /// Parameter name as the model names it (`KD`, `ka`, `kd`, `Rmax`, `offset`).
    pub name: String,
    pub value: f64,
    /// Standard error.
    pub se: f64,
}

/// One curve (concentration) of a fit's curve set.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FitPoint {
    /// The subset's curve name (`Fc=2-1`).
    pub subset: String,
    pub file: f64,
    pub cycle: f64,
    pub sample: String,
    /// Molar concentration (M).
    pub concentration: f64,
    /// The concentration as entered, in `unit`.
    pub concentration_stated: f64,
    pub unit: String,
    /// The response the fit used (RU), when stored.
    pub response: f64,
    /// 1 included in the fit, 0 excluded, NaN when not stored.
    pub included: f64,
}

/// One fit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fit {
    /// `N` of the item.
    pub item: u32,
    pub item_name: String,
    /// `ModelName` (`Steady State Affinity`, `1:1 Binding`).
    pub model: String,
    /// The model's expression or type (`Conc*Rmax/(Conc+KD)+offset`).
    pub expression: String,
    pub chi2: f64,
    pub sample: String,
    pub ligand: String,
    /// °C.
    pub temperature: f64,
    /// Curve names of the subsets (`Fc=2-1`), `;`-separated.
    pub curves: String,
    /// `fitStatus` where stored.
    pub status: String,
    pub params: Vec<Param>,
    /// Unit of each parameter, from `ReportParameters` (`KD (M)|KD`).
    pub units: BTreeMap<String, String>,
    pub points: Vec<FitPoint>,
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.has_tag_name(name))
}

fn text_of(n: Node<'_, '_>, name: &str) -> String {
    child(n, name)
        .and_then(|c| c.text())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn num(s: &str) -> f64 {
    s.trim().parse::<f64>().unwrap_or(f64::NAN)
}

/// `id:name|value|error;…` (the id may itself contain `:`).
pub(crate) fn parse_parameters(s: &str) -> Vec<Param> {
    s.split(';')
        .filter_map(|e| {
            let (head, rest) = e.split_once('|')?;
            let (scope, name) = match head.rsplit_once(':') {
                Some((a, b)) => (a.trim(), b.trim()),
                None => ("", head.trim()),
            };
            if name.is_empty() {
                return None;
            }
            let mut it = rest.split('|');
            let value = num(it.next().unwrap_or(""));
            let se = num(it.next().unwrap_or(""));
            Some(Param {
                scope: scope.to_string(),
                name: name.to_string(),
                value,
                se,
            })
        })
        .collect()
}

/// `KD (M)|KD;Rmax (RU)|Rmax` -> {KD: M, Rmax: RU}.
pub(crate) fn parse_units(s: &str) -> BTreeMap<String, String> {
    s.split(';')
        .filter_map(|e| {
            let (label, name) = e.split_once('|')?;
            let unit = label.rsplit_once('(')?.1.strip_suffix(')')?.trim();
            (!unit.is_empty()).then(|| (name.trim().to_string(), unit.to_string()))
        })
        .collect()
}

/// The curves of a `CurveSet` element and its subset names.
fn curve_set(cs: Node<'_, '_>) -> (Vec<FitPoint>, Vec<String>) {
    let mut points = Vec::new();
    let mut names = Vec::new();
    for sub in cs
        .children()
        .filter(|c| c.is_element() && c.tag_name().name().starts_with("Subset"))
    {
        let subset = text_of(sub, "CurveName");
        names.push(subset.clone());
        for cv in sub.children().filter(|c| {
            c.is_element()
                && c.tag_name()
                    .name()
                    .strip_prefix("Curve")
                    .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        }) {
            let inj = child(cv, "Injections").and_then(|i| child(i, "Injection"));
            let (conc, stated, response) = match inj {
                Some(i) => (
                    num(&text_of(i, "MolarConcentration")),
                    num(&text_of(i, "Concentration")),
                    num(&text_of(i, "Response")),
                ),
                None => (f64::NAN, f64::NAN, f64::NAN),
            };
            let included = match text_of(cv, "Included").as_str() {
                "true" => 1.0,
                "false" => 0.0,
                _ => f64::NAN,
            };
            points.push(FitPoint {
                subset: subset.clone(),
                file: num(&text_of(cv, "FileIndex")),
                cycle: num(&text_of(cv, "CycleNumber")),
                sample: text_of(cv, "SampleName"),
                concentration: conc,
                concentration_stated: stated,
                unit: text_of(cv, "ConcUnit"),
                response,
                included,
            });
        }
    }
    (points, names)
}

/// A fit from its model element (`model` or `FitN`), curve set and key.
fn fit_of(
    item: &EvalItem,
    model: Node<'_, '_>,
    cs: Option<Node<'_, '_>>,
    key: Option<Node<'_, '_>>,
    status: String,
) -> Fit {
    let (points, names) = cs.map(curve_set).unwrap_or_default();
    let pick = |cs_name: &str, key_name: &str| -> String {
        let a = cs.map(|c| text_of(c, cs_name)).unwrap_or_default();
        if a.is_empty() {
            key.map(|k| text_of(k, key_name)).unwrap_or_default()
        } else {
            a
        }
    };
    let curves = if names.iter().any(|n| !n.is_empty()) {
        names.join(";")
    } else {
        key.map(|k| text_of(k, "curveName")).unwrap_or_default()
    };
    Fit {
        item: item.index,
        item_name: item.name.clone(),
        model: model.attribute("ModelName").unwrap_or("").to_string(),
        expression: text_of(model, "Model"),
        chi2: num(&text_of(model, "Chi2")),
        sample: pick("SampleName", "sampleName"),
        ligand: pick("LigandName", "ligandName"),
        temperature: num(&pick("Temperature", "temperature")),
        curves,
        status,
        params: parse_parameters(&text_of(model, "Parameters")),
        units: parse_units(&text_of(model, "ReportParameters")),
        points,
    }
}

/// Parse one item stream (ISO-8859-1 XML).
pub(crate) fn parse_item(index: u32, bytes: &[u8]) -> Result<(EvalItem, Vec<Fit>), String> {
    let text = latin1(until_nul(bytes));
    let doc = Document::parse(&text).map_err(|e| format!("EvaluationItem{index}: {e}"))?;
    let root = doc.root_element();
    let mut item = EvalItem {
        index,
        class: root.attribute("ClassName").unwrap_or("").to_string(),
        name: text_of(root, "Name"),
        fits: 0,
    };
    let mut fits = Vec::new();
    // the older layout: modelFits/modelFits/modelFit, each with its own curve set and key
    for mf in root
        .descendants()
        .filter(|n| n.is_element() && n.has_tag_name("modelFit"))
    {
        let Some(model) = child(mf, "model") else {
            continue;
        };
        let cs = child(mf, "curveSet").and_then(|c| child(c, "CurveSet"));
        fits.push(fit_of(
            &item,
            model,
            cs,
            child(mf, "key"),
            text_of(mf, "fitStatus"),
        ));
    }
    // the newer layout: Fits/FitN beside the item's CurveSet
    if let Some(fs) = child(root, "Fits") {
        let cs = child(root, "CurveSet");
        for f in fs.children().filter(|c| {
            c.is_element()
                && c.tag_name()
                    .name()
                    .strip_prefix("Fit")
                    .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        }) {
            fits.push(fit_of(&item, f, cs, None, String::new()));
        }
    }
    item.fits = fits.len();
    Ok((item, fits))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // exact values
    use super::*;

    #[test]
    fn parameters_and_units() {
        let p = parse_parameters("0:KD|6.5E-05|4.3E-06;0:1-32:ka|3396690.9|1964.4;0:offset|0|");
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].scope.as_str(), p[0].name.as_str()), ("0", "KD"));
        assert_eq!(p[0].value, 6.5e-5);
        assert_eq!((p[1].scope.as_str(), p[1].name.as_str()), ("0:1-32", "ka"));
        assert!(p[2].se.is_nan());
        let u = parse_units("KD (M)|KD;Rmax (RU)|Rmax;offset (RU)|offset;x|y");
        assert_eq!(u.get("KD").map(String::as_str), Some("M"));
        assert_eq!(u.len(), 3);
    }

    #[test]
    fn both_layouts() {
        let old = "<EvaluationItem7 ClassName=\"AffinityScreen\"><Name>Affinity Screen</Name><modelFits><modelFits><modelFit><curveSet><CurveSet><SampleName>C1</SampleName><Temperature>20</Temperature><Subset0><CurveName>Fc=2-1</CurveName><Curve0><FileIndex>1</FileIndex><CycleNumber>14</CycleNumber><SampleName>C1</SampleName><ConcUnit>\u{b5}M</ConcUnit><Injections><Injection><Concentration>1</Concentration><MolarConcentration>1E-06</MolarConcentration><Response>2.5</Response></Injection></Injections><Included>true</Included></Curve0></Subset0></CurveSet></curveSet><model ModelName=\"Steady State Affinity\"><Model>Conc*Rmax/(Conc+KD)+offset</Model><Chi2>0.11</Chi2><Parameters>0:KD|6.5E-05|4.3E-06</Parameters><ReportParameters>KD (M)|KD</ReportParameters></model><fitStatus>Cleared</fitStatus></modelFit></modelFits></modelFits></EvaluationItem7>";
        let bytes: Vec<u8> = old.chars().map(|c| c as u8).collect();
        let (item, fits) = parse_item(3, &bytes).unwrap();
        assert_eq!(item.class, "AffinityScreen");
        assert_eq!(fits.len(), 1);
        let f = &fits[0];
        assert_eq!(f.sample, "C1");
        assert_eq!(f.curves, "Fc=2-1");
        assert_eq!(f.points[0].unit, "\u{b5}M");
        assert_eq!(f.points[0].included, 1.0);
        assert_eq!(f.points[0].concentration, 1e-6);
        assert_eq!(f.status, "Cleared");
        let new = "<E ClassName=\"KineticsAffinity\"><Name>K</Name><CurveSet><SampleName>S</SampleName><LigandName>L</LigandName><Subset0><CurveName>Fc=2-1</CurveName></Subset0></CurveSet><Fits><Fit0 ModelName=\"1:1 Binding\"><Chi2>1.5</Chi2><Parameters>0:1-32:ka|3E6|2E3</Parameters></Fit0></Fits></E>";
        let (_, fits) = parse_item(1, new.as_bytes()).unwrap();
        assert_eq!(fits[0].model, "1:1 Binding");
        assert_eq!(fits[0].ligand, "L");
        assert_eq!(fits[0].params[0].name, "ka");
        assert!(parse_item(0, b"<a>").is_err());
    }
}

//! The LC pump program in an instrument method's per-device text (`docs/formats/thermo-raw.md`
//! § Gradient table): solvent names and one step per row, flow in µL/min.
//!
//! Three layouts occur in the corpus: the Accela pumps' `Pump 1 gradient table:`, the
//! EASY-nLC's `Gradient:` block and the Vanquish instrument script. Anything else gives `None`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// One gradient step: time from injection, flow and composition as written.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientStep {
    /// Minutes from the start of the run.
    pub time_min: f64,
    /// Flow in µL/min (converted from the unit the text uses).
    pub flow_ul_min: Option<f64>,
    /// Percent of each solvent channel (`A`…`D`) the text lists for this step.
    pub percent: BTreeMap<String, f64>,
}

/// The pump program of one device.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Gradient {
    /// Device storage the text came from (`LegacyDualPump`, `Proxeon_EASY-nLC`, `SiiXcalibur`).
    pub device: String,
    /// Solvent channel → name, when the method names it.
    pub solvents: BTreeMap<String, String>,
    pub steps: Vec<GradientStep>,
}

impl Gradient {
    /// `{"device", "solvents", "steps": [{"time_min", "flow_ul_min", "percent": {"B": 5}}]}`.
    pub fn to_json(&self) -> Value {
        json!({
            "device": self.device,
            "solvents": self.solvents,
            "steps": self.steps.iter().map(|s| {
                let mut o = serde_json::Map::new();
                o.insert("time_min".into(), json!(s.time_min));
                if let Some(f) = s.flow_ul_min {
                    o.insert("flow_ul_min".into(), json!(f));
                }
                o.insert("percent".into(), json!(s.percent));
                Value::Object(o)
            }).collect::<Vec<_>>(),
        })
    }
}

/// The first device text that holds a pump program.
pub fn from_device_texts(texts: &[(String, String)]) -> Option<Gradient> {
    texts.iter().find_map(|(device, t)| {
        let mut g = accela(t).or_else(|| easy_nlc(t)).or_else(|| vanquish(t))?;
        g.device.clone_from(device);
        Some(g)
    })
}

/// µL/min per unit of a flow unit as written (`µl/min`, `nl/min`, `ml/min`).
fn flow_factor(unit: &str) -> Option<f64> {
    let u = unit
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase()
        .replace(['\u{b5}', '\u{3bc}'], "u");
    match u.as_str() {
        "ul/min" | "ul / min" => Some(1.0),
        "nl/min" | "nl / min" => Some(0.001),
        "ml/min" | "ml / min" => Some(1000.0),
        _ => None,
    }
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// Accela pump text: `Solvent A: …` lines and `Pump 1 gradient table:` with a
/// `No. Time A% B% C% D% µl/min` header.
fn accela(text: &str) -> Option<Gradient> {
    let mut g = Gradient::default();
    for l in text.lines() {
        if let Some(rest) = l.trim().strip_prefix("Solvent ")
            && let Some((ch, name)) = rest.split_once(':')
            && ch.len() == 1
            && !name.trim().is_empty()
        {
            g.solvents.insert(ch.to_string(), name.trim().to_string());
        }
    }
    let mut lines = text
        .lines()
        .skip_while(|l| !l.trim_end().ends_with("gradient table:"));
    lines.next()?;
    let header: Vec<&str> = lines
        .by_ref()
        .find(|l| !l.trim().is_empty())?
        .split_whitespace()
        .collect();
    if header.first() != Some(&"No.") || header.get(1) != Some(&"Time") {
        return None;
    }
    let factor = header.last().and_then(|u| flow_factor(u));
    let channels: Vec<(usize, String)> = header
        .iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let c = h.strip_suffix('%')?;
            (c.len() == 1).then(|| (i, c.to_string()))
        })
        .collect();
    for l in lines {
        let cells: Vec<&str> = l.split_whitespace().collect();
        if cells.is_empty() {
            if g.steps.is_empty() {
                continue;
            }
            break;
        }
        if cells.len() != header.len() || cells[0].parse::<u32>().is_err() {
            break;
        }
        let time_min = cells[1].parse::<f64>().ok()?;
        let percent = channels
            .iter()
            .filter_map(|(i, c)| Some((c.clone(), cells.get(*i)?.parse::<f64>().ok()?)))
            .collect();
        let flow_ul_min = factor.and_then(|f| {
            cells
                .last()
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| round6(v * f))
        });
        g.steps.push(GradientStep {
            time_min,
            flow_ul_min,
            percent,
        });
    }
    (!g.steps.is_empty()).then_some(g)
}

/// `mm:ss` → minutes.
fn mm_ss(s: &str) -> Option<f64> {
    let (m, sec) = s.split_once(':')?;
    Some(m.parse::<f64>().ok()? + sec.parse::<f64>().ok()? / 60.0)
}

/// EASY-nLC text: `Gradient:`, a `Time [mm:ss] Duration [mm:ss] Flow [nl/min] Mixture [%B]`
/// header, one row per step.
fn easy_nlc(text: &str) -> Option<Gradient> {
    let mut lines = text.lines().skip_while(|l| l.trim() != "Gradient:");
    lines.next()?;
    let header = lines.next()?;
    if !(header.contains("Time [mm:ss]") && header.contains("Mixture [%B]")) {
        return None;
    }
    let factor = header
        .split('[')
        .filter_map(|p| p.split_once(']').map(|(u, _)| u))
        .find_map(flow_factor);
    let mut g = Gradient::default();
    for l in lines {
        let cells: Vec<&str> = l.split_whitespace().collect();
        if cells.len() != 4 {
            break;
        }
        let (Some(time_min), Some(flow), Some(b)) = (
            mm_ss(cells[0]),
            cells[2].parse::<f64>().ok(),
            cells[3].parse::<f64>().ok(),
        ) else {
            break;
        };
        g.steps.push(GradientStep {
            time_min: round6(time_min),
            flow_ul_min: factor.map(|f| round6(flow * f)),
            percent: BTreeMap::from([("B".to_string(), b)]),
        });
    }
    (!g.steps.is_empty()).then_some(g)
}

/// `0.500 [ml/min]` → (0.5, `ml/min`).
fn value_unit(s: &str) -> Option<(f64, &str)> {
    let (v, rest) = s.trim().split_once(' ')?;
    let unit = rest.trim().strip_prefix('[')?.strip_suffix(']')?;
    Some((v.parse().ok()?, unit))
}

/// Vanquish instrument script: `PumpModule.Pump.%A1_Equate: "…"` solvent names selected by
/// `%A_Selector`, then `<t> [min]` blocks with `PumpModule.Pump.Flow.Nominal` and
/// `PumpModule.Pump.%B.Value`.
fn vanquish(text: &str) -> Option<Gradient> {
    const PUMP: &str = "PumpModule.Pump.";
    let mut g = Gradient::default();
    let mut equate: BTreeMap<String, String> = BTreeMap::new();
    let mut selected: BTreeMap<String, String> = BTreeMap::new();
    let mut current: Option<GradientStep> = None;
    let mut in_script = false;
    for l in text.lines() {
        if !l.starts_with(' ') && !l.starts_with('\t') {
            if let Some(s) = current.take()
                && (s.flow_ul_min.is_some() || !s.percent.is_empty())
            {
                g.steps.push(s);
            }
            if let Some((t, _)) = l.split_once(" [min]")
                && let Ok(t) = t.trim().parse::<f64>()
            {
                in_script = true;
                current = Some(GradientStep {
                    time_min: t,
                    flow_ul_min: None,
                    percent: BTreeMap::new(),
                });
            }
            continue;
        }
        let Some(rest) = l.trim().strip_prefix(PUMP) else {
            continue;
        };
        let Some((key, val)) = rest.split_once(':') else {
            continue;
        };
        let val = val.trim();
        if let Some(ch) = key
            .strip_prefix('%')
            .and_then(|k| k.strip_suffix("_Equate"))
        {
            equate.insert(ch.to_string(), val.trim_matches('"').to_string());
        } else if let Some(ch) = key
            .strip_prefix('%')
            .and_then(|k| k.strip_suffix("_Selector"))
        {
            selected.insert(ch.to_string(), val.trim_start_matches('%').to_string());
        } else if let Some(step) = current.as_mut().filter(|_| in_script) {
            if key == "Flow.Nominal" {
                step.flow_ul_min =
                    value_unit(val).and_then(|(v, u)| flow_factor(u).map(|f| round6(v * f)));
            } else if let Some(ch) = key
                .strip_prefix('%')
                .and_then(|k| k.strip_suffix(".Value"))
                .filter(|c| c.len() == 1)
                && let Some((v, "%")) = value_unit(val)
            {
                step.percent.insert(ch.to_string(), v);
            }
        }
    }
    if let Some(s) = current.take()
        && (s.flow_ul_min.is_some() || !s.percent.is_empty())
    {
        g.steps.push(s);
    }
    for (ch, which) in selected {
        if let Some(name) = equate.get(&which).filter(|n| !n.is_empty()) {
            g.solvents.insert(ch, name.clone());
        }
    }
    (!g.steps.is_empty()).then_some(g)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const ACCELA: &str = "Instrument:                   Accela 1250 Pump\n\nPump 1 settings:\n\nName:                         Pump 1\nSolvent A:                    \nSolvent C:                    W 0.1% FA\nSolvent D:                    ACN 0.1% FA\n\nPump 1 gradient table:\n\nNo. Time  A%    B%    C%    D%    \u{b5}l/min     \n0   0.00  0.0   0.0   99.0  1.0   500.0      \n1   9.00  0.0   0.0   5.0   95.0  500.0      \n\n\n";

    #[test]
    fn accela_table() {
        let g = accela(ACCELA).unwrap();
        assert_eq!(g.solvents.len(), 2);
        assert_eq!(g.solvents["D"], "ACN 0.1% FA");
        assert_eq!(g.steps.len(), 2);
        assert_eq!(g.steps[1].time_min, 9.0);
        assert_eq!(g.steps[1].flow_ul_min, Some(500.0));
        assert_eq!(g.steps[1].percent["D"], 95.0);
        assert_eq!(g.steps[1].percent.len(), 4);
    }

    #[test]
    fn nlc_gradient() {
        let t = "Gradient:\n   Time [mm:ss]   Duration [mm:ss]   Flow [nl/min]   Mixture [%B]\n         00:00              00:00             300              5\n         40:30              40:30             300             40\n\nPre-column equilibration:\n";
        let g = easy_nlc(t).unwrap();
        assert_eq!(g.steps.len(), 2);
        assert_eq!(g.steps[1].time_min, 40.5);
        assert_eq!(g.steps[1].flow_ul_min, Some(0.3));
        assert_eq!(g.steps[1].percent["B"], 40.0);
    }

    #[test]
    fn vanquish_script() {
        let t = "---- Script ----\ninitial     Instrument Setup\n            PumpModule.Pump.%B_Selector: %B1\n            PumpModule.Pump.%A_Selector: %A1\n            PumpModule.Pump.%A1_Equate: \"MQ + 0.1% formic acid\"\n            PumpModule.Pump.%B1_Equate: \"ACN + 0.1% formic acid\"\n            PumpModule.Pump.%B2_Equate: \"other\"\n0.000 [min] Inject\n            SamplerModule.Sampler.Inject \n0.000 [min] Run\n            PumpModule.Pump.Flow.Nominal: 0.500 [ml/min]\n            PumpModule.Pump.%B.Value: 1.0 [%]\n3.000 [min]\n            PumpModule.Pump.Flow.Nominal: 0.500 [ml/min]\n            PumpModule.Pump.%B.Value: 15.0 [%]\n12.000 [min] Stop Run\n";
        let g = vanquish(t).unwrap();
        assert_eq!(g.solvents["A"], "MQ + 0.1% formic acid");
        assert_eq!(g.solvents["B"], "ACN + 0.1% formic acid");
        assert_eq!(g.steps.len(), 2);
        assert_eq!(g.steps[1].time_min, 3.0);
        assert_eq!(g.steps[1].flow_ul_min, Some(500.0));
        assert_eq!(g.steps[1].percent["B"], 15.0);
    }

    #[test]
    fn other_text_is_none() {
        assert!(from_device_texts(&[("LTQ".into(), "MS Run Time (min): 19.00".into())]).is_none());
        let g = from_device_texts(&[
            ("LTQ".into(), "nothing".into()),
            ("LegacyDualPump".into(), ACCELA.into()),
        ])
        .unwrap();
        assert_eq!(g.device, "LegacyDualPump");
    }
}

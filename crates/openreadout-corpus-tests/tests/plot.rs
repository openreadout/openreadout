//! Figures of the launch scenario files: the numbers `plot` reports for what it drew agree with
//! the analyses (wells that grew or crashed, group means at the end of the run, the f–I curve,
//! the TIC apex and the spectrum's base peak, gate populations).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test plot`
//! (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; missing files are skipped).
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_plot::{PlotOutput, parse_spec, plot};
use serde_json::{Value, json};

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

fn file(name: &str) -> Option<String> {
    let p = files_dir().join(name);
    if p.exists() {
        Some(p.display().to_string())
    } else {
        eprintln!("skip: {name} not in the corpus directory");
        None
    }
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_plate::PlateReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_fcs::FcsReader))
}

fn run(spec: Value) -> PlotOutput {
    plot(&registry(), &parse_spec(spec).unwrap())
        .unwrap()
        .output
}

#[test]
fn plate_one_wells_that_grew_and_crashed() {
    let Some(f) = file("softmax5-kinetic-phage-120308b.pda") else {
        return;
    };
    let o = run(json!({
        "data": {"file": f, "subtract": "blank",
                 "color_by": {"at": 6, "below": 0.1, "label": "crashed", "else": "grew"}},
        "facet": "plate",
    }));
    let p = &o.panels[0];
    assert_eq!(p.facets, Some(96));
    let count = |c: &str| {
        p.series
            .iter()
            .find(|s| s.class.as_deref() == Some(c))
            .and_then(|s| s.count)
    };
    // 24 λ wells with up to 500k phage and 16 without phage grew, and HK97 H3 escaped; the
    // 12 λ wells with 5M-500M phage were held back and 35 infected HK97 wells crashed
    assert_eq!(count("grew"), Some(41));
    assert_eq!(count("crashed"), Some(47));
    assert_eq!(count("blank"), Some(8));
    assert_eq!(p.x.unit.as_deref(), Some("h"));
    assert!((p.x.range.unwrap()[1] - 18.6).abs() < 0.05);
}

#[test]
fn plate_one_growth_patterns() {
    let Some(f) = file("softmax5-kinetic-phage-120308b.pda") else {
        return;
    };
    let o = run(json!({"file": f, "data": {"subtract": "blank", "color_by": "growth"}, "facet": "plate"}));
    let p = &o.panels[0];
    let wells = |c: &str| {
        p.series
            .iter()
            .find(|s| s.class.as_deref() == Some(c))
            .and_then(|s| s.count)
    };
    // the infected HK97 wells crash and come back, except the three that never grow (E10,
    // F11, G7) and H3 and H10, which escaped; λ A11 never grows, B11-D11 come back late
    assert_eq!(wells("no growth"), Some(4));
    assert_eq!(wells("crashed, then regrew"), Some(34));
    assert_eq!(wells("grew"), Some(50));
    assert_eq!(wells("crashed"), None);
}

#[test]
fn plate_two_group_means() {
    let Some(f) = file("softmax5-kinetic-phage-120524.pda") else {
        return;
    };
    let o = run(json!({
        "data": {"file": f, "by": "group", "subtract": "first",
                 "groups": ["99.9 * +", "95 * +", "10 * +"]},
    }));
    let s = &o.panels[0].series;
    let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["99.9 +", "95 +", "10 +"]);
    assert!(
        s.iter().all(|x| x.data.contains("mean ± SD of 8 wells")),
        "{s:?}"
    );
}

#[test]
fn fi_curve_and_sweeps() {
    let Some(f) = file("pyabf-2019-07-24-0055-fsi.abf") else {
        return;
    };
    let o = run(json!({"data": {"file": f, "show": "fi", "mark": "bars"}}));
    let s = &o.panels[0].series[0];
    assert_eq!(s.y_range.unwrap()[1], 64.0);
    assert_eq!(s.x_range.unwrap()[1], 300.0);
    let o = run(json!({"data": {"file": f, "show": "sweeps", "sweep": [16], "spikes": true}}));
    let s = &o.panels[0].series[0];
    assert!(s.name.starts_with("+300 pA"), "{}", s.name);
    assert!(s.marked.len() > 64, "{}", s.marked.len());
}

#[test]
fn tic_apex_and_the_spectrum_there() {
    let Some(f) = file("mtbls20-caffeine-pos.mzML") else {
        return;
    };
    let o = run(json!({"panels": [
        {"data": {"file": f, "peaks": 1}},
        {"data": {"file": f, "rt": "apex", "peaks": 3}},
    ]}));
    let apex = &o.panels[0].series[0].marked[0];
    assert!((apex.x - 36.8).abs() < 0.5, "{apex:?}");
    let base = &o.panels[1].series[0].marked[0];
    // caffeine [M+H]+
    assert!((base.x - 195.0877).abs() < 0.01, "{base:?}");
}

#[test]
fn gate_populations_in_a_dot_plot() {
    let (Some(f), Some(w)) = (file("flowkit-8c-e01.fcs"), file("flowkit-8c-ics.wsp")) else {
        return;
    };
    let o = run(
        json!({"data": {"file": f, "x": "CD4 PE-Cy7 FLR-A", "y": "CD8 PerCP-Cy55 FLR-A",
                                "workspace": w, "populations": ["CD4+", "CD8+"]}}),
    );
    let s = &o.panels[0].series;
    // FlowJo's gates: 82,484 CD4+ and 47,165 CD8+ events of 290,172
    let cd4 = s.iter().find(|x| x.name.starts_with("CD4+")).unwrap();
    let cd8 = s.iter().find(|x| x.name.starts_with("CD8+")).unwrap();
    assert_eq!(cd4.points, 82_484);
    assert_eq!(cd8.points, 47_165);
}

//! Scales shared across series: FCS files overlaid on one axis, histograms on a log axis, and
//! bars on a log axis.

#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_plot::{parse_spec, plot};
use serde_json::json;

/// An FCS 3.1 file of one float parameter (`FITC-A`, a fluorescence channel) holding `values`.
fn fcs(dir: &Path, name: &str, values: &[f32]) -> PathBuf {
    let data: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let kws = [
        ("$BYTEORD", "1,2,3,4".to_string()),
        ("$DATATYPE", "F".into()),
        ("$MODE", "L".into()),
        ("$PAR", "1".into()),
        ("$TOT", values.len().to_string()),
        ("$NEXTDATA", "0".into()),
        ("$BEGINANALYSIS", "0".into()),
        ("$ENDANALYSIS", "0".into()),
        ("$BEGINSTEXT", "0".into()),
        ("$ENDSTEXT", "0".into()),
        ("$P1N", "FITC-A".into()),
        ("$P1B", "32".into()),
        ("$P1R", "262144".into()),
        ("$P1E", "0,0".into()),
    ];
    let render = |begin: u64, end: u64| {
        let mut t = String::from("/");
        let mut all: Vec<(String, String)> = kws
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        all.push(("$BEGINDATA".into(), format!("{begin:010}")));
        all.push(("$ENDDATA".into(), format!("{end:010}")));
        for (k, v) in all {
            t.push_str(&k);
            t.push('/');
            t.push_str(&v);
            t.push('/');
        }
        t
    };
    let text_len = render(0, 0).len() as u64;
    let text_end = 58 + text_len - 1;
    let data_begin = text_end + 1;
    let data_end = data_begin + data.len() as u64 - 1;
    let mut out = format!(
        "FCS3.1    {:>8}{text_end:>8}{data_begin:>8}{data_end:>8}{:>8}{:>8}",
        58, 0, 0
    )
    .into_bytes();
    out.extend_from_slice(render(data_begin, data_end).as_bytes());
    out.extend_from_slice(&data);
    let path = dir.join(name);
    std::fs::write(&path, out).unwrap();
    path
}

/// `n` values spread evenly (in log) around `centre`.
fn around(centre: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| centre * 10f32.powf((i as f32 / n as f32 - 0.5) * 0.6))
        .collect()
}

#[test]
fn overlaid_fcs_files_share_one_scale() {
    let d = tempfile::tempdir().unwrap();
    // a bright file reaching 10⁵ and a dim one under 10³: alone, each would get its own top
    let bright = fcs(d.path(), "bright.fcs", &around(30_000.0, 2000));
    let dim = fcs(d.path(), "dim.fcs", &around(160.0, 2000));
    let reg = Registry::new().with(Box::new(openreadout_fcs::FcsReader));
    let spec = parse_spec(json!({"data": [
        {"file": bright, "show": "histogram", "x": "FITC-A", "label": "bright"},
        {"file": dim, "show": "histogram", "x": "FITC-A", "label": "dim"},
    ]}))
    .unwrap();
    let p = plot(&reg, &spec).unwrap();
    let s = &p.output.panels[0].series;
    assert_eq!(s.len(), 2);
    assert!(s[0].data.contains("T=100000"), "{}", s[0].data);
    assert_eq!(s[0].data, s[1].data);
    assert!(
        p.output.notes.iter().any(|n| n.contains("for all 2 files")),
        "{:?}",
        p.output.notes
    );
    // the dim file alone keeps its own, smaller scale
    let alone =
        parse_spec(json!({"data": {"file": dim, "show": "histogram", "x": "FITC-A"}})).unwrap();
    let a = plot(&reg, &alone).unwrap();
    assert!(
        a.output.panels[0].series[0].data.contains("T=1000,"),
        "{}",
        a.output.panels[0].series[0].data
    );
    // where the dim peak sits on the shared axis is where it sits on an axis of its own
    // scale, mapped through that scale: the peak of a histogram is the bin with most events
    let peak = |p: &openreadout_plot::Plotted, k: usize| {
        let v = &p.hover[0]["series"][k];
        let (xs, ys) = (v["x"].as_array().unwrap(), v["y"].as_array().unwrap());
        let i = (0..ys.len())
            .max_by(|&a, &b| ys[a].as_f64().unwrap().total_cmp(&ys[b].as_f64().unwrap()))
            .unwrap();
        xs[i].as_f64().unwrap()
    };
    let shared = peak(&p, 1);
    let own = peak(&a, 0);
    assert!(
        shared < own,
        "the dim peak moved up the shared axis: {shared} vs {own}"
    );
}

#[test]
fn histograms_on_a_log_axis_have_log_bins() {
    let d = tempfile::tempdir().unwrap();
    let f = fcs(d.path(), "wide.fcs", &around(500.0, 4000));
    let reg = Registry::new().with(Box::new(openreadout_fcs::FcsReader));
    let spec = parse_spec(json!({
        "x": {"log": true},
        "data": {"file": f, "show": "histogram", "x": "FITC-A", "transform": "none"},
    }))
    .unwrap();
    let p = plot(&reg, &spec).unwrap();
    let v = &p.hover[0]["series"][0];
    let xs: Vec<f64> = v["x"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let ys: Vec<f64> = v["y"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    // bin edges are equal steps of log10(x): the first bin is as wide (in log) as the last
    let edges: Vec<f64> = xs.iter().copied().filter(|x| *x > 0.0).collect();
    let first = (edges[2] / edges[1]).log10();
    let last = (edges[edges.len() - 2] / edges[edges.len() - 3]).log10();
    assert!(
        (first - last).abs() < 0.02,
        "first bin {first}, last bin {last}"
    );
    // and no single bin holds most of the events
    let total: f64 = ys.iter().sum::<f64>() / 2.0;
    let top = ys.iter().copied().fold(0.0, f64::max);
    assert!(top < 0.25 * total, "one bin holds {top} of {total}");
}

#[test]
fn grouped_bars_on_a_log_axis_show_every_bar() {
    let d = tempfile::tempdir().unwrap();
    let t = d.path().join("medians.csv");
    std::fs::write(
        &t,
        "cells,fucose,median\nnormal,no,5437\nnormal,yes,6183\none copy,no,5659\none copy,yes,6352\nknockout,no,189\nknockout,yes,5276\n",
    )
    .unwrap();
    let spec = parse_spec(json!({
        "y": {"log": true},
        "annotate": [{"type": "hline", "y": 189, "label": "knockout"}],
        "data": {"table": t, "x": "cells", "y": "median", "by": "fucose", "mark": "bars"},
    }))
    .unwrap();
    let p = plot(&Registry::new(), &spec).unwrap();
    let y = &p.output.panels[0].y;
    let [lo, hi] = y.range.unwrap();
    assert_eq!(y.scale, "log");
    assert!(lo < 189.0 && hi >= 6352.0, "y range {lo}–{hi}");
    assert_eq!(p.output.panels[0].annotations.len(), 1);
    // ticks: more than one labelled value on the axis
    let svg = p.figure.to_svg();
    for label in ["100", "1k"] {
        assert!(svg.contains(&format!(">{label}<")), "no {label} tick");
    }
}

//! Figures from synthetic data: a kinetic plate as a long CSV, a table file, inline values.

use std::fmt::Write as _;
use std::path::Path;

use openreadout_core::Registry;
use openreadout_plot::{FileFormat, encode, parse_spec, plot, run};
use serde_json::json;

/// A 2 × 4 kinetic plate: row A grows, row B grows then crashes; column 4 is blank.
fn plate(dir: &Path) -> String {
    let mut s = String::from("well,time_s,value,sample,role\n");
    for (r, row) in ["A", "B"].iter().enumerate() {
        for c in 1..=4 {
            for k in 0..=40 {
                let t = f64::from(k) * 900.0;
                let h = t / 3600.0;
                let grow = 0.6 / (1.0 + (-(h - 3.0) * 1.5).exp());
                let v = if c == 4 {
                    0.05
                } else if r == 1 {
                    0.05 + grow * (-(h - 4.0).max(0.0)).exp()
                } else {
                    0.05 + grow
                };
                let (sample, role) = if c == 4 {
                    ("Blank", "blank")
                } else {
                    (if r == 0 { "lambda" } else { "HK97" }, "sample")
                };
                let _ = writeln!(s, "{row}{c},{t},{v:.4},{sample},{role}");
            }
        }
    }
    let p = dir.join("plate.csv");
    std::fs::write(&p, s).unwrap();
    p.display().to_string()
}

#[test]
fn plate_grid_with_flags_and_blank_subtraction() {
    let d = tempfile::tempdir().unwrap();
    let f = plate(d.path());
    let spec = parse_spec(json!({
        "title": "growth",
        "data": {"file": f, "subtract": "blank", "color_by": {"at": 8, "below": 0.1, "label": "crashed", "else": "grew"}},
        "facet": "plate",
        "column_labels": ["0", "5", "50", "blank"],
    }))
    .unwrap();
    let p = plot(&Registry::new(), &spec).unwrap();
    let panel = &p.output.panels[0];
    assert_eq!(panel.layout, "plate");
    assert_eq!(panel.facets, Some(8));
    assert_eq!(panel.x.unit.as_deref(), Some("h"));
    let classes: Vec<_> = panel
        .series
        .iter()
        .filter_map(|s| s.class.clone())
        .collect();
    assert!(
        classes.contains(&"crashed".to_string()) && classes.contains(&"grew".to_string()),
        "{classes:?}"
    );
    // after subtracting the blank, row A tops out near 0.6
    let grew = panel
        .series
        .iter()
        .find(|s| s.class.as_deref() == Some("grew"))
        .unwrap();
    assert!(
        (grew.y_range.unwrap()[1] - 0.6).abs() < 0.05,
        "{:?}",
        grew.y_range
    );
    assert!(p.output.notes.iter().any(|n| n.contains("blank")));
    let png = p.figure.to_png(1.0).unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn group_means_and_annotations() {
    let d = tempfile::tempdir().unwrap();
    let f = plate(d.path());
    let spec = parse_spec(json!({
        "data": {"file": f, "by": "group", "groups": ["lambda", "HK97"]},
        "annotate": [{"x": [3, 4.5], "label": "came back"}, {"y": 0.3}, {"x": 6, "y": 0.4, "label": "here", "to": [8, 0.1]}],
    }))
    .unwrap();
    let p = plot(&Registry::new(), &spec).unwrap();
    let panel = &p.output.panels[0];
    assert_eq!(panel.series.len(), 2);
    assert!(panel.series.iter().all(|s| s.mark == "band"));
    assert_eq!(panel.annotations.len(), 3, "{:?}", panel.annotations);
    // a group the layout does not have names the ones it does
    let bad = parse_spec(json!({"data": {"file": f, "groups": ["lamda"]}})).unwrap();
    let e = plot(&Registry::new(), &bad).unwrap_err().to_string();
    assert!(e.contains("lambda") && e.contains("HK97"), "{e}");
}

#[test]
fn table_columns_grouped_bars() {
    let d = tempfile::tempdir().unwrap();
    let t = d.path().join("rows.csv");
    std::fs::write(
        &t,
        "path,name,percent,parent\n/x/e01.fcs,IFNg+,0.004,CD8\n/x/e03.fcs,IFNg+,2.6,CD8\n/x/e01.fcs,TNFa+,0.02,CD8\n/x/e03.fcs,TNFa+,1.7,CD8\n/x/e03.fcs,CD4,30,T\n",
    )
    .unwrap();
    let spec = parse_spec(json!({
        "data": {"table": t, "x": "name", "y": "percent", "by": "path", "where": ["parent=CD8"]},
    }))
    .unwrap();
    let p = plot(&Registry::new(), &spec).unwrap();
    let panel = &p.output.panels[0];
    assert_eq!(panel.x.categories, vec!["IFNg+", "TNFa+"]);
    let names: Vec<_> = panel.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["e01", "e03"]);
    let bad = parse_spec(json!({"data": {"table": t, "x": "nme", "y": "percent"}})).unwrap();
    let e = plot(&Registry::new(), &bad).unwrap_err().to_string();
    assert!(e.contains("columns: path, name, percent, parent"), "{e}");
}

#[test]
fn inline_values_and_every_output_format() {
    let d = tempfile::tempdir().unwrap();
    let spec = parse_spec(json!({
        "title": "IFNγ+",
        "panels": [
            {"title": "bars", "data": {"values": {"x": ["E01", "E03"], "y": [0.004, 2.6], "y_unit": "%"}}},
            {"title": "points", "data": {"values": {"x": [1, 2, 3], "y": [1, 4, 9], "error": [0.5, 0.5, 1], "labels": ["", "", "nine"]}}},
        ],
        "theme": "dark",
    }))
    .unwrap();
    let p = plot(&Registry::new(), &spec).unwrap();
    assert_eq!(p.output.inline_series, 2);
    assert!(p.output.files.is_empty());
    assert_eq!(p.output.panels[1].series[0].marked[0].text, "nine");
    for (ext, magic) in [("png", &b"\x89PNG"[..]), ("svg", b"<svg"), ("pdf", b"%PDF")] {
        let path = d.path().join(format!("f.{ext}"));
        let mut s = spec.clone();
        s.output = Some(path.display().to_string());
        let out = run(&Registry::new(), &s).unwrap();
        let w = out.output.output.unwrap();
        assert!(w.verified && w.format == ext);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.windows(magic.len()).take(200).any(|x| x == magic),
            "{ext}"
        );
        // never over an existing file without overwrite
        assert!(run(&Registry::new(), &s).is_err());
    }
    let svg = encode(&p.figure, FileFormat::Svg).unwrap();
    assert!(String::from_utf8_lossy(&svg).contains("IFNγ+"));
}

#[test]
fn helpful_errors() {
    let e = parse_spec(json!({"data": {"values": {"x": [1], "y": [1]}}, "colour": "red"}))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("unknown plot argument `colour`") && e.contains("annotate"),
        "{e}"
    );
    let s = parse_spec(json!({"data": {"values": {"x": [1, 2], "y": [1]}}})).unwrap();
    assert!(
        plot(&Registry::new(), &s)
            .unwrap_err()
            .to_string()
            .contains("must match")
    );
    let s = parse_spec(
        json!({"data": {"values": {"x": [1], "y": [1]}}, "annotate": [{"x": [1, 2], "y": [1, 2]}]}),
    )
    .unwrap();
    assert!(
        plot(&Registry::new(), &s)
            .unwrap_err()
            .to_string()
            .contains("annotate[0]")
    );
    let s =
        parse_spec(json!({"data": {"values": {"x": [1], "y": [1]}, "color": "chartreuse-ish"}}))
            .unwrap();
    assert!(
        plot(&Registry::new(), &s)
            .unwrap_err()
            .to_string()
            .contains("#RRGGBB")
    );
    let s = parse_spec(json!({"title": "nothing"})).unwrap();
    assert!(
        plot(&Registry::new(), &s)
            .unwrap_err()
            .to_string()
            .contains("nothing to plot")
    );
}

#[test]
fn growth_colours_and_a_figure_level_file() {
    let d = tempfile::tempdir().unwrap();
    let f = plate(d.path());
    // `file` at the figure level is the file of every series that names none
    let spec = parse_spec(json!({
        "file": f,
        "data": {"color_by": "growth", "subtract": "blank"},
        "facet": "plate",
    }))
    .unwrap();
    let p = plot(&Registry::new(), &spec).unwrap();
    let classes: Vec<_> = p.output.panels[0]
        .series
        .iter()
        .filter_map(|s| s.class.clone())
        .collect();
    assert!(classes.contains(&"grew".to_string()), "{classes:?}");
    assert!(
        classes.iter().any(|c| c.starts_with("crashed")),
        "{classes:?}"
    );
    // a file alone draws its default view
    let p = plot(&Registry::new(), &parse_spec(json!({"file": f})).unwrap()).unwrap();
    assert_eq!(p.output.files, vec![f.clone()]);
}

#[test]
fn misplaced_keys_say_where_they_go() {
    let e = parse_spec(json!({"show": "wells", "data": {"file": "p.csv"}}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("describes a series"), "{e}");
    let e = parse_spec(json!({"data": {"values": {"x": [1], "y": [1]}, "x": {"label": "pA"}}}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("data.x") && e.contains("on the panel"), "{e}");
    let e = parse_spec(json!({"panels": [{"data": [{"file": "a"}, {"file": "b", "title": "t"}]}]}))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("panels[0].data[1]") && e.contains("`title`"),
        "{e}"
    );
}

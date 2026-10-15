//! Figures from synthetic data: a kinetic plate as a long CSV, a table file, inline values.

use std::fmt::Write as _;
use std::path::Path;

use openreadout_core::Registry;
use openreadout_plot::{FileFormat, SaveOptions, encode, parse_spec, plot, write};
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
    let bad = parse_spec(json!({"data": {"file": f, "groups": ["phi80"]}})).unwrap();
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
    for (ext, magic) in [
        ("png", &b"\x89PNG"[..]),
        ("jpg", b"\xFF\xD8"),
        ("tiff", b"II*\0"),
        ("svg", b"<svg"),
        ("pdf", b"%PDF"),
    ] {
        let path = d.path().join(format!("f.{ext}"));
        let w = write(&p.figure, &path, &SaveOptions::default(), &[]).unwrap();
        assert!(w.verified && FileFormat::from_name(ext).unwrap().as_str() == w.format);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            bytes.windows(magic.len()).take(200).any(|x| x == magic),
            "{ext}"
        );
        // never over an existing file without overwrite
        assert!(write(&p.figure, &path, &SaveOptions::default(), &[]).is_err());
    }
    let (svg, ..) = encode(&p.figure, FileFormat::Svg, &SaveOptions::default()).unwrap();
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
    let e = parse_spec(json!({"data": {"file": "r.raw", "tic": true}}))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("unknown field `tic`") && e.contains("chromatogram"),
        "{e}"
    );
    let e = parse_spec(json!({"data": {"file": "a"}, "panels": [], "y": {"name": "OD"}}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("y: unknown field `name`"), "{e}");
    let e = parse_spec(json!({"panels": [{"data": [{"file": "a"}, {"file": "b", "title": "t"}]}]}))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("panels[0].data[1]") && e.contains("`title`"),
        "{e}"
    );
}

/// One-element lists written as the element alone (R's `auto_unbox`) are read as lists.
#[test]
fn single_values_where_lists_go() {
    let spec = parse_spec(json!({
        "data": {"values": {"x": "a", "y": 2.5, "error": 0.1, "labels": "only"}, "mark": "bars"},
        "column_labels": "dose",
        "row_labels": "row",
    }))
    .unwrap();
    let v = spec.panel.data.as_ref().unwrap().to_vec()[0]
        .values
        .clone()
        .unwrap();
    assert_eq!(
        (v.x.len(), v.y, v.error, v.labels.len()),
        (1, vec![2.5], vec![0.1], 1)
    );
    assert_eq!(spec.panel.column_labels, vec!["dose".to_string()]);
    plot(&Registry::new(), &spec).unwrap();
    let spec = parse_spec(
        json!({"data": {"table": "t.csv", "x": "a", "y": "b", "where": "a=1", "groups": "g"}}),
    )
    .unwrap();
    let s = &spec.panel.data.as_ref().unwrap().to_vec()[0];
    assert_eq!((s.where_.len(), s.groups.len()), (1, 1));
}

/// Lists are deduplicated and capped, the data read is budgeted, and text that would leave no
/// room for the plots is refused, each with a usage error.
#[test]
fn limits_on_lists_points_and_text() {
    let dir = tempfile::tempdir().unwrap();
    let f = plate(dir.path());
    let usage = |v: serde_json::Value| {
        let e = parse_spec(v)
            .and_then(|s| plot(&Registry::new(), &s))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(e.exit_code(), 2, "{e}");
        e.to_string()
    };
    // a file named 200 times is read once
    let plotted = plot(
        &Registry::new(),
        &parse_spec(json!({"data": {"file": vec![f.clone(); 200]}})).unwrap(),
    )
    .unwrap();
    assert_eq!(plotted.output.files.len(), 1);
    let many: Vec<String> = (0..200).map(|i| format!("{f}{i}")).collect();
    assert!(usage(json!({"data": {"file": many}})).contains("over the limit"));
    let sweeps: Vec<u32> = (0..500).collect();
    assert!(usage(json!({"data": {"file": f, "sweep": sweeps}})).contains("over the limit"));
    let count = 4_000_000;
    let x: Vec<f64> = (0..count).map(f64::from).collect();
    let big = json!({"x": x, "y": x});
    let e = usage(
        json!({"data": [{"values": big}, {"values": big}, {"values": big}, {"values": big}]}),
    );
    assert!(e.contains("data points"), "{e}");
    let panels: Vec<_> = (0..30)
        .map(|_| json!({"data": {"values": {"x": [1], "y": [1]}}}))
        .collect();
    assert!(usage(json!({"panels": panels})).contains("panels"));
    let anns: Vec<_> = (0..600)
        .map(|i| json!({"x": i, "y": 1, "label": "a"}))
        .collect();
    assert!(
        usage(json!({"data": {"values": {"x": [1], "y": [1]}}, "annotate": anns}))
            .contains("annotations")
    );
    let words = "word ".repeat(20_000);
    assert!(
        usage(json!({"data": {"values": {"x": [1], "y": [1]}}, "caption": words}))
            .contains("caption")
    );
    assert!(
        usage(json!({"data": {"values": {"x": [1], "y": [1]}}, "title": "t".repeat(400)}))
            .contains("title")
    );
    // a caption that fits the limit but not a short figure
    let e = usage(
        json!({"data": {"values": {"x": [1], "y": [1]}}, "caption": "word ".repeat(390), "width": 320, "height": 200}),
    );
    assert!(e.contains("no room"), "{e}");
    // long words wrap quickly
    let started = std::time::Instant::now();
    let caption = vec!["x".repeat(390); 5].join(" ");
    plot(
        &Registry::new(),
        &parse_spec(
            json!({"data": {"values": {"x": [1], "y": [1]}}, "caption": caption, "height": 1500}),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
}

/// A figure is never saved over one of the files it was drawn from.
#[test]
fn a_figure_never_replaces_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let f = plate(dir.path());
    let spec = parse_spec(json!({"data": {"file": f}})).unwrap();
    let inputs = spec.input_files();
    assert_eq!(inputs, vec![std::path::PathBuf::from(&f)]);
    let plotted = plot(&Registry::new(), &spec).unwrap();
    let o = SaveOptions {
        overwrite: true,
        ..SaveOptions::default()
    };
    // the plate file renamed to look like a figure, reached through `..`
    let png = dir.path().join("plate.png");
    std::fs::copy(&f, &png).unwrap();
    let dotted = dir.path().join("x/../plate.png");
    let refs: Vec<&Path> = vec![png.as_path()];
    let e = write(&plotted.figure, &dotted, &o, &refs).unwrap_err();
    assert_eq!(e.exit_code(), 2, "{e}");
    assert_eq!(std::fs::read(&png).unwrap(), std::fs::read(&f).unwrap());
}

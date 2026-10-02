//! Crawl, search, health and export on a synthetic share (no corpus).
#![allow(clippy::many_single_char_names)]

use std::io::Write as _;
use std::path::Path;

use openreadout_core::Registry;
use openreadout_index::tables::{Cell, scan_table};
use openreadout_index::{HealthOptions, IndexOptions, SearchRequest, health, index, search, synth};

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_fcs::FcsReader))
        .with(Box::new(openreadout_tiff::TiffReader))
}

fn opts(root: &Path, idx: &Path) -> IndexOptions {
    let mut o = IndexOptions::default();
    o.roots = vec![root.to_path_buf()];
    o.index_dir = idx.to_path_buf();
    o.chunk = 3;
    o
}

fn q(idx: &Path, query: &str) -> Vec<String> {
    let mut r = SearchRequest::default();
    r.query = query.into();
    r.limit = Some(0);
    search(idx, &r)
        .unwrap()
        .results
        .iter()
        .map(|m| {
            let p = m["path"].as_str().unwrap();
            Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// Rows of experiments.parquet without the columns that change between runs.
fn stable_rows(idx: &Path) -> Vec<String> {
    let mut out = Vec::new();
    scan_table(&idx.join("experiments.parquet"), None, &mut |rows| {
        for r in 0..rows.len() {
            let mut s = String::new();
            for (c, name) in rows.names().iter().enumerate() {
                if matches!(
                    name.as_str(),
                    "indexed_at" | "elapsed_ms" | "change" | "moved_from"
                ) {
                    continue;
                }
                s.push_str(&format!("{name}={}|", rows.cell(r, c).to_text()));
            }
            out.push(s);
        }
        Ok(())
    })
    .unwrap();
    out
}

#[test]
fn crawl_search_incremental_resume_health() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("share");
    let idx = tmp.path().join("index");
    synth::share(&root).unwrap();
    let reg = registry();

    let m = index(&reg, &opts(&root, &idx), None).unwrap();
    assert!(m.complete);
    // 3 TIFFs + 4 FCS; notes.txt, README.md, data.bin, empty are unknown; .DS_Store hidden
    assert_eq!(m.crawl.items, 11, "{:?}", m.crawl);
    assert_eq!(m.datasets, 7);
    assert_eq!(m.crawl.unknown, 4);
    assert_eq!(m.crawl.walk.hidden_skipped, 1);
    assert_eq!(m.unknown_extensions["txt"].count, 1);
    assert_eq!(m.unknown_extensions["(none)"].count, 1);
    assert_eq!(m.formats["fcs"].count, 4);
    assert_eq!(m.formats["tiff"].count, 3);
    assert!(m.pii.flags >= 3, "{:?}", m.pii);
    assert!(idx.join("state/records.jsonl").exists());
    assert!(!idx.join("state/journal.jsonl").exists());

    // Search.
    assert_eq!(q(&idx, "format=fcs sample~A12"), ["tube_A12.fcs"]);
    assert_eq!(q(&idx, "format=fcs acquired<2020"), ["tube_A12.fcs"]);
    assert_eq!(
        q(&idx, "format=fcs acquired>=2021 -status=truncated"),
        ["tube_B03.fcs", "tube_C07.fcs"]
    );
    assert_eq!(
        q(&idx, "format=tiff x>16"),
        ["cells_b.tif", "cells_b_copy.tif"]
    );
    assert_eq!(q(&idx, "rows=20|30"), ["tube_A12.fcs", "tube_B03.fcs"]);
    assert_eq!(q(&idx, "pii=email"), ["tube_A12.fcs"]);
    assert_eq!(q(&idx, "pii=true format=tiff"), Vec::<String>::new());
    assert_eq!(
        q(&idx, "status=truncated|corrupt|unreadable"),
        ["tube_C07_truncated.fcs"]
    );
    assert_eq!(
        q(&idx, "technique:CHMO_0000061 -sample~roe"),
        ["tube_A12.fcs", "tube_C07.fcs", "tube_C07_truncated.fcs"]
    );
    assert_eq!(q(&idx, "channel~SSC events>35 status=ok"), ["tube_C07.fcs"]);
    assert_eq!(
        q(&idx, "cells_a OR sample=A12"),
        ["tube_A12.fcs", "cells_a.tif"],
        "walk order: flow/ before imaging/"
    );
    let mut r = SearchRequest::default();
    r.sort = Some("-size".into());
    r.limit = Some(2);
    let s = search(&idx, &r).unwrap();
    assert_eq!(s.total, 7);
    assert_eq!(s.returned, 2);
    assert!(s.truncated);
    let sizes: Vec<u64> = s
        .results
        .iter()
        .map(|x| x["size_bytes"].as_u64().unwrap())
        .collect();
    assert!(sizes[0] >= sizes[1]);

    // Health.
    let h = health(&idx, &HealthOptions::default()).unwrap();
    assert_eq!(h.duplicates.group_count, 1, "{:?}", h.duplicates);
    assert!(h.duplicates.confirmed);
    assert_eq!(h.duplicates.groups[0].paths.len(), 2);
    assert_eq!(h.integrity.problem_count, 1);
    assert_eq!(h.at_risk.count, 0, "TIFF and FCS are open formats");
    assert!(h.pii.by_kind.contains_key("person_name"));
    let md = openreadout_index::health::render_markdown(&h);
    assert!(md.contains("# Storage health report"));
    assert!(
        !md.contains("Jane Roe"),
        "health never prints flagged values"
    );

    // Incremental: nothing changed.
    let full = stable_rows(&idx);
    let m2 = index(&reg, &opts(&root, &idx), None).unwrap();
    assert_eq!(m2.crawl.unchanged, 11, "{:?}", m2.crawl);
    assert_eq!(m2.crawl.new + m2.crawl.changed, 0);
    assert_eq!(stable_rows(&idx), full);

    // A changed file, a moved file and a removed file.
    std::fs::write(
        root.join("imaging/2019/cells_a.tif"),
        synth::tiff(16, 8, 4, 1),
    )
    .unwrap();
    std::fs::rename(
        root.join("flow/run1/tube_B03.fcs"),
        root.join("flow/tube_B03_moved.fcs"),
    )
    .unwrap();
    std::fs::remove_file(root.join("misc/data.bin")).unwrap();
    let m3 = index(&reg, &opts(&root, &idx), None).unwrap();
    assert_eq!(m3.crawl.changed, 1, "{:?}", m3.crawl);
    assert_eq!(m3.crawl.removed, 2);
    assert_eq!(m3.changes["moved"], 1, "{:?}", m3.changes);
    assert_eq!(q(&idx, "change=moved"), ["tube_B03_moved.fcs"]);
    assert_eq!(q(&idx, "cells_a planes=4"), ["cells_a.tif"]);

    // Resume: a capped run stops after 4 items, reruns continue, the result equals one run.
    let idx2 = tmp.path().join("index2");
    let mut capped = opts(&root, &idx2);
    capped.max_files = Some(4);
    let p = index(&reg, &capped, None).unwrap();
    assert!(!p.complete);
    assert!(p.next.as_deref().unwrap_or("").contains("again"));
    assert!(idx2.join("state/journal.jsonl").exists());
    let mut sessions = 1;
    loop {
        let p = index(&reg, &capped, None).unwrap();
        sessions += 1;
        if p.complete {
            assert_eq!(p.crawl.sessions, sessions);
            assert_eq!(p.crawl.items, 10);
            break;
        }
        assert!(sessions < 10);
    }
    let idx3 = tmp.path().join("index3");
    index(&reg, &opts(&root, &idx3), None).unwrap();
    let norm = |v: Vec<String>, from: &Path| -> Vec<String> {
        v.into_iter()
            .map(|s| s.replace(&from.display().to_string(), "IDX"))
            .collect()
    };
    assert_eq!(
        norm(stable_rows(&idx2), &idx2),
        norm(stable_rows(&idx3), &idx3)
    );

    // A killed run leaves a torn journal line: the next run drops it and continues.
    let idx4 = tmp.path().join("index4");
    let mut c4 = opts(&root, &idx4);
    c4.max_files = Some(3);
    index(&reg, &c4, None).unwrap();
    let mut j = std::fs::OpenOptions::new()
        .append(true)
        .open(idx4.join("state/journal.jsonl"))
        .unwrap();
    j.write_all(b"{\"t\":\"rec\",\"v\":{\"path\":\"/tor")
        .unwrap();
    drop(j);
    c4.max_files = None;
    let done = index(&reg, &c4, None).unwrap();
    assert!(done.complete);
    assert_eq!(done.crawl.items, 10);
    assert_eq!(
        norm(stable_rows(&idx4), &idx4),
        norm(stable_rows(&idx3), &idx3)
    );

    // Search cells.
    let mut r = SearchRequest::default();
    r.query = "format=fcs sample=A12".into();
    r.fields = vec!["all".into()];
    let s = search(&idx3, &r).unwrap();
    assert_eq!(s.results[0]["operator"], "Jane Roe");
    assert!(
        s.results[0]["pii_kinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "email")
    );
    let _ = Cell::Null;
}

#[test]
fn export_dataset_with_redaction() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("share");
    let idx = tmp.path().join("index");
    synth::share(&root).unwrap();
    std::fs::write(
        root.join("flow/LICENSE"),
        "Creative Commons Attribution 4.0 International",
    )
    .unwrap();
    let reg = registry();
    index(&reg, &opts(&root, &idx), None).unwrap();
    let out = tmp.path().join("out");
    let mut o = openreadout_index::ExportDatasetOptions::default();
    o.query = "format=fcs|tiff -status=truncated".into();
    o.output = out.clone();
    o.redact = Some(openreadout_index::pii::Redactor::new(
        b"test-salt-123".to_vec(),
    ));
    let r = openreadout_index::export_dataset(&reg, &idx, &o, None).unwrap();
    assert_eq!(r.datasets.len(), 6, "{:?}", r.skipped);
    assert!(r.verified);
    assert_eq!(r.counts["tables"], 3);
    assert_eq!(r.counts["table_rows"], 90);
    assert_eq!(r.counts["images"], 3);
    assert_eq!(r.licenses.get("CC-BY-4.0"), Some(&3));
    let sheet = std::fs::read_to_string(out.join("datasheet.json")).unwrap();
    assert!(!sheet.contains("Jane Roe"));
    assert!(!sheet.contains("jroe@example.org"));
    assert!(sheet.contains("redacted:"));
    assert!(out.join("DATASHEET.md").exists());
    for d in &r.datasets {
        for f in &d.outputs {
            assert!(out.join(&f.file).exists(), "{}", f.file);
            if f.kind == "metadata" {
                let t = std::fs::read_to_string(out.join(&f.file)).unwrap();
                assert!(!t.contains("Jane Roe"), "{}", f.file);
            }
        }
    }
    // Rerun: everything is resumed, nothing exported again.
    let r2 = openreadout_index::export_dataset(&reg, &idx, &o, None).unwrap();
    assert_eq!(r2.resumed, 6);
    assert_eq!(r2.exported_now, 0);
    // CSV tables.
    let out2 = tmp.path().join("out_csv");
    let mut o2 = openreadout_index::ExportDatasetOptions::default();
    o2.query = "sample=C07".into();
    o2.output = out2.clone();
    o2.tables = openreadout_index::export::TableFormat::Csv;
    let r3 = openreadout_index::export_dataset(&reg, &idx, &o2, None).unwrap();
    let csv = r3.datasets[0]
        .outputs
        .iter()
        .find(|f| f.kind == "table")
        .unwrap();
    let text = std::fs::read_to_string(out2.join(&csv.file)).unwrap();
    assert_eq!(text.lines().next(), Some("FSC-A,SSC-A,Time"));
    assert_eq!(text.lines().count(), 41);
}

/// A directory data set whose reader names its member files (an ImageXpress plate folder: the
/// HTD plus one TIFF per plane) is reused on the next crawl, and a changed plane file makes it
/// changed. The members are listed once (not once by the walk and again by the reader).
#[test]
fn directory_datasets_are_reused_when_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("share");
    let idx = tmp.path().join("index");
    let plate = root.join("screen/P7");
    std::fs::create_dir_all(&plate).unwrap();
    let htd = "\"HTSInfoFile\", Version 1.0\r\n\"XWells\", 12\r\n\"YWells\", 8\r\n\"WellsSelection1\", TRUE, TRUE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE, FALSE\r\n\"Sites\", FALSE\r\n\"Waves\", TRUE\r\n\"NWavelengths\", 2\r\n\"WaveName1\", \"DAPI\"\r\n\"WaveName2\", \"FITC\"\r\n\"EndFile\"\r\n";
    std::fs::write(plate.join("P7.HTD"), htd).unwrap();
    let mut seed = 0;
    for well in ["A01", "A02"] {
        for wave in [1, 2] {
            seed += 1;
            std::fs::write(
                plate.join(format!("P7_{well}_w{wave}.tif")),
                synth::tiff(8, 4, 1, seed),
            )
            .unwrap();
        }
    }
    let reg = Registry::new()
        .with(Box::new(openreadout_hcs::ImageXpressReader))
        .with(Box::new(openreadout_tiff::TiffReader));
    let m = index(&reg, &opts(&root, &idx), None).unwrap();
    assert_eq!(m.formats["imagexpress"].count, 1, "{:?}", m.formats);
    assert_eq!(m.crawl.items, 1, "{:?}", m.crawl);
    let records = std::fs::read_to_string(idx.join("state/records.jsonl")).unwrap();
    let rec: serde_json::Value = records
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| v["v"].clone())
        .find(|v| v["format"] == "imagexpress")
        .unwrap();
    assert_eq!(rec["members"].as_array().unwrap().len(), 5, "{rec}");

    let m2 = index(&reg, &opts(&root, &idx), None).unwrap();
    assert_eq!(m2.crawl.unchanged, 1, "{:?}", m2.crawl);
    assert_eq!(m2.crawl.changed, 0);

    std::fs::write(plate.join("P7_A02_w2.tif"), synth::tiff(8, 4, 2, 9)).unwrap();
    let m3 = index(&reg, &opts(&root, &idx), None).unwrap();
    assert_eq!(m3.crawl.changed, 1, "{:?}", m3.crawl);
    let m4 = index(&reg, &opts(&root, &idx), None).unwrap();
    assert_eq!(m4.crawl.unchanged, 1, "{:?}", m4.crawl);
}

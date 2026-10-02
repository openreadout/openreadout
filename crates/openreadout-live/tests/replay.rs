//! Replay-driven tests on synthetic files (no corpus needed): an OME-TIFF with its OME-XML at
//! the end and an OME-Zarr store are rewritten step by step; at every step the reader must
//! report every complete plane and the unfinished rest as `in_progress`, and the final output
//! must be complete and hash-identical to the source. The same files watched with `Watcher`
//! give one `plane_new` per plane. Corpus files (ND2, CZI, real OME-TIFF, Thermo RAW) are
//! covered by `openreadout-corpus-tests/tests/live.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use openreadout_core::Registry;
use openreadout_core::live::{AcquisitionState, assess_dataset, assess_with};
use openreadout_live::replay::{Pattern, Replayer, plan, tree_hash};
use openreadout_live::watch::{EventKind, WatchOptions, Watcher};

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
}

fn ome_tiff(w: u32, h: u32, pages: u32) -> Vec<u8> {
    openreadout_live::synth::ome_tiff(w, h, pages)
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

struct Seen {
    complete: u64,
    state: Option<AcquisitionState>,
    format_planes: u64,
}

fn look(reg: &Registry, p: &Path) -> Option<Seen> {
    let (_, ds) = reg.open(p).ok()?;
    let info = ds.info().ok()?;
    let acq = assess_dataset(ds.as_ref(), p);
    Some(Seen {
        complete: acq.as_ref().map_or(info.plane_count, |a| a.complete_planes),
        state: acq.map(|a| a.state),
        format_planes: info.plane_count,
    })
}

#[test]
fn ome_tiff_replay_reports_complete_planes_then_finishes_identical() {
    let reg = registry();
    let d = tmp();
    let src = d.path().join("src.ome.tif");
    std::fs::write(&src, ome_tiff(16, 8, 6)).unwrap();
    let dst = d.path().join("live.ome.tif");
    let plan = plan(&src, Pattern::OmeTiff, true).unwrap();
    assert_eq!(plan.units(), 6);
    let mut r = Replayer::new(plan, &dst).unwrap();
    let mut units = 0u64;
    let mut checked_partial = false;
    while let Some(step) = r.step().unwrap().cloned() {
        if step.unit.is_some() {
            units += 1;
        }
        if r.finished() {
            break;
        }
        let s = look(&reg, &dst).expect("a growing OME-TIFF opens");
        assert_eq!(
            s.state,
            Some(AcquisitionState::InProgress),
            "after {} units ({})",
            units,
            step.kind
        );
        assert_eq!(
            s.complete, units,
            "complete planes after {units} units ({})",
            step.kind
        );
        checked_partial |= step.kind == "partial" && units >= 1;
    }
    assert!(checked_partial);
    let s = look(&reg, &dst).unwrap();
    assert_eq!(s.state, None, "finished file is complete");
    assert_eq!(s.format_planes, 6);
    assert_eq!(tree_hash(&dst).unwrap(), tree_hash(&src).unwrap());
}

#[test]
fn old_or_truncated_files_are_not_in_progress() {
    let reg = registry();
    let d = tmp();
    let src = d.path().join("src.ome.tif");
    let bytes = ome_tiff(16, 8, 6);
    std::fs::write(&src, &bytes).unwrap();
    // A partial write left alone for an hour: interrupted.
    let dst = d.path().join("old.ome.tif");
    let p = plan(&src, Pattern::OmeTiff, false).unwrap();
    let mut r = Replayer::new(p, &dst).unwrap();
    for _ in 0..3 {
        r.step().unwrap();
    }
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&dst)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    let (_, ds) = reg.open(&dst).unwrap();
    let ws = ds.write_state().expect("still incomplete");
    let a = assess_with(&dst, &ws, Duration::from_secs(300)).unwrap();
    assert_eq!(a.state, AcquisitionState::Interrupted);
    // A finished file cut short (a truncated copy): its description points past the end.
    let cut = d.path().join("cut.ome.tif");
    std::fs::write(&cut, &bytes[..bytes.len() * 6 / 10]).unwrap();
    let (_, ds) = reg.open(&cut).unwrap();
    assert!(
        ds.write_state().is_none(),
        "a truncated copy is not a growing file"
    );
}

fn zarr_source(dir: &Path) -> PathBuf {
    let tif = dir.join("stack.tif");
    std::fs::write(&tif, openreadout_index_free_tiff(16, 8, 5)).unwrap();
    let reg = registry();
    let (_, mut ds) = reg.open(&tif).unwrap();
    let out = dir.join("src.ome.zarr");
    let mut o = openreadout_omezarr::ZarrExportOptions::default();
    o.chunk = 8;
    openreadout_omezarr::export_ome_zarr(ds.as_mut(), &tif, &out, &o).unwrap();
    out
}

/// A plain multi-page uint16 TIFF (pages become Z planes).
fn openreadout_index_free_tiff(w: u32, h: u32, pages: u32) -> Vec<u8> {
    let bytes = ome_tiff(w, h, pages);
    // Drop the OME-XML: the same pages as a plain TIFF.
    let mut plain = bytes.clone();
    // The description entry is the 6th of page 0 (at 8 + 2 + 5*12); make it an empty inline
    // string so the file reads as a plain TIFF.
    let e = 8 + 2 + 5 * 12;
    plain[e + 4..e + 12].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    plain
}

#[test]
fn ome_zarr_replay_counts_chunks_then_finishes_identical() {
    let reg = registry();
    let d = tmp();
    let src = zarr_source(d.path());
    let dst = d.path().join("live.ome.zarr");
    let p = plan(&src, Pattern::OmeZarr, false).unwrap();
    let units = p.units();
    assert!(units >= 5, "level-0 chunk files: {units}");
    let mut r = Replayer::new(p, &dst).unwrap();
    let mut done = 0u64;
    let mut last_complete = 0u64;
    let mut saw_progress = false;
    while let Some(step) = r.step().unwrap().cloned() {
        if step.unit.is_some() {
            done += 1;
        }
        if r.finished() {
            break;
        }
        let Some(s) = look(&reg, &dst) else {
            continue; // metadata documents not all written yet
        };
        if done < units {
            if done > 0 {
                assert_eq!(
                    s.state,
                    Some(AcquisitionState::InProgress),
                    "after {done} chunks"
                );
            }
            assert!(s.complete >= last_complete, "complete planes never go back");
            last_complete = s.complete;
            saw_progress |= s.complete > 0 && s.complete < 5;
        }
    }
    assert!(saw_progress);
    let s = look(&reg, &dst).unwrap();
    assert_eq!(s.state, None);
    assert_eq!(s.format_planes, 5);
    assert_eq!(tree_hash(&dst).unwrap(), tree_hash(&src).unwrap());
}

#[test]
fn watch_reports_each_plane_once_and_completion() {
    let reg = registry();
    let d = tmp();
    let src = d.path().join("src.ome.tif");
    std::fs::write(&src, ome_tiff(16, 8, 4)).unwrap();
    let inbox = d.path().join("inbox");
    std::fs::create_dir(&inbox).unwrap();
    let dst = inbox.join("run.ome.tif");
    let mut r = Replayer::new(plan(&src, Pattern::OmeTiff, true).unwrap(), &dst).unwrap();
    let mut o = WatchOptions::default();
    o.roots.push(inbox.clone());
    o.qc = Some(openreadout_live::qc::Rules::defaults());
    let mut w = Watcher::new(o);
    let mut events = Vec::new();
    while r.step().unwrap().is_some() {
        w.poll(&reg, &mut |e| events.push(e));
    }
    // Two more polls: the finished file must be seen unchanged once before it is complete.
    w.poll(&reg, &mut |e| events.push(e));
    w.poll(&reg, &mut |e| events.push(e));
    let kinds: Vec<EventKind> = events.iter().map(|e| e.event).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == EventKind::DatasetNew)
            .count(),
        1
    );
    let planes: Vec<u32> = events
        .iter()
        .filter(|e| e.event == EventKind::PlaneNew)
        .map(|e| e.t.unwrap() + e.z.unwrap())
        .collect();
    assert_eq!(planes.len(), 4, "{events:#?}");
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == EventKind::DatasetComplete)
            .count(),
        1,
        "{kinds:?}"
    );
    assert_eq!(*kinds.last().unwrap(), EventKind::DatasetComplete);
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
}

//! Files still being written, on corpus files: each file is replayed with its format's write
//! pattern (`openreadout_live::replay`, every unit written in two steps so a unit in flight is
//! seen too). At every step the reader must report the complete planes (never more than were
//! written, never fewer than the units written) as `in_progress`; the finished replay must be
//! complete, hash-identical to the source and read the same planes. Truncated copies of the
//! finished files must not look in progress. `watch_latency_corpus` prints how long a new
//! plane takes to become an event with the in-process watcher.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test live -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use openreadout_core::live::{AcquisitionState, assess_dataset, assess_with};
use openreadout_core::{Error, Registry};
use openreadout_live::replay::{Pattern, Replayer, plan, tree_hash};
use openreadout_live::watch::{EventKind, WatchOptions, Watcher};

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
}

fn source(name: &str) -> Option<PathBuf> {
    let p = corpus_dir().join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skip {name}: not in the corpus");
        None
    }
}

/// Replay `name`; `planes_per_unit` is how many planes one unit (frame chunk, subblock, page)
/// completes.
fn replay_file(name: &str, pattern: Pattern, planes_per_unit: u64) {
    let Some(src) = source(name) else { return };
    let reg = registry();
    let d = tempfile::tempdir().unwrap();
    let dst = d.path().join(name);
    let p = plan(&src, pattern, true).unwrap();
    let units = p.units();
    let mut r = Replayer::new(p, &dst).unwrap();
    let mut done = 0u64;
    let mut in_progress_seen = 0;
    while let Some(step) = r.step().unwrap().cloned() {
        if step.unit.is_some() {
            done += 1;
        }
        if r.finished() {
            break;
        }
        let Ok((_, ds)) = reg.open(&dst) else {
            assert!(
                done <= 1,
                "{name}: unreadable after {done} of {units} units"
            );
            continue;
        };
        if ds.info().is_err() {
            assert!(done <= 1, "{name}: info fails after {done} units");
            continue;
        }
        let Some(a) = assess_dataset(ds.as_ref(), &dst) else {
            // Before the first unit the file may look like an empty, finished container.
            assert!(
                done == 0,
                "{name}: no acquisition status after {done} of {units} units ({})",
                step.kind
            );
            continue;
        };
        assert_eq!(
            a.state,
            AcquisitionState::InProgress,
            "{name} after {done} units"
        );
        assert_eq!(
            a.complete_planes,
            done * planes_per_unit,
            "{name}: complete planes after {done} units ({})",
            step.kind
        );
        in_progress_seen += 1;
    }
    assert!(
        in_progress_seen >= units,
        "{name}: {in_progress_seen} in-progress states"
    );
    assert_eq!(
        tree_hash(&dst).unwrap(),
        tree_hash(&src).unwrap(),
        "{name}: final hash"
    );
    let (_, ds) = reg.open(&dst).unwrap();
    assert!(
        assess_dataset(ds.as_ref(), &dst).is_none(),
        "{name}: finished"
    );
    let (_, orig) = reg.open(&src).unwrap();
    assert_eq!(
        ds.info().unwrap().plane_count,
        orig.info().unwrap().plane_count
    );
    eprintln!("{name}: {units} units replayed, in progress at every step, final hash identical");
}

#[test]
fn nd2_replay() {
    replay_file("aics-ND2-dims-p1z5t3c2y32x32.nd2", Pattern::Nd2, 2);
}

#[test]
fn czi_replay() {
    replay_file("zenodo7015307-T-2-Z-5-CH-1.czi", Pattern::Czi, 1);
    replay_file("zenodo7015307-T-3-Z-5-CH-2.czi", Pattern::Czi, 1);
}

#[test]
fn ome_tiff_replay() {
    replay_file("ome-artificial-time-series.ome.tiff", Pattern::OmeTiff, 1);
    replay_file("aics-s-3-t-1-c-3-z-5.ome.tiff", Pattern::OmeTiff, 1);
}

#[test]
fn thermo_raw_append_is_never_called_in_progress() {
    let name = "mtbls805-msms-869.raw";
    let Some(src) = source(name) else { return };
    let reg = registry();
    let d = tempfile::tempdir().unwrap();
    let dst = d.path().join(name);
    let mut r = Replayer::new(plan(&src, Pattern::Append, false).unwrap(), &dst).unwrap();
    while r.step().unwrap().is_some() {
        if r.finished() {
            break;
        }
        match reg.open(&dst).and_then(|(_, ds)| ds.info().map(|_| ds)) {
            Ok(ds) => assert!(ds.write_state().is_none()),
            Err(e) => assert!(matches!(e, Error::Corrupt { .. }), "{e}"),
        }
    }
    assert_eq!(tree_hash(&dst).unwrap(), tree_hash(&src).unwrap());
    let (_, ds) = reg.open(&dst).unwrap();
    assert_eq!(ds.info().unwrap().spectra[0].scan_count, 13);
}

#[test]
fn truncated_copies_are_not_in_progress() {
    let reg = registry();
    let d = tempfile::tempdir().unwrap();
    for name in [
        "zenodo7015307-T-2-Z-5-CH-1.czi",
        "ome-artificial-time-series.ome.tiff",
        "aics-ND2-dims-p1z5t3c2y32x32.nd2",
    ] {
        let Some(src) = source(name) else { continue };
        let bytes = std::fs::read(&src).unwrap();
        let cut = d.path().join(format!("cut-{name}"));
        std::fs::write(&cut, &bytes[..bytes.len() * 6 / 10]).unwrap();
        let (_, ds) = reg.open(&cut).unwrap();
        let ws = ds.write_state();
        if Path::new(name).extension().is_some_and(|e| e == "nd2") {
            // ND2 keeps no end-of-file pointer that a cut copy would betray: only the time
            // tells. Fresh: in progress; an hour old: interrupted.
            let ws = ws.expect("an ND2 without its chunk map is incomplete");
            let hour_ago = SystemTime::now() - Duration::from_secs(3600);
            std::fs::File::options()
                .write(true)
                .open(&cut)
                .unwrap()
                .set_modified(hour_ago)
                .unwrap();
            let a = assess_with(&cut, &ws, Duration::from_secs(300)).unwrap();
            assert_eq!(a.state, AcquisitionState::Interrupted, "{name}");
        } else {
            assert!(ws.is_none(), "{name}: a truncated copy points past its end");
        }
    }
}

#[test]
fn watch_latency_corpus() {
    for (name, pattern) in [
        ("aics-ND2-dims-p1z5t3c2y32x32.nd2", Pattern::Nd2),
        ("zenodo7015307-T-2-Z-5-CH-1.czi", Pattern::Czi),
        ("aics-s-3-t-1-c-3-z-5.ome.tiff", Pattern::OmeTiff),
    ] {
        let Some(src) = source(name) else { continue };
        let d = tempfile::tempdir().unwrap();
        let inbox = d.path().join("inbox");
        std::fs::create_dir(&inbox).unwrap();
        let dst = inbox.join(name);
        let p = plan(&src, pattern, false).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<Instant>();
        let writer = std::thread::spawn(move || {
            let mut r = Replayer::new(p, &dst).unwrap();
            while let Some(step) = r.step().unwrap().cloned() {
                if step.unit.is_some() {
                    tx.send(Instant::now()).unwrap();
                    std::thread::sleep(Duration::from_millis(150));
                }
            }
        });
        let reg = registry();
        let mut o = WatchOptions::default();
        o.roots.push(inbox);
        let mut w = Watcher::new(o);
        let mut writes = Vec::new();
        let mut first_seen: Vec<Instant> = Vec::new();
        let mut complete = false;
        let deadline = Instant::now() + Duration::from_secs(60);
        while !complete && Instant::now() < deadline {
            w.poll(&reg, &mut |e| {
                if e.event == EventKind::PlaneNew {
                    first_seen.push(Instant::now());
                }
                complete |= e.event == EventKind::DatasetComplete;
            });
            while let Ok(t) = rx.try_recv() {
                writes.push(t);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        writer.join().unwrap();
        while let Ok(t) = rx.try_recv() {
            writes.push(t);
        }
        assert!(complete, "{name}: complete");
        // Units complete planes in order; for ND2 one frame completes two planes.
        let per = first_seen.len() / writes.len().max(1);
        let mut lat: Vec<f64> = writes
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                first_seen
                    .get(i * per.max(1))
                    .map(|s| s.saturating_duration_since(*w).as_secs_f64() * 1000.0)
            })
            .collect();
        lat.sort_by(f64::total_cmp);
        eprintln!(
            "{name}: {} units, plane_new latency median {:.0} ms, max {:.0} ms (poll 250 ms, units every 150 ms)",
            lat.len(),
            lat[lat.len() / 2],
            lat[lat.len() - 1]
        );
        assert!(lat[lat.len() - 1] < 2000.0, "{name}");
    }
}

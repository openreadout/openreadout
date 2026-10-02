//! Fuzz regressions for every reader in the registry.
//!
//! Replays each `crates/*/tests/fixtures/malformed/file-*` input (a whole file that crashed a
//! cargo-fuzz target in `fuzz/`, or a hand-made file that exercises an allocation guard)
//! through **every** registered reader, and each `bundle-<target>-*` input (a multi-file data
//! set joined with the fuzz harness's separator), written out as a directory, likewise. Every
//! `Dataset` method must return `Ok` or a clean `Error` — never panic, and never report an
//! internal error (exit code 1), and never take more than a few seconds.
//!
//! Each input is replayed twice: from the file on disk, and from memory (the same bytes in a
//! `MemFs` namespace), so readers that read byte sources are held to the same contract there.
//!
//! `OPENREADOUT_REPLAY_DIR=<dir>` replays every file under `<dir>` as an extra `file-`
//! fixture (triage of a fuzz run: copy the artifacts there with the target's extension).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use openreadout_core::Error;
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex, SNIFF_LEN};
use openreadout_core::source::{Fs, Input, MemFs, MemSource};

#[path = "../src/registry.rs"]
mod registry;

/// A small malformed file must be handled in well under this (debug build). A declared
/// count that drives a loop (2^32 frames in a 4 KB file) took minutes before it was capped.
const SLOW_SECS: u64 = 20;

/// Separator between the files of a multi-file fuzz input (`fuzz/src/lib.rs`).
const BUNDLE_SEP: &[u8] = b"\n=====FUZZ-NEXT-FILE=====\n";

/// Multi-file fuzz targets: the directory opened when no part is (`""`: the scratch
/// directory), the file names of the parts, and which part is opened. Must match the
/// `whole_bundle` calls in `fuzz/fuzz_targets/`.
type Bundle = (
    &'static str,
    &'static str,
    &'static [&'static str],
    Option<usize>,
);
const BUNDLES: &[Bundle] = &[
    (
        "whole_spikeglx",
        "",
        &["s.imec0.ap.meta", "s.imec0.ap.bin"],
        Some(1),
    ),
    (
        "whole_bruker",
        "",
        &[
            "acqus",
            "fid",
            "pdata/1/procs",
            "pdata/1/1r",
            "acqu2s",
            "ser",
        ],
        None,
    ),
    ("whole_ser", "", &["s_1.ser", "s.emi"], Some(0)),
    ("whole_imzml", "", &["s.imzML", "s.ibd"], Some(0)),
    (
        "whole_tims",
        "",
        &["s.d/analysis.tdf", "s.d/analysis.tdf_bin"],
        Some(0),
    ),
    (
        "whole_vsi",
        "",
        &["s.vsi", "_s_/stack1/frame_t_0.ets"],
        Some(0),
    ),
    (
        "whole_zarr",
        "s.zarr",
        &[
            "s.zarr/.zgroup",
            "s.zarr/.zattrs",
            "s.zarr/0/.zarray",
            "s.zarr/0/0.0.0.0.0",
            "s.zarr/zarr.json",
            "s.zarr/0/zarr.json",
            "s.zarr/0/c/0/0/0/0/0",
        ],
        None,
    ),
    (
        "whole_chemstation",
        "s.D",
        &["s.D/dad1A.ch", "s.D/dad1.uv", "s.D/MSD1.MS"],
        None,
    ),
    (
        "whole_waters",
        "s.raw",
        &[
            "s.raw/_HEADER.TXT",
            "s.raw/_extern.inf",
            "s.raw/_FUNCTNS.INF",
            "s.raw/_FUNC001.IDX",
            "s.raw/_FUNC001.DAT",
            "s.raw/_CHROMS.INF",
            "s.raw/_CHRO001.DAT",
        ],
        None,
    ),
    ("whole_sciex", "", &["s.wiff", "s.wiff.scan"], Some(0)),
    (
        "whole_oif",
        "",
        &[
            "50x.oif",
            "50x.oif.files/s_C001.pty",
            "50x.oif.files/s_C001.tif",
        ],
        Some(0),
    ),
    (
        "whole_masshunter",
        "s.d",
        &[
            "s.d/AcqData/MSScan.xsd",
            "s.d/AcqData/MSScan.bin",
            "s.d/AcqData/MSPeak.bin",
            "s.d/AcqData/MSProfile.bin",
            "s.d/AcqData/MSMassCal.bin",
            "s.d/AcqData/DefaultMassCal.xml",
            "s.d/AcqData/MSTS.xml",
            "s.d/AcqData/Contents.xml",
            "s.d/AcqData/Devices.xml",
            "s.d/AcqData/sample_info.xml",
            "s.d/AcqData/TCC1.cd",
            "s.d/AcqData/TCC1.cg",
        ],
        None,
    ),
];

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every fixture under `crates/*/tests/fixtures/malformed/` whose name starts with `prefix`.
fn fixtures(prefix: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for krate in std::fs::read_dir(workspace().join("crates")).unwrap() {
        let dir = krate.unwrap().path().join("tests/fixtures/malformed");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        v.extend(rd.map(|e| e.unwrap().path()).filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        }));
    }
    v.sort();
    v
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The sequence the fuzz harness runs: every read-only `Dataset` operation the CLI and MCP
/// server expose. Returns the errors.
fn exercise(reader: &dyn FormatReader, input: &Input, head: &[u8]) -> Vec<Error> {
    let head = &head[..head.len().min(SNIFF_LEN)];
    let opened = if input.is_local() {
        let _ = reader.sniff(head, input.path());
        reader.open(input.path())
    } else {
        let _ = reader.sniff_input(head, input);
        reader.open_input(input)
    };
    let mut ds: Box<dyn Dataset> = match opened {
        Ok(ds) => ds,
        Err(e) => return vec![e],
    };
    let mut errs = Vec::new();
    if let Ok(info) = ds.info() {
        let _ = serde_json::to_string(&info);
    }
    errs.extend(ds.vendor_metadata().err());
    let _ = ds.provenance();
    errs.extend(ds.entries().err());
    errs.extend(ds.check().err());
    match ds.read_plane(0, PlaneIndex::default()) {
        Ok(plane) => {
            let acc = openreadout_core::stats::Accumulator::from_plane(&plane);
            let _ = acc.finish(64, openreadout_core::stats::HistogramScale::Linear);
            let _ = acc.finish(64, openreadout_core::stats::HistogramScale::Log);
        }
        Err(e) => errs.push(e),
    }
    errs.extend(ds.read_table(0, 0, 64).err());
    errs.extend(ds.read_trace(0, 0, 0, 4096).err());
    errs.extend(ds.read_spectrum(0, 0).err());
    if ds.attachments().is_ok_and(|a| !a.is_empty()) {
        errs.extend(ds.read_attachment(0).err());
    }
    errs.extend(ds.frames(0, Some(16)).err());
    errs
}

/// Run `exercise` under `catch_unwind`; describe any panic or internal error.
fn replay(reader: &dyn FormatReader, input: &Input, head: &[u8], label: &str) -> Vec<String> {
    let id = reader.descriptor().id;
    let label = if input.is_local() {
        label.to_string()
    } else {
        format!("{label} (from memory)")
    };
    let start = Instant::now();
    let r = catch_unwind(AssertUnwindSafe(|| exercise(reader, input, head)));
    let secs = start.elapsed().as_secs();
    if secs > SLOW_SECS {
        return vec![format!(
            "{label} [{id}]: took {secs} s (a declared count drives a loop?)"
        )];
    }
    match r {
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
            vec![format!("{label} [{id}]: panicked: {msg}")]
        }
        Ok(errs) => errs
            .into_iter()
            .filter(|e| e.exit_code() == 1)
            .map(|e| format!("{label} [{id}]: internal error {} ({e})", e.code()))
            .collect(),
    }
}

#[test]
fn file_fixtures_fail_cleanly_in_every_reader() {
    let mut files = fixtures("file-");
    assert!(!files.is_empty(), "no fixtures found");
    if let Some(extra) = std::env::var_os("OPENREADOUT_REPLAY_DIR") {
        walk(Path::new(&extra), &mut files);
    }
    let reg = registry::registry();
    let readers: Vec<&dyn FormatReader> = reg
        .descriptors()
        .iter()
        .filter_map(|d| reg.by_id(&d.id))
        .collect();
    let mut failures = Vec::new();
    for p in &files {
        let data = std::fs::read(p).unwrap();
        let head = &data[..data.len().min(SNIFF_LEN)];
        let mem = Input::from_bytes(p.file_name().unwrap(), data.clone());
        for r in &readers {
            let label = p.display().to_string();
            failures.extend(replay(*r, &Input::local(p), head, &label));
            failures.extend(replay(*r, &mem, head, &label));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixtures misbehaved:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}

#[test]
fn bundle_fixtures_fail_cleanly() {
    let reg = registry::registry();
    let mut failures = Vec::new();
    for p in fixtures("bundle-") {
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        let (_, root, names, open) = BUNDLES
            .iter()
            .find(|(t, _, _, _)| name.starts_with(&format!("bundle-{t}-")))
            .unwrap_or_else(|| panic!("{name}: no bundle layout for this target"));
        let data = std::fs::read(&p).unwrap();
        let mut parts: Vec<&[u8]> = Vec::new();
        let mut rest = &data[..];
        while let Some(i) = rest.windows(BUNDLE_SEP.len()).position(|w| w == BUNDLE_SEP) {
            parts.push(&rest[..i]);
            rest = &rest[i + BUNDLE_SEP.len()..];
        }
        parts.push(rest);
        parts.resize(names.len(), &[]);
        let dir =
            std::env::temp_dir().join(format!("openreadout-bundle-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut mem = MemFs::new();
        for (n, part) in names.iter().zip(&parts) {
            let f = dir.join(n);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, part).unwrap();
            mem.insert(n, Arc::new(MemSource::new(*n, part.to_vec())));
        }
        let (path, head) = open.map_or_else(
            || (dir.join(root), &[][..]),
            |i| (dir.join(names[i]), parts[i]),
        );
        let mem_path: &str = open.map_or(root, |i| names[i]);
        let mem = Input::new(mem_path, Fs::new(Arc::new(mem)));
        for r in reg.descriptors().iter().filter_map(|d| reg.by_id(&d.id)) {
            failures.extend(replay(r, &Input::local(&path), head, &name));
            failures.extend(replay(r, &mem, head, &name));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

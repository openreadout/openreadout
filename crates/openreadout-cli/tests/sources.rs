//! Byte sources: every committed fixture read from memory (a `MemFs` mirror of its directory)
//! and through a host callback with a small block cache gives exactly the answers it gives
//! from its path. Readers that do not read byte sources yet must say so with exit 6.

use std::path::{Path, PathBuf};

#[path = "../src/registry.rs"]
mod registry;
#[path = "support/sources.rs"]
mod support;

use openreadout_core::source::Input;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Fixtures that are valid files (not the malformed ones), and which readers must read them
/// from memory.
const FIXTURES: &[&str] = &[
    "crates/openreadout-cli/tests/fixtures/mini.czi",
    "crates/openreadout-cli/tests/fixtures/mini.nd2",
    "crates/openreadout-cli/tests/fixtures/mini.lif",
    "crates/openreadout-lif/tests/fixtures/synthetic-dims.lif",
    "crates/openreadout-lif/tests/fixtures/lof/Single.lof",
    "crates/openreadout-lif/tests/fixtures/xlef/Experiment.xlef",
    "crates/openreadout-lif/tests/fixtures/xlef/Collection/Series002.xlif",
    "crates/openreadout-hdf5/tests/fixtures/synthetic-generic.h5",
    "crates/openreadout-hdf5/tests/fixtures/synthetic-imaris.ims",
    "crates/openreadout-hdf5/tests/fixtures/synthetic-timeseries.nwb",
    "crates/openreadout-hdf5/tests/fixtures/synthetic-ecephys.nwb",
    "crates/openreadout-zarr/tests/fixtures/ngff04-v2-plate.zip",
    "crates/openreadout-zarr/tests/fixtures/ngff05-v3-czyx-sharded.zip",
];

#[test]
fn fixtures_read_the_same_from_memory_and_callbacks() {
    let reg = registry::registry();
    let mut failures = Vec::new();
    let mut read = 0;
    for rel in FIXTURES {
        let path = workspace().join(rel);
        let files = support::unit_files(&path, 64 << 20).expect("fixture directory");
        let mut inputs = vec![
            ("memory", Input::new(&path, support::mirror(&files))),
            (
                "directory callbacks",
                Input::new(&path, support::callback_mirror(&files)),
            ),
        ];
        // A callback serves one file: only for data sets that are one file.
        let single = reg
            .open(&path)
            .is_ok_and(|(_, ds)| ds.member_files().is_empty());
        if single {
            inputs.push(("callback", support::callback(&path)));
        }
        for (how, input) in inputs {
            match support::compare(&reg, &path, &input) {
                Ok(true) => read += 1,
                Ok(false) => {}
                Err(e) => failures.push(format!("{rel} ({how}):\n    {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        read >= 6,
        "only {read} fixture reads went through byte sources"
    );
}

#[test]
fn a_buffer_is_detected_by_content_and_name() {
    let reg = registry::registry();
    let path = workspace().join("crates/openreadout-cli/tests/fixtures/mini.czi");
    let bytes = std::fs::read(&path).unwrap();
    // The host-given name does not have to exist anywhere.
    let input = Input::from_bytes(Path::new("dropped/mini.czi"), bytes);
    let (det, ds) = reg.open_input(&input).unwrap();
    assert_eq!(det.format_id, "czi");
    let info = ds.info().unwrap();
    assert_eq!(Path::new(&info.path), Path::new("dropped/mini.czi"));
    assert!(!info.images.is_empty());
}

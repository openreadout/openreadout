//! Runs in Node through `wasm-bindgen-test-runner` (`scripts/wasm.sh test`): the committed
//! fixtures read from bytes, through a JS callback and lazily, with the same answers.
#![cfg(target_arch = "wasm32")]

use openreadout_wasm::{Files, InstrumentFile, formats, version};
use serde_json::Value;
use wasm_bindgen_test::wasm_bindgen_test;

const CZI: &[u8] = include_bytes!("../../openreadout-cli/tests/fixtures/mini.czi");
const ND2: &[u8] = include_bytes!("../../openreadout-cli/tests/fixtures/mini.nd2");
const LIF: &[u8] = include_bytes!("../../openreadout-cli/tests/fixtures/mini.lif");
const IMS: &[u8] = include_bytes!("../../openreadout-hdf5/tests/fixtures/synthetic-imaris.ims");
const NWB: &[u8] = include_bytes!("../../openreadout-hdf5/tests/fixtures/synthetic-timeseries.nwb");

fn parse(s: &str) -> Value {
    serde_json::from_str(s).expect("envelope is JSON")
}

fn data(s: &str) -> Value {
    let v = parse(s);
    assert_eq!(v["ok"], Value::Bool(true), "{s}");
    v["data"].clone()
}

#[wasm_bindgen_test]
fn formats_and_version() {
    let f = data(&formats());
    let ids: Vec<&str> = f["formats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"czi") && ids.contains(&"nd2") && ids.contains(&"lif"));
    assert!(ids.contains(&"atf") && ids.contains(&"plexon"));
    assert!(!version().is_empty());
}

#[wasm_bindgen_test]
fn every_command_on_bytes() {
    for (name, bytes, format) in [
        ("mini.czi", CZI, "czi"),
        ("mini.nd2", ND2, "nd2"),
        ("mini.lif", LIF, "lif"),
        ("synthetic-imaris.ims", IMS, "ims"),
    ] {
        let mut f = InstrumentFile::from_bytes(name, bytes.to_vec());
        assert_eq!(
            data(&f.info(Some("format".into()), None))["format"],
            format,
            "{name}"
        );
        let info = data(&f.info(None, None));
        assert_eq!(info["path"], name);
        assert!(
            info["images"].as_array().is_some_and(|a| !a.is_empty()),
            "{name}"
        );
        let dump = data(&f.info(Some("full".into()), None));
        assert!(
            dump["vendor"].is_object() || dump["vendor"].is_array(),
            "{name}"
        );
        let explain = data(&f.info(
            Some("explain".into()),
            Some(r#"{"ask": "what was measured?"}"#.into()),
        ));
        assert!(
            explain["summary"].as_str().is_some_and(|s| !s.is_empty()),
            "{name}"
        );
        assert!(data(&f.check(false))["ok"].is_boolean(), "{name}");
        assert!(
            data(&f.info(Some("structure".into()), None))["entries"].is_array(),
            "{name}"
        );
        let bad = parse(&f.info(Some("dump".into()), None));
        assert_eq!(bad["error"]["exit_code"], 2, "{name}");
        let p = data(&f.preview(Some(r#"{"max_size": 64}"#.into())));
        assert_eq!(p["encoding"], "png", "{name}");
        let png = f.image();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{name}");
        assert_eq!(png.len() as u64, p["bytes"].as_u64().unwrap(), "{name}");
    }
}

#[wasm_bindgen_test]
fn traces_from_bytes() {
    let mut f = InstrumentFile::from_bytes("synthetic-timeseries.nwb", NWB.to_vec());
    let info = data(&f.info(None, None));
    assert!(!info["traces"].as_array().unwrap().is_empty());
    let t = data(&f.trace(0, 0, 0.0, -1.0, 16.0));
    assert!(t["channels"].is_array(), "{t}");
    let err = parse(&f.table(7, 0.0, 10.0));
    assert_eq!(err["ok"], Value::Bool(false));
    assert!(err["error"]["exit_code"].as_i64().is_some());
    // not a mass-spectrometry file; unknown options are usage errors
    assert_eq!(parse(&f.scans(None))["ok"], Value::Bool(false));
    let bad = parse(&f.scans(Some(r#"{"nope": 1}"#.into())));
    assert_eq!(bad["error"]["exit_code"], 2);
}

#[wasm_bindgen_test]
fn a_js_callback_reads_only_what_is_asked() {
    let bytes = js_sys::Uint8Array::from(CZI);
    let calls = js_sys::Array::new();
    let read = js_sys::Function::new_with_args(
        "offset, length",
        "this.calls.push([offset, length]); return this.bytes.subarray(offset, offset + length);",
    );
    let ctx = js_sys::Object::new();
    js_sys::Reflect::set(&ctx, &"bytes".into(), &bytes).unwrap();
    js_sys::Reflect::set(&ctx, &"calls".into(), &calls).unwrap();
    let bound = read.bind0(&ctx);
    #[allow(clippy::cast_precision_loss)]
    let mut f = InstrumentFile::from_callback("mini.czi", CZI.len() as f64, bound).unwrap();
    let mut from_bytes = InstrumentFile::from_bytes("mini.czi", CZI.to_vec());
    assert_eq!(
        parse(&f.info(None, None)),
        parse(&from_bytes.info(None, None))
    );
    assert!(calls.length() > 0);
}

#[wasm_bindgen_test]
fn lazy_files_ask_for_the_blocks_they_need() {
    #[allow(clippy::cast_precision_loss)]
    let mut f = InstrumentFile::lazy("mini.lif", LIF.len() as f64).unwrap();
    let mut rounds = 0;
    let out = loop {
        let out = f.info(None, None);
        let Some(p) = f.pending() else {
            break out;
        };
        rounds += 1;
        assert!(rounds < 64, "no progress");
        let (at, len) = (p[0] as usize, p[1] as usize);
        f.provide(p[0], &LIF[at..at + len]).unwrap();
    };
    assert!(rounds >= 1);
    let mut whole = InstrumentFile::from_bytes("mini.lif", LIF.to_vec());
    assert_eq!(parse(&out), parse(&whole.info(None, None)));
}

#[wasm_bindgen_test]
fn dropped_folders_resolve_siblings() {
    let mut files = Files::new();
    files.add("drop/mini.nd2", ND2.to_vec());
    files.add("drop/notes.txt", b"not data".to_vec());
    assert_eq!(files.paths(), vec!["drop/mini.nd2", "drop/notes.txt"]);
    let mut f = files.open("drop/mini.nd2");
    assert_eq!(data(&f.info(None, None))["format"]["id"], "nd2");
}

#[wasm_bindgen_test]
fn garbage_is_a_clean_error() {
    let mut f = InstrumentFile::from_bytes("x.bin", b"\0\x01 nothing here".to_vec());
    let v = parse(&f.info(None, None));
    assert_eq!(v["ok"], Value::Bool(false));
    assert_eq!(v["error"]["code"], "unknown_format");
    let mut e = InstrumentFile::from_bytes("x.czi", Vec::new());
    assert_eq!(parse(&e.info(None, None))["error"]["exit_code"], 4);
}

#[wasm_bindgen_test]
fn nwb_ecephys_tables_from_bytes() {
    let bytes = include_bytes!("../../openreadout-hdf5/tests/fixtures/synthetic-ecephys.nwb");
    let mut f = InstrumentFile::from_bytes("ecephys.nwb", bytes.to_vec());
    let info = data(&f.info(None, None));
    for table in info["tables"].as_array().unwrap() {
        let i = u32::try_from(table["index"].as_u64().unwrap()).unwrap();
        data(&f.table(i, 0.0, 10.0));
    }
    for trace in info["traces"].as_array().unwrap() {
        let i = u32::try_from(trace["index"].as_u64().unwrap()).unwrap();
        data(&f.trace(i, 0, 0.0, -1.0, 16.0));
    }
}

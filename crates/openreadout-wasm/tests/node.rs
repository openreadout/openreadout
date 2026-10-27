//! Runs in Node through `wasm-bindgen-test-runner` (`scripts/wasm.sh test`): the committed
//! fixtures read from bytes, through a JS callback and lazily, with the same answers.
#![cfg(target_arch = "wasm32")]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use openreadout_core::source::MemSource;
use openreadout_core::zip::ZipIndex;
use openreadout_wasm::{Files, InstrumentFile, formats, version};
use serde_json::Value;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

const CZI: &[u8] = include_bytes!("../../openreadout-cli/tests/fixtures/mini.czi");
const ND2: &[u8] = include_bytes!("../../openreadout-cli/tests/fixtures/mini.nd2");
const LIF: &[u8] = include_bytes!("../../openreadout-cli/tests/fixtures/mini.lif");
const IMS: &[u8] = include_bytes!("../../openreadout-hdf5/tests/fixtures/synthetic-imaris.ims");
const NWB: &[u8] = include_bytes!("../../openreadout-hdf5/tests/fixtures/synthetic-timeseries.nwb");
/// A zipped OME-Zarr 0.4 image (Zarr v2, blosc).
const ZARR: &[u8] =
    include_bytes!("../../openreadout-zarr/tests/fixtures/ngff04-v2-tczyx-blosc-lz4.zip");

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
fn preview_annotations_dark_and_svg() {
    let mut f = InstrumentFile::from_bytes("mini.nd2", ND2.to_vec());
    let p = data(
        &f.preview(Some(
            r#"{"max_size": 200, "axes": "rulers", "dark": true, "format": "svg",
            "annotations": [{"type": "point", "x": 4, "y": 4, "label": "here"}]}"#
                .into(),
        )),
    );
    assert_eq!(p["encoding"], "svg");
    assert_eq!(p["annotations"].as_array().map(Vec::len), Some(1));
    assert!(String::from_utf8_lossy(&f.image()).contains("<svg"));
    let bad = parse(&f.preview(Some(r#"{"colour": "red"}"#.into())));
    assert_eq!(bad["error"]["exit_code"], 2);
    let bad = parse(&f.preview(Some(r#"{"format": "gif"}"#.into())));
    assert_eq!(bad["error"]["exit_code"], 2);
}

#[wasm_bindgen_test]
fn preview_scale_draws_more_pixels_at_the_same_layout() {
    let mut f = InstrumentFile::from_bytes("mini.nd2", ND2.to_vec());
    let one = data(&f.preview(Some(r#"{"max_size": 100, "axes": "rulers"}"#.into())));
    let two = data(&f.preview(Some(
        r#"{"max_size": 100, "axes": "rulers", "scale": 2}"#.into(),
    )));
    assert_eq!(two["width"], one["width"].as_u64().unwrap() * 2);
    assert_eq!(two["image"]["plot_area"], one["image"]["plot_area"]);
    let bad = parse(&f.preview(Some(r#"{"scale": 9}"#.into())));
    assert_eq!(bad["error"]["exit_code"], 2);
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

/// The files of the zipped OME-Zarr fixture, under the folder `root`.
fn zarr_folder(root: &str) -> BTreeMap<String, Vec<u8>> {
    let src = Arc::new(MemSource::new("image.zip", ZARR));
    let zip = ZipIndex::from_source(src, Path::new("image.zip"), "test").unwrap();
    zip.members
        .iter()
        .map(|m| (format!("{root}/{}", m.name), zip.read(m).unwrap()))
        .collect()
}

fn field(o: &js_sys::Object, key: &str) -> JsValue {
    js_sys::Reflect::get(o, &JsValue::from_str(key)).unwrap()
}

/// Repeat `op` on the lazy file `f`, answering what it asks for from `files`, until it needs
/// nothing more. Returns the last result and the number of rounds.
fn answered(
    f: &mut InstrumentFile,
    files: &BTreeMap<String, Vec<u8>>,
    mut op: impl FnMut(&mut InstrumentFile) -> String,
) -> (String, usize) {
    let mut rounds = 0;
    loop {
        let out = op(f);
        let Some(needs) = f.pending() else {
            return (out, rounds);
        };
        rounds += 1;
        assert!(rounds < 500, "no progress");
        for need in needs {
            let path = field(&need, "path").as_string().unwrap();
            if field(&need, "lookup").is_truthy() {
                // As the npm wrapper does: the answer brings the file's first block along.
                match files.get(&path) {
                    Some(b) => {
                        #[allow(clippy::cast_precision_loss)]
                        f.found(&path, b.len() as f64).unwrap();
                        f.provide(&path, 0.0, &b[..b.len().min(1 << 20)]).unwrap();
                    }
                    None => f.absent(&path),
                }
            } else {
                let at = field(&need, "offset").as_f64().unwrap();
                let len = field(&need, "length").as_f64().unwrap();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let bytes = &files[&path][at as usize..(at + len) as usize];
                f.provide(&path, at, bytes).unwrap();
            }
        }
    }
}

#[wasm_bindgen_test]
fn lazy_files_ask_for_the_blocks_they_need() {
    let files = BTreeMap::from([("mini.lif".to_string(), LIF.to_vec())]);
    #[allow(clippy::cast_precision_loss)]
    let mut f = InstrumentFile::lazy("mini.lif", LIF.len() as f64).unwrap();
    let (out, rounds) = answered(&mut f, &files, |f| f.info(None, None));
    assert!(rounds >= 1);
    let mut whole = InstrumentFile::from_bytes("mini.lif", LIF.to_vec());
    assert_eq!(parse(&out), parse(&whole.info(None, None)));
}

#[wasm_bindgen_test]
fn ome_zarr_reads_from_a_zip_store() {
    let mut f = InstrumentFile::from_bytes("image.ome.zarr.zip", ZARR.to_vec());
    let info = data(&f.info(None, None));
    assert_eq!(info["format"]["id"], "ome-zarr");
    data(&f.preview(None));
    assert!(!f.image().is_empty());
}

/// A dropped `.zarr` folder: every file and its size is known, bytes come as they are needed.
#[wasm_bindgen_test]
fn ome_zarr_folders_read_lazily() {
    let files = zarr_folder("image.zarr");
    #[allow(clippy::cast_precision_loss)]
    let sizes = files.values().map(|b| b.len() as f64).collect();
    let mut f =
        InstrumentFile::lazy_files(files.keys().cloned().collect(), sizes, "image.zarr").unwrap();
    let (info, _) = answered(&mut f, &files, |f| f.info(None, None));
    let mut zip = InstrumentFile::from_bytes("image.ome.zarr.zip", ZARR.to_vec());
    assert_eq!(data(&info)["images"], data(&zip.info(None, None))["images"]);
    answered(&mut f, &files, |f| f.preview(None));
    data(&zip.preview(None));
    assert_eq!(f.image(), zip.image());
}

/// A store read over HTTP: nothing is known up front, each file is looked up when a reader
/// asks for it, and absent files (a missing chunk, a `.zattrs` Zarr v3 does not have) are
/// answered as such.
#[wasm_bindgen_test]
fn ome_zarr_stores_look_files_up_one_by_one() {
    let files = zarr_folder("6001240.zarr");
    let mut f = InstrumentFile::lazy_store("6001240.zarr", None).unwrap();
    let (info, rounds) = answered(&mut f, &files, |f| f.info(None, None));
    // One round for the files the readers check for while detecting the format, then about
    // one per Zarr node (the image, its labels and each pyramid level).
    assert!(rounds <= 8, "{rounds} rounds");
    let mut zip = InstrumentFile::from_bytes("image.ome.zarr.zip", ZARR.to_vec());
    assert_eq!(data(&info)["images"], data(&zip.info(None, None))["images"]);
    answered(&mut f, &files, |f| f.preview(None));
    data(&zip.preview(None));
    assert_eq!(f.image(), zip.image());
}

/// A file opened from a store read over HTTP finds its companion files the same way: the
/// descriptor of a Bruker EPR spectrum (`.DSC`) is looked up, then its data file (`.DTA`).
#[wasm_bindgen_test]
fn files_in_stores_look_up_their_companions() {
    let dsc = "#DESC\t1.2\nDSRC\tEXP\nBSEQ\tBIG\nIKKF\tREAL\nXTYP\tIDX\nYTYP\tNODATA\nZTYP\tNODATA\nIRFMT\tD\nXPTS\t3\nXMIN\t3300.0\nXWID\t100.0\nTITL\t'test'\nXNAM\t'Field'\nXUNI\t'G'\n";
    let dta: Vec<u8> = [1.5f64, -2.0, 4.25]
        .iter()
        .flat_map(|x| x.to_be_bytes())
        .collect();
    let files = BTreeMap::from([
        ("files/a.DSC".to_string(), dsc.as_bytes().to_vec()),
        ("files/a.DTA".to_string(), dta),
    ]);
    assert!(InstrumentFile::lazy_store("files", Some("elsewhere/a.DSC".into())).is_err());
    let mut f = InstrumentFile::lazy_store("files", Some("files/a.DSC".into())).unwrap();
    let (info, _) = answered(&mut f, &files, |f| f.info(None, None));
    assert_eq!(data(&info)["format"]["id"], "bruker-bes3t");
    let (trace, _) = answered(&mut f, &files, |f| f.trace(0, 0, 0.0, -1.0, 10.0));
    let mut held = Files::new();
    for (p, b) in &files {
        held.add(p, b.clone());
    }
    let mut whole = held.open("files/a.DSC");
    assert_eq!(data(&trace), data(&whole.trace(0, 0, 0.0, -1.0, 10.0)));
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

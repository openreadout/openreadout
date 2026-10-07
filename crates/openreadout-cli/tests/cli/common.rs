//! Helpers shared by the CLI test modules.

use std::path::PathBuf;
use std::process::Command;

/// The CLI under test. The live window is 0 here: these tests write damaged copies moments
/// before reading them, which must be judged as damaged, not as acquisitions still in progress
/// (live behaviour is tested in `tests/watch.rs`; see `book/src/guides/lab-shares.md`).
pub fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_openreadout"));
    c.env("OPENREADOUT_LIVE_WINDOW", "0");
    c
}

pub fn corpus(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    );
    let p = dir.join(name);
    p.exists().then_some(p)
}

pub fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// Truncate a corpus file to 60% and expect `check` to report corruption with exit 4.
pub fn truncated_copy(src: &PathBuf) -> PathBuf {
    let data = std::fs::read(src).unwrap();
    let dir = std::env::temp_dir().join(format!("openreadout-trunc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join(src.file_name().unwrap());
    std::fs::write(&dst, &data[..data.len() * 6 / 10]).unwrap();
    dst
}

pub fn read_json(p: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

/// A minimal FCS 3.1 file: 4 events × 2 parameters, 16-bit little-endian integers.
pub fn tiny_fcs(dir: &std::path::Path) -> PathBuf {
    let kws = [
        ("$BYTEORD", "1,2,3,4"),
        ("$DATATYPE", "I"),
        ("$MODE", "L"),
        ("$PAR", "2"),
        ("$TOT", "4"),
        ("$NEXTDATA", "0"),
        ("$BEGINANALYSIS", "0"),
        ("$ENDANALYSIS", "0"),
        ("$BEGINSTEXT", "0"),
        ("$ENDSTEXT", "0"),
        ("$P1N", "FSC-A"),
        ("$P1S", "Forward, area"),
        ("$P1B", "16"),
        ("$P1R", "1024"),
        ("$P1E", "0,0"),
        ("$P2N", "SSC-A"),
        ("$P2B", "16"),
        ("$P2R", "65536"),
        ("$P2E", "0,0"),
    ];
    let mut text = String::from("/");
    for (k, v) in kws {
        text.push_str(&format!("{k}/{v}/"));
    }
    let text_len = text.len() + "$BEGINDATA/0000000000/".len() + "$ENDDATA/0000000000/".len();
    let data_begin = 58 + text_len;
    let data: Vec<u8> = [1u16, 2, 1023, 65535, 0xFC00 | 7, 9, 100, 200]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let data_end = data_begin + data.len() - 1;
    text.push_str(&format!(
        "$BEGINDATA/{data_begin:010}/$ENDDATA/{data_end:010}/"
    ));
    assert_eq!(text.len(), text_len);
    let mut out = format!(
        "FCS3.1    {:>8}{:>8}{data_begin:>8}{data_end:>8}{:>8}{:>8}",
        58,
        57 + text_len,
        0,
        0
    )
    .into_bytes();
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(&data);
    let p = dir.join("tiny.fcs");
    std::fs::write(&p, &out).unwrap();
    p
}

pub fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("openreadout-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A Bruker experiment directory: TD 8 little-endian int32 complex, NC -1; pdata/1 SI 4.
pub fn tiny_bruker(dir: &std::path::Path) -> PathBuf {
    let exp = dir.join("study").join("7");
    std::fs::create_dir_all(exp.join("pdata/1")).unwrap();
    let params = |pairs: &[(&str, &str)]| {
        let mut s = String::from("##TITLE= Parameter file, TopSpin 3.6.2\n##JCAMPDX= 5.0\n");
        for (k, v) in pairs {
            s.push_str(&format!("##${k}= {v}\n"));
        }
        s + "##END=\n"
    };
    std::fs::write(
        exp.join("acqus"),
        params(&[
            ("TD", "8"),
            ("AQ_mod", "3"),
            ("NC", "-1"),
            ("SW_h", "2000"),
            ("NUC1", "<13C>"),
            ("SFO1", "100.6"),
        ]),
    )
    .unwrap();
    let fid: Vec<u8> = [2i32, -2, 4, 6, 8, 10, 12, 14]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    std::fs::write(exp.join("fid"), fid).unwrap();
    std::fs::write(
        exp.join("pdata/1/procs"),
        params(&[
            ("SI", "4"),
            ("NC_proc", "0"),
            ("OFFSET", "200"),
            ("SW_p", "400"),
            ("SF", "100"),
        ]),
    )
    .unwrap();
    let r: Vec<u8> = [5i32, 6, 7, 8]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    std::fs::write(exp.join("pdata/1/1r"), r).unwrap();
    exp
}

/// Minimal complete files (8x8 CZI, 8x8x2-frame ND2, 8x6x2 LIF) synthesized by `fuzz/seeds.py`.
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn run(args: &[&str]) -> std::process::Output {
    bin().args(args).output().unwrap()
}

/// A directory whose name has spaces and non-ASCII characters, holding `doctor.tif` and
/// `doctor.fcs`.
pub fn ux_fixtures(root: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
    let dir = root.join("données ü").join("mit Leerzeichen");
    let out = bin()
        .args(["self", "doctor", "--write-fixtures"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let (t, f) = (dir.join("doctor.tif"), dir.join("doctor.fcs"));
    assert!(t.exists() && f.exists());
    (dir, t, f)
}

pub fn jsonl(out: &std::process::Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}

pub fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Plane hashes reported by `planes --json` for `path` (image, c, z, t, xxh3).
pub fn plane_hashes(path: &std::path::Path) -> Vec<(u64, u64, u64, u64, String)> {
    let out = bin()
        .args(["planes", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    json(&out)["data"]["planes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let n = |k: &str| p[k].as_u64().unwrap();
            (
                n("image"),
                n("c"),
                n("z"),
                n("t"),
                p["xxh3"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

//! Installation-level behaviour: formats, schemas, exit codes, the skill, MCP config, doctor,
//! completions and man pages, the panic handler, stdin, threads, progress and output pipes.

use crate::common::*;

#[test]
fn formats_lists_all_readers() {
    let out = bin().args(["self", "formats", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["ok"], true);
    assert_eq!(v["schema_version"], "2");
    let ids: Vec<&str> = v["data"]["formats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "czi",
            "lif",
            "nd2",
            "thermo-raw",
            "fcs",
            "imzml",
            "mzml",
            "mzxml",
            "mzmlb",
            "bruker-tdf",
            "oir",
            "vsi",
            "mirax",
            "zvi",
            "oib",
            "oif",
            "dcimg",
            "rdml",
            "applied-biosystems-eds",
            "bio-rad-pcrd",
            "rotor-gene-rex",
            "roche-lightcycler-ixo",
            "abf",
            "atf",
            "neuralynx",
            "blackrock",
            "spikeglx",
            "intan",
            "plexon",
            "heka-patchmaster",
            "ced-spike2",
            "winwcp",
            "open-ephys",
            "agilent-masshunter",
            "sciex-wiff",
            "openlab-cds",
            "chemstation",
            "andi-chrom",
            "waters-raw",
            "shimadzu",
            "chromeleon",
            "empower-arw",
            "opera-harmony",
            "imagexpress",
            "cellvoyager",
            "tiff",
            "ome-zarr",
            "mrc",
            "dm",
            "ser",
            "emd",
            "ims",
            "nwb",
            "bruker-nmr",
            "jcamp-dx",
            "varian-nmr",
            "jeol-jdf",
            "magritek-spinsolve",
            "bruker-opus",
            "thermo-omnic",
            "renishaw-wdf",
            "perkinelmer-sp",
            "jasco-jws",
            "galactic-spc",
            "witec-project",
            "agilent-fpa",
            "perkinelmer-fsm",
            "agilent-cary",
            "microcal-itc",
            "cytiva-biacore-blr",
            "cytiva-biacore-bme",
            "agilent-seahorse-asyr",
            "sartorius-octet-frd",
            "malvern-zetasizer-dts",
            "genepix-gpr",
            "biorad-scn",
            "cytiva-unicorn-res",
            "cytiva-unicorn-zip",
            "bruker-bes3t",
            "bruker-esp",
            "panalytical-xrdml",
            "bruker-raw",
            "bruker-brml",
            "rigaku-ras",
            "rigaku-rasx",
            "biologic-mpr",
            "biologic-mpt",
            "gamry-dta",
            "neware-nda",
            "neware-ndax",
            "arbin-res",
            "netzsch-ngb",
            "ta-universal-analysis",
            "ta-trios",
            "hdf5",
            "plate",
        ]
    );
}

#[test]
fn schema_is_valid_json_schema() {
    for name in [
        "info",
        "info-full",
        "info-explain",
        "info-structure",
        "info-format",
        "check",
        "planes",
        "compare",
        "export",
        "extract",
        "trace",
        "scans",
        "spectrum",
        "formats",
        "stats",
        "doctor",
        "envelope",
    ] {
        let out = bin().args(["self", "schema", name]).output().unwrap();
        assert!(out.status.success(), "schema {name}");
        let v = json(&out);
        assert!(
            v.get("$schema").is_some() || v.get("title").is_some(),
            "schema {name} lacks $schema/title"
        );
    }
}

#[test]
fn unknown_format_exits_3_with_envelope() {
    let dir = std::env::temp_dir().join(format!("openreadout-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("notes.txt");
    std::fs::write(&p, b"this is not an instrument file\n").unwrap();
    let out = bin()
        .args(["info", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let v = json(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "unknown_format");
    assert_eq!(v["error"]["exit_code"], 3);
    assert!(v["error"]["hint"].as_str().unwrap().contains("formats"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_file_exits_5() {
    let out = bin()
        .args(["info", "/nonexistent/definitely-missing.czi"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
}

#[test]
fn usage_error_exits_2() {
    let out = bin()
        .args(["export", "x.czi", "--select", "q=1"])
        .output()
        .unwrap();
    // clap parses fine; the selection parser rejects axis q, but only after opening the file (exit 5 here).
    assert!(matches!(out.status.code(), Some(2 | 5)));
    let out = bin().args(["nonsense-command"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn skill_prints_frontmatter() {
    let out = bin().args(["self", "skill"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with("---\nname: openreadout\n"));
}

#[test]
fn mcp_config_snippets() {
    for client in ["claude", "cursor", "codex", "vscode", "claude-desktop"] {
        let out = bin().args(["mcp", "--config", client]).output().unwrap();
        assert!(out.status.success(), "{client}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("openreadout"));
    }
}

#[test]
fn doctor_reports_build_and_passes_self_test() {
    let out = bin().args(["self", "doctor", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let v = json(&out);
    let d = &v["data"];
    assert_eq!(d["ok"], true);
    assert_eq!(d["version"], env!("CARGO_PKG_VERSION"));
    assert!(!d["target"].as_str().unwrap().is_empty());
    let formats: Vec<&str> = d["formats"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(formats.contains(&"czi") && formats.contains(&"tiff") && formats.contains(&"fcs"));
    let checks = d["checks"].as_array().unwrap();
    assert!(checks.len() >= 5);
    assert!(checks.iter().all(|c| c["ok"] == true), "{checks:?}");
    if d["mcp"] == true {
        let tools = d["mcp_tools"].to_string();
        for t in ["openreadout_info", "openreadout_stats", "openreadout_check"] {
            assert!(tools.contains(t), "{t}");
        }
    }
    let out = bin()
        .args(["--color", "never", "self", "doctor"])
        .output()
        .unwrap();
    assert!(stdout(&out).contains("all checks passed"));
}

#[test]
fn completions_and_man_pages() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let out = bin().args(["self", "completions", shell]).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{shell}");
        let s = stdout(&out);
        assert!(s.contains("openreadout") && s.contains("stats"), "{shell}");
    }
    let out = bin()
        .args(["self", "completions", "tcsh"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("man");
    let out = bin()
        .args(["self", "man", "--out"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    for page in [
        "openreadout.1",
        "openreadout-info.1",
        "openreadout-stats.1",
        "openreadout-check.1",
        "openreadout-self-doctor.1",
    ] {
        let text = std::fs::read_to_string(dir.join(page)).unwrap();
        assert!(text.starts_with(".ie") || text.contains(".TH"), "{page}");
    }
}

#[test]
fn schemas_of_new_commands() {
    for name in ["stats", "compare", "doctor"] {
        let out = bin().args(["self", "schema", name]).output().unwrap();
        assert!(out.status.success(), "schema {name}");
        let v = json(&out);
        assert!(v.get("title").is_some(), "{name}");
    }
}

#[test]
fn panic_handler_exit_1_envelope_and_no_backtrace() {
    let out = bin()
        .args(["--self-panic", "info", "--json", "x"])
        .env_remove("RUST_BACKTRACE")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "internal_panic");
    assert_eq!(v["error"]["exit_code"], 1);
    assert!(v["error"]["hint"].as_str().unwrap().contains("issues"));
    let err = stderr(&out);
    assert!(
        err.contains("internal error") && err.contains("hint:"),
        "{err}"
    );
    assert!(
        !err.contains("stack backtrace") && !err.contains("backtrace::"),
        "{err}"
    );
    let out = bin()
        .args(["info", "x"])
        .env("OPENREADOUT_DEBUG_PANIC", "1")
        .env("RUST_BACKTRACE", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "no envelope without --json");
    assert!(
        stderr(&out).lines().count() > 3,
        "backtrace printed with RUST_BACKTRACE"
    );
}

#[test]
fn stdin_is_spooled_and_capped() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, tif, fcs) = ux_fixtures(tmp.path());
    for cmd in [
        &["info", "--view", "format"][..],
        &["info"],
        &["info", "--view", "full"],
        &["check"],
        &["info", "--view", "structure"],
        &["info", "--view", "explain"],
        &["planes"],
    ] {
        let out = bin()
            .args(cmd)
            .args(["--json", "-"])
            .stdin(std::fs::File::open(&tif).unwrap())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{cmd:?}: {}", stderr(&out));
        let v = json(&out);
        let path = v["data"]["path"]
            .as_str()
            .or_else(|| v["data"]["file"]["path"].as_str());
        if !cmd.contains(&"explain") {
            assert_eq!(path, Some("-"), "{cmd:?}");
        }
    }
    let out = bin()
        .args(["info", "--json", "-"])
        .stdin(std::fs::File::open(&fcs).unwrap())
        .output()
        .unwrap();
    assert_eq!(json(&out)["data"]["format"]["id"], "fcs");
    // Over the cap: usage error with a hint about the variable.
    let out = bin()
        .args(["info", "--json", "-"])
        .env("OPENREADOUT_STDIN_MAX_BYTES", "100")
        .stdin(std::fs::File::open(&tif).unwrap())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        json(&out)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("OPENREADOUT_STDIN_MAX_BYTES")
    );
    // Empty standard input is not an instrument file.
    let out = bin()
        .args(["info", "--view", "format", "-"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn threads_give_identical_exports() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, tif, _) = ux_fixtures(tmp.path());
    // A corpus file with many planes when present, the synthetic TIFF otherwise.
    let src = corpus("aics-ND2-dims-p1z5t3c2y32x32.nd2").unwrap_or(tif);
    let mut outputs = Vec::new();
    for (n, to, ext) in [
        ("1", "ome-tiff", "ome.tiff"),
        ("4", "ome-tiff", "ome.tiff"),
        ("1", "ome-zarr", "ome.zarr"),
        ("3", "ome-zarr", "ome.zarr"),
    ] {
        let o = dir.join(format!("t{n}.{ext}"));
        let out = bin()
            .args(["--threads", n, "export", "--format", to, "-o"])
            .arg(&o)
            .arg(&src)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        outputs.push(o);
    }
    assert_eq!(
        std::fs::read(&outputs[0]).unwrap(),
        std::fs::read(&outputs[1]).unwrap(),
        "OME-TIFF bytes differ between 1 and 4 threads"
    );
    let tree = |root: &std::path::Path| {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push((
                        p.strip_prefix(root).unwrap().to_path_buf(),
                        std::fs::read(&p).unwrap(),
                    ));
                }
            }
        }
        files.sort();
        files
    };
    assert_eq!(
        tree(&outputs[2]),
        tree(&outputs[3]),
        "OME-Zarr stores differ"
    );
    // planes hashes are the same whatever the thread count.
    let hashes = |n: &str| {
        let out = bin()
            .args(["--threads", n, "planes", "--json"])
            .arg(&src)
            .output()
            .unwrap();
        json(&out)["data"]["planes"].clone()
    };
    assert_eq!(hashes("1"), hashes("4"));
    let out = bin()
        .args(["--threads", "0", "info"])
        .arg(&src)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn progress_colour_and_quiet() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, tif, _) = ux_fixtures(tmp.path());
    let o = dir.join("p.ome.tiff");
    // Forced progress on a non-terminal stderr: plain lines; stdout unchanged.
    let out = bin()
        .args(["--progress", "export", "--overwrite", "-o"])
        .arg(&o)
        .arg(&tif)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(stderr(&out).contains("planes 2/2"), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("wrote "));
    // Default on a pipe: no progress.
    let out = bin()
        .args(["export", "--overwrite", "-o"])
        .arg(&o)
        .arg(&tif)
        .output()
        .unwrap();
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
    let out = bin()
        .args(["--progress", "--no-progress", "export", "--overwrite", "-o"])
        .arg(&o)
        .arg(&tif)
        .output()
        .unwrap();
    assert!(out.stderr.is_empty());
    // --quiet: nothing on success; --json unaffected.
    let out = bin()
        .args(["-q", "export", "--overwrite", "-o"])
        .arg(&o)
        .arg(&tif)
        .output()
        .unwrap();
    assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty());
    let out = bin()
        .args(["--quiet", "info", "--json"])
        .arg(&tif)
        .output()
        .unwrap();
    assert_eq!(json(&out)["ok"], true);
    // Colour: never on a pipe by default, forced by --color always or CLICOLOR_FORCE, and
    // NO_COLOR wins over CLICOLOR_FORCE. JSON is never coloured.
    let esc = |out: &std::process::Output| out.stdout.contains(&0x1b);
    let ls = |args: &[&str], envs: &[(&str, &str)]| {
        let mut c = bin();
        c.env_remove("NO_COLOR").env_remove("CLICOLOR_FORCE");
        for (k, v) in envs {
            c.env(k, v);
        }
        c.args(args)
            .args(["info", "--view", "structure"])
            .arg(&tif)
            .output()
            .unwrap()
    };
    assert!(!esc(&ls(&[], &[])));
    assert!(esc(&ls(&["--color", "always"], &[])));
    assert!(esc(&ls(&[], &[("CLICOLOR_FORCE", "1")])));
    assert!(!esc(&ls(
        &[],
        &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]
    )));
    assert!(!esc(&ls(&["--color", "never"], &[("CLICOLOR_FORCE", "1")])));
    let out = bin()
        .args(["--color", "always", "info", "--json"])
        .arg(&tif)
        .output()
        .unwrap();
    assert!(!esc(&out));
    // `ls` renders a table with a header.
    let s = stdout(&ls(&["--color", "never"], &[]));
    assert!(
        s.contains("kind") && s.contains("offset") && s.contains("──"),
        "{s}"
    );
}

#[test]
fn closed_stdout_pipe_is_not_reported_as_a_bug() {
    use std::io::Read;
    let tmp = tempfile::tempdir().unwrap();
    let (_, tif, _) = ux_fixtures(tmp.path());
    let mut child = bin()
        .args(["info", "--view", "full", "--json"])
        .arg(&tif)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Close our end of stdout at once.
    drop(child.stdout.take());
    let status = child.wait().unwrap();
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(!err.contains("internal error"), "{err}");
    assert!(matches!(status.code(), Some(0 | 1) | None));
}

#[cfg(windows)]
#[test]
fn windows_long_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let mut dir = tmp.path().to_path_buf();
    while dir.as_os_str().len() < 300 {
        dir.push("a_rather_long_directory_name_for_testing");
    }
    let (_, tif, _) = ux_fixtures(&dir);
    assert!(tif.as_os_str().len() > 260);
    let out = bin().args(["info", "--json"]).arg(&tif).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = bin()
        .args(["info", "--jsonl", "-r"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

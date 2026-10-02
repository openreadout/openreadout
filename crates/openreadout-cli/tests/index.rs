//! `index`, `search`, `health`, `export-dataset`, `dump --redact` and `check --headers-only` on
//! a synthetic share (no corpus). Every name and value in the synthetic files is invented.

use std::path::Path;
use std::process::{Command, Output};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_openreadout"));
    c.env_remove("OPENREADOUT_REDACT_SALT");
    c
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn index_search_health_export_redact() {
    let tmp = tempfile::tempdir().unwrap();
    let share = tmp.path().join("share");
    let idx = tmp.path().join("idx");
    openreadout_index::synth::share(&share).unwrap();

    let out = run(&["index", s(&share), "-o", s(&idx), "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["ok"], true);
    let d = &v["data"];
    assert_eq!(d["complete"], true);
    assert_eq!(d["datasets"], 7);
    assert_eq!(d["formats"]["fcs"]["count"], 4);
    assert_eq!(d["schema_version"], "1");
    assert_eq!(
        d["tables"]["experiments"]["columns"][0]["name"], "path",
        "index.json lists the columns"
    );
    for f in [
        "index.json",
        "experiments.parquet",
        "files.parquet",
        "problems.parquet",
    ] {
        assert!(idx.join(f).exists(), "{f}");
    }

    // Human output.
    let out = run(&["index", s(&share), "-o", s(&idx)]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("7 data sets"), "{text}");
    assert!(text.contains("unchanged"), "{text}");

    // Search: JSON envelope, JSONL, table, sorting, errors.
    let out = run(&["search", s(&idx), "format=fcs sample~A12", "--json"]);
    let v = json(&out);
    assert_eq!(v["data"]["total"], 1);
    assert!(
        v["data"]["results"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("tube_A12.fcs")
    );
    let out = run(&[
        "search",
        s(&idx),
        "format=tiff",
        "--jsonl",
        "--fields",
        "path,size",
        "--sort",
        "-size",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0]["size_bytes"].as_u64() >= lines[2]["size_bytes"].as_u64());
    let out = run(&["search", s(&idx), "acquired<2020"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("tube_A12.fcs") && text.contains("1 of 1 matching"),
        "{text}"
    );
    let out = run(&["search", s(&idx), "colour=red", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(json(&out)["error"]["code"], "usage");
    let out = run(&["search", s(&tmp.path().join("nope")), "x"]);
    assert_eq!(out.status.code(), Some(2));

    // Health.
    let md = tmp.path().join("health.md");
    let out = run(&["search", "--health", s(&idx), "--json", "-o", s(&md)]);
    let v = json(&out);
    assert_eq!(v["data"]["duplicates"]["group_count"], 1);
    assert_eq!(v["data"]["integrity"]["problem_count"], 1);
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(text.contains("## Duplicates") && !text.contains("Jane Roe"));
    let out = run(&["index", s(&share), "-o", s(&idx), "--health"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("# Storage health report"));

    // search --export: redaction needs a salt.
    let out_dir = tmp.path().join("export");
    let out = run(&[
        "search",
        s(&idx),
        "format=fcs -status=truncated",
        "--export",
        s(&out_dir),
        "--redact",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2), "no salt");
    let salt = tmp.path().join("salt");
    std::fs::write(&salt, "a-secret-salt\n").unwrap();
    let out = run(&[
        "search",
        s(&idx),
        "format=fcs -status=truncated",
        "--export",
        s(&out_dir),
        "--redact",
        "--salt-file",
        s(&salt),
        "--json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["datasets"].as_array().unwrap().len(), 3);
    assert_eq!(v["data"]["verified"], true);
    let sheet = std::fs::read_to_string(out_dir.join("datasheet.json")).unwrap();
    assert!(!sheet.contains("Jane Roe") && !sheet.contains("a-secret-salt"));
    assert!(out_dir.join("DATASHEET.md").exists());

    // dump --redact: same replacement as export-dataset for the same salt.
    let fcs = share.join("flow/run1/tube_A12.fcs");
    let out = bin()
        .args(["info", "--view", "full", s(&fcs), "--json", "--redact"])
        .env("OPENREADOUT_REDACT_SALT", "a-secret-salt")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains("Jane Roe") && !text.contains("jroe@example.org"),
        "{text}"
    );
    let op = json(&out)["data"]["file"]["experiment"]["acquisition"]["operator"].clone();
    assert!(op.as_str().unwrap().starts_with("redacted:"));
    assert!(
        sheet.contains(op.as_str().unwrap()),
        "stable across commands"
    );
    let out = run(&["info", "--view", "full", s(&fcs), "--json"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Jane Roe"),
        "no redaction by default"
    );

    // check --headers-only.
    let out = run(&[
        "check",
        "--headers-only",
        s(&share.join("flow/run2/tube_C07_truncated.fcs")),
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(4));

    // Schemas.
    for of in ["index", "search", "search-health", "search-export"] {
        let out = run(&["self", "schema", of]);
        assert!(out.status.success(), "{of}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["type"], "object", "{of}");
    }
}

/// The MCP tools over stdio: `openreadout_index` (with progress notifications and a cap),
/// then `openreadout_search`.
#[test]
fn mcp_index_and_search() {
    use std::io::{BufRead, BufReader, Write};
    let tmp = tempfile::tempdir().unwrap();
    let share = tmp.path().join("share");
    for d in 0..3 {
        openreadout_index::synth::share(&share.join(format!("lab{d}"))).unwrap();
    }
    let idx = tmp.path().join("idx");
    let mut child = bin()
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut send = |v: serde_json::Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let mut progress: Vec<serde_json::Value> = Vec::new();
    let mut wait = |id: i64| -> serde_json::Value {
        loop {
            let l = lines.next().unwrap().unwrap();
            let v: serde_json::Value = serde_json::from_str(&l).unwrap();
            if v["method"] == "notifications/progress" {
                progress.push(v["params"].clone());
                continue;
            }
            if v["id"] == id {
                return v;
            }
        }
    };
    send(
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
    );
    wait(1);
    send(serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let call = |id: i64| {
        serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{
        "name":"openreadout_index","_meta":{"progressToken":"p1"},
        "arguments":{"roots":[s(&share)],"index_dir":s(&idx),"max_files":10,"threads":2}}})
    };
    send(call(2));
    let r = wait(2);
    let m = &r["result"]["structuredContent"];
    assert_eq!(m["complete"], false, "{}", m["crawl"]);
    assert_eq!(m["crawl"]["items"], 10, "the cap holds within a chunk");
    assert!(m["tables"]["experiments"].get("columns").is_none());
    let mut id = 3;
    loop {
        send(call(id));
        let r = wait(id);
        id += 1;
        if r["result"]["structuredContent"]["complete"] == true {
            assert_eq!(r["result"]["structuredContent"]["datasets"], 21);
            break;
        }
        assert!(id < 20, "{}", r["result"]["structuredContent"]["crawl"]);
    }
    send(
        serde_json::json!({"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"openreadout_search","arguments":{"index_dir":s(&idx),"query":"format=fcs sample~A12","fields":["path","sample_id"]}}}),
    );
    let r = wait(100);
    let found = &r["result"]["structuredContent"];
    assert_eq!(found["total"], 3, "{r}");
    assert_eq!(found["results"][0]["sample_id"], "A12");
    assert!(!progress.is_empty(), "progress notifications were sent");
    let _ = child.kill();
    let _ = child.wait();
}

/// Kill a crawl mid-way (SIGKILL), rerun the same command: it resumes from the last checkpoint
/// and the tables equal those of an uninterrupted crawl.
#[test]
fn killed_crawl_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let share = tmp.path().join("share");
    for d in 0..120 {
        openreadout_index::synth::share(&share.join(format!("lab{d:02}"))).unwrap();
    }
    let idx = tmp.path().join("idx");
    let journal = idx.join("state/journal.jsonl");
    let mut child = bin()
        .args([
            "--threads",
            "2",
            "index",
            s(&share),
            "-o",
            s(&idx),
            "--check",
            "full",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let mut killed = false;
    loop {
        let ckpts = std::fs::read_to_string(&journal).map_or(0, |t| {
            t.lines()
                .filter(|l| l.starts_with("{\"t\":\"ckpt\""))
                .count()
        });
        if ckpts >= 1 {
            killed = child.try_wait().unwrap().is_none();
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        if child.try_wait().unwrap().is_some() || start.elapsed().as_secs() > 60 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let out = run(&[
        "index",
        s(&share),
        "-o",
        s(&idx),
        "--check",
        "full",
        "--json",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["complete"], true);
    assert_eq!(v["data"]["datasets"], 840);
    assert!(killed, "the crawl finished before it could be killed");
    {
        assert!(
            v["data"]["crawl"]["sessions"].as_u64().unwrap() >= 2,
            "{}",
            v["data"]["crawl"]
        );
    }
    let fresh = tmp.path().join("idx2");
    let out = run(&[
        "index",
        s(&share),
        "-o",
        s(&fresh),
        "--check",
        "full",
        "--json",
    ]);
    let w = json(&out);
    for k in [
        "datasets",
        "files",
        "bytes",
        "formats",
        "check_status",
        "pii",
        "unknown_extensions",
    ] {
        assert_eq!(v["data"][k], w["data"][k], "{k}");
    }
    let q = |i: &Path| {
        let out = run(&[
            "search",
            s(i),
            "",
            "--limit",
            "0",
            "--jsonl",
            "--fields",
            "path,sample_id,check_status,table_rows",
        ]);
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    assert_eq!(q(&idx), q(&fresh));
}

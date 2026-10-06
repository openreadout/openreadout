//! `openreadout_export` to Parquet over MCP sends progress notifications as it reads a trace,
//! ending at the total, so a client can tell a long export from a stuck one.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// A SpikeGLX NI-DAQ stream of `channels` x `samples` 16-bit samples (invented values).
fn spikeglx(dir: &std::path::Path, channels: u32, samples: u64) -> std::path::PathBuf {
    let bin = dir.join("progress_g0_t0.nidq.bin");
    let n = usize::try_from(samples * u64::from(channels)).unwrap();
    let data: Vec<u8> = (0..n)
        .flat_map(|i| ((i % 2000) as i16 - 1000).to_le_bytes())
        .collect();
    std::fs::write(&bin, &data).unwrap();
    let meta = format!(
        "acqMnMaXaDw=0,0,{channels},0\nappVersion=20230323\nfileSizeBytes={}\nfileTimeSecs={}\nfirstSample=0\n\
         nSavedChans={channels}\nniAiRangeMax=5\nniAiRangeMin=-5\nniMAGain=1\nniMNGain=1\n\
         niMaxInt=32768\nniSampRate=30000\nniXAChans1=0:{}\nniXDBytes1=0\n\
         snsMnMaXaDw=0,0,{channels},0\nsnsSaveChanSubset=all\ntypeThis=nidq\n",
        data.len(),
        samples as f64 / 30000.0,
        channels - 1,
    );
    std::fs::write(bin.with_extension("meta"), meta).unwrap();
    bin
}

#[test]
fn parquet_export_reports_progress() {
    let tmp = tempfile::tempdir().unwrap();
    // 300,000 samples: several 65,536-sample batches.
    let input = spikeglx(tmp.path(), 4, 300_000);
    let output = tmp.path().join("out.parquet");
    let mut child = Command::new(env!("CARGO_BIN_EXE_openreadout"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
    send(
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"openreadout_export","_meta":{"progressToken":"p1"},
        "arguments":{"file":input.to_string_lossy(),"format":"parquet","output":output.to_string_lossy()}}}),
    );
    let r = wait(2);
    assert!(
        r["result"]["isError"] != true && r.get("error").is_none(),
        "{r}"
    );
    assert!(output.exists());
    let _ = child.kill();
    let _ = child.wait();
    assert!(progress.len() >= 2, "progress notifications: {progress:?}");
    let last = progress.last().unwrap();
    assert_eq!(last["progress"], last["total"], "{progress:?}");
    assert_eq!(last["total"].as_f64(), Some(300_000.0), "{progress:?}");
}

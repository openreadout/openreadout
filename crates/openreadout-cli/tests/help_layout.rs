//! `--help` lists a command's own flags first ("Options"), then the shared sections ("Several
//! inputs", "Batch table …", "Global options"), so the flags that answer the question come first.

use std::process::Command;

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_openreadout"))
        .args(args)
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?} --help failed");
    String::from_utf8(out.stdout).unwrap()
}

/// Flags listed in the section that starts with `heading` (up to the next heading line).
fn section(text: &str, heading: &str) -> Vec<String> {
    let mut on = false;
    let mut flags = Vec::new();
    for line in text.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            on = line == heading;
            continue;
        }
        if on {
            let t = line.trim_start();
            if t.starts_with('-') && line.starts_with("  ") && !line.starts_with("          ") {
                let flag = t
                    .split([' ', ','])
                    .find(|w| w.starts_with("--"))
                    .unwrap_or(t)
                    .split('=')
                    .next()
                    .unwrap_or("")
                    .to_string();
                flags.push(flag);
            }
        }
    }
    flags
}

const GLOBAL: [&str; 8] = [
    "--threads",
    "--color",
    "--progress",
    "--no-progress",
    "--quiet",
    "--live-window",
    "--only",
    "--compact",
];
const BATCH: [&str; 5] = [
    "--recursive",
    "--jsonl",
    "--continue-on-error",
    "--fail-fast",
    "--skip-unknown",
];

#[test]
fn own_flags_come_before_shared_ones() {
    for (cmd, own) in [
        (
            &["table"][..],
            &["--table", "--first-row", "--filter", "--count", "--json"][..],
        ),
        (&["stats"], &["--image", "--select", "--mip", "--json"]),
        (&["trace"], &["--trace", "--sweep", "--json"]),
        (&["info"], &["--json", "--view", "--max-images"]),
        (&["analyze", "gate"], &["--json"]),
        (&["export"], &["--format", "--output", "--run", "--json"]),
    ] {
        let text = help(cmd);
        let options = section(&text, "Options:");
        for f in own {
            assert!(
                options.iter().any(|o| o == f),
                "`{cmd:?} --help`: {f} not in the first Options section: {options:?}"
            );
        }
        for f in GLOBAL.iter().chain(&BATCH) {
            assert!(
                !options.iter().any(|o| o == f),
                "`{cmd:?} --help`: shared flag {f} listed among the command's own options"
            );
        }
        let at = |h: &str| text.find(h);
        let opts = at("\nOptions:").unwrap();
        assert!(
            at("\nGlobal options:").is_some_and(|g| g > opts),
            "`{cmd:?} --help`: global options after the command's own"
        );
        let global = section(&text, "Global options:");
        assert!(global.iter().any(|f| f == "--threads"), "{global:?}");
        if let Some(b) = at("\nSeveral inputs:") {
            assert!(b > opts);
            let batch = section(&text, "Several inputs:");
            assert!(batch.iter().any(|f| f == "--recursive"), "{batch:?}");
        }
    }
}

#[test]
fn flag_names_are_unchanged() {
    // parsing is untouched: the shared flags still work after the command's own
    let out = Command::new(env!("CARGO_BIN_EXE_openreadout"))
        .args(["self", "formats", "--json", "--compact", "--quiet"])
        .output()
        .unwrap();
    assert!(out.status.success());
}

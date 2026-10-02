//! Records the target triple for `openreadout self doctor`, and embeds the agent skill's
//! reference files (`references/*.md`, copies of skills/openreadout/references kept by
//! `cargo xtask skill-parity`) for `openreadout self skill --install`.

use std::fmt::Write as _;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());
    println!("cargo:rustc-env=OPENREADOUT_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=references");
    let dir =
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("references");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "md"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut code = String::from("&[\n");
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let name = f.file_name().unwrap().to_string_lossy();
        let _ = writeln!(
            code,
            "    ({name:?}, include_str!({:?})),",
            f.display().to_string()
        );
    }
    code.push(']');
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("skill_references.rs");
    std::fs::write(out, code).unwrap();
}

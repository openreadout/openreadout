//! Distribution helpers: package-manager manifests rendered from release checksums, and the
//! version-agreement check used by CI and the release workflow.
//!
//! Templates live under `packaging/` and use two kinds of placeholder:
//!
//! - `{{version}}` — the release version without a leading `v` (e.g. `0.1.0`);
//! - `{{sha256:<asset file name>}}` — the SHA-256 of that release asset, looked up in `SHA256SUMS`.
//!
//! Rendering fails if a placeholder is unknown, an asset is missing from the checksums, or a
//! checksum is not 64 hex digits, so a broken release can never produce a plausible-looking
//! manifest.
//!
//! A template's leading `#` comment block describes the template. The rendered file starts with
//! a short header of its own instead (see [`render_template`]).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::root;

/// Parse a `sha256sum`-style file: `<hex>  <name>` (text mode) or `<hex> *<name>` (binary mode).
pub fn parse_sums(text: &str) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let sha = parts.next().unwrap_or_default().to_ascii_lowercase();
        let name = parts
            .next()
            .map(|n| n.trim_start().trim_start_matches('*'))
            .unwrap_or_default();
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) || name.is_empty() {
            bail!(
                "SHA256SUMS line {}: expected `<64 hex digits>  <file>`",
                i + 1
            );
        }
        // Keep only the file name: `sha256sum dist/foo` writes a path.
        let name = name.rsplit('/').next().unwrap_or(name).to_string();
        out.insert(name, sha);
    }
    if out.is_empty() {
        bail!("SHA256SUMS is empty");
    }
    Ok(out)
}

/// Replace every `{{...}}` placeholder in `template`.
pub fn render(template: &str, version: &str, sums: &BTreeMap<String, String>) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            bail!(
                "unterminated placeholder near `{}`",
                &rest[start..rest.len().min(start + 40)]
            );
        };
        let key = after[..end].trim();
        if key == "version" {
            out.push_str(version);
        } else if let Some(asset) = key.strip_prefix("sha256:") {
            let sha = sums
                .get(asset.trim())
                .with_context(|| format!("release asset `{asset}` is not listed in SHA256SUMS"))?;
            out.push_str(sha);
        } else {
            bail!("unknown placeholder `{{{{{key}}}}}` (known: version, sha256:<asset>)");
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The header a rendered file starts with in place of the template's leading comment block.
/// JSON has no comments, so a JSON template gets none.
fn rendered_header(template: &str, version: &str) -> Option<String> {
    let src = format!("https://github.com/openreadout/openreadout/blob/main/packaging/{template}");
    match Path::new(template).extension().and_then(|e| e.to_str()) {
        Some("rb") => Some(format!(
            "# OpenReadout {version}: installs the prebuilt, statically linked release binary.\n\
             # Rendered from the template {src}\n"
        )),
        Some("yaml") => Some(format!(
            "# OpenReadout {version}, rendered from the template {src}\n"
        )),
        _ => None,
    }
}

/// Swap the leading `#` comment block of `text` for `header`. `# yaml-language-server:` lines
/// in that block stay first, because editors read the schema from them.
fn replace_header(text: &str, header: &str) -> String {
    let mut lines = text.split_inclusive('\n').peekable();
    let mut out = String::with_capacity(text.len());
    while let Some(line) = lines.next_if(|l| l.starts_with('#')) {
        if line.starts_with("# yaml-language-server:") {
            out.push_str(line);
        }
    }
    out.push_str(header);
    out.extend(lines);
    out
}

/// Render the template `packaging/<template>`, whose contents are `text`: fill in the
/// placeholders and replace the template's header comment.
pub fn render_template(
    template: &str,
    text: &str,
    version: &str,
    sums: &BTreeMap<String, String>,
) -> Result<String> {
    let rendered = render(text, version, sums)?;
    Ok(match rendered_header(template, version) {
        Some(header) => replace_header(&rendered, &header),
        None => rendered,
    })
}

/// The workspace version from the root `Cargo.toml`.
pub fn cargo_version() -> Result<String> {
    let text = fs::read_to_string(root().join("Cargo.toml"))?;
    let doc: toml::Value = toml::from_str(&text)?;
    doc.get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .context("Cargo.toml has no [workspace.package] version")
}

fn normalize_version(v: Option<String>) -> Result<String> {
    let v = match v {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => cargo_version()?,
    };
    let ok = v.split('.').count() >= 3
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if !ok {
        bail!("`{v}` is not a semantic version");
    }
    Ok(v)
}

/// Render `packaging/<template>` into `out`.
pub fn render_file(template: &str, version: Option<String>, sums: &Path, out: &Path) -> Result<()> {
    let version = normalize_version(version)?;
    let sums =
        parse_sums(&fs::read_to_string(sums).with_context(|| format!("read {}", sums.display()))?)?;
    let src = root().join("packaging").join(template);
    let text = fs::read_to_string(&src).with_context(|| format!("read {}", src.display()))?;
    let rendered = render_template(template, &text, &version, &sums)?;
    if Path::new(template)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        serde_json::from_str::<serde_json::Value>(&rendered)
            .with_context(|| format!("rendered {template} is not valid JSON"))?;
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, rendered)?;
    println!("wrote {} (version {version})", out.display());
    Ok(())
}

/// Render the three winget manifests into `<out>/manifests/o/OpenReadout/OpenReadout/<version>/`.
pub fn winget(version: Option<String>, sums: &Path, out: &Path) -> Result<()> {
    let v = normalize_version(version)?;
    let dir: PathBuf = out.join(format!("manifests/o/OpenReadout/OpenReadout/{v}"));
    for f in [
        "OpenReadout.OpenReadout.yaml",
        "OpenReadout.OpenReadout.installer.yaml",
        "OpenReadout.OpenReadout.locale.en-US.yaml",
    ] {
        render_file(&format!("winget/{f}"), Some(v.clone()), sums, &dir.join(f))?;
    }
    Ok(())
}

struct Checker {
    version: String,
    problems: Vec<String>,
    checked: usize,
}

impl Checker {
    fn expect(&mut self, what: &str, got: Option<&str>) {
        self.checked += 1;
        match got {
            Some(g) if g == self.version => {}
            Some(g) => self
                .problems
                .push(format!("{what}: {g} (expected {})", self.version)),
            None => self.problems.push(format!("{what}: missing")),
        }
    }

    fn require(&mut self, ok: bool, problem: impl FnOnce() -> String) {
        self.checked += 1;
        if !ok {
            self.problems.push(problem());
        }
    }
}

fn read_json(rel: &str) -> Result<serde_json::Value> {
    let p = root().join(rel);
    serde_json::from_str(&fs::read_to_string(&p).with_context(|| format!("read {rel}"))?)
        .with_context(|| format!("parse {rel}"))
}

fn read_toml(rel: &str) -> Result<toml::Value> {
    let p = root().join(rel);
    toml::from_str(&fs::read_to_string(&p).with_context(|| format!("read {rel}"))?)
        .with_context(|| format!("parse {rel}"))
}

/// Every place a release version is written must agree with `[workspace.package] version`.
/// With `tag`, the tag (minus a leading `v`) must match too and CHANGELOG.md must have its heading.
pub fn version_check(tag: Option<&str>) -> Result<()> {
    let v = cargo_version()?;
    let r = root();
    let mut c = Checker {
        version: v.clone(),
        problems: Vec::new(),
        checked: 0,
    };

    cargo_versions(&mut c, &r)?;
    mcp_versions(&mut c, &r, &v)?;
    plugin_versions(&mut c, &r)?;
    python_versions(&mut c, &v)?;
    pinned_versions(&mut c, &r)?;

    if let Some(tag) = tag {
        c.expect("git tag", Some(tag.trim_start_matches('v')));
        let changelog = fs::read_to_string(r.join("CHANGELOG.md"))?;
        c.require(changelog.contains(&format!("## [{v}]")), || {
            format!("CHANGELOG.md has no `## [{v}]` heading")
        });
    }

    if c.problems.is_empty() {
        println!("version {v}: {} version strings agree", c.checked);
        Ok(())
    } else {
        for p in &c.problems {
            println!("MISMATCH {p}");
        }
        bail!(
            "{} of {} version strings disagree with Cargo.toml ({v}); see scripts/bump-version.sh",
            c.problems.len(),
            c.checked
        )
    }
}

/// Internal dependency requirements, every crate inheriting the workspace version, and Cargo.lock.
fn cargo_versions(c: &mut Checker, r: &Path) -> Result<()> {
    let cargo = read_toml("Cargo.toml")?;
    if let Some(deps) = cargo
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(toml::Value::as_table)
    {
        for (name, spec) in deps.iter().filter(|(n, _)| n.starts_with("openreadout-")) {
            c.expect(
                &format!("Cargo.toml [workspace.dependencies] {name}"),
                spec.get("version").and_then(toml::Value::as_str),
            );
        }
    }
    for entry in fs::read_dir(r.join("crates"))? {
        let p = entry?.path().join("Cargo.toml");
        let Ok(text) = fs::read_to_string(&p) else {
            continue;
        };
        let doc: toml::Value = toml::from_str(&text)?;
        let inherits = doc
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.get("workspace"))
            .and_then(toml::Value::as_bool)
            == Some(true);
        c.require(inherits, || {
            format!(
                "{}: package version must be `version.workspace = true`",
                p.strip_prefix(r).unwrap_or(&p).display()
            )
        });
    }
    // Cargo.lock: workspace packages (path sources carry no `source`).
    let lock = read_toml("Cargo.lock")?;
    for pkg in lock
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = pkg.get("name").and_then(toml::Value::as_str).unwrap_or("");
        if name.starts_with("openreadout") && pkg.get("source").is_none() {
            c.expect(
                &format!("Cargo.lock {name}"),
                pkg.get("version").and_then(toml::Value::as_str),
            );
        }
    }
    Ok(())
}

/// The MCP registry entry, the names the registry matches, and the Claude Desktop bundle.
fn mcp_versions(c: &mut Checker, r: &Path, v: &str) -> Result<()> {
    // MCP registry entry and Claude Desktop bundle.
    let s = read_json("server.json")?;
    c.expect("server.json version", s["version"].as_str());
    for p in s["packages"].as_array().into_iter().flatten() {
        match p["registryType"].as_str() {
            Some("mcpb") => {
                let id = p["identifier"].as_str().unwrap_or("");
                let want = format!("/releases/download/v{v}/");
                c.require(id.contains(&want), || {
                    format!("server.json mcpb identifier {id} does not contain {want}")
                });
            }
            Some(kind) => c.expect(
                &format!("server.json packages[{kind}] version"),
                p["version"].as_str(),
            ),
            None => {}
        }
    }
    // The registry checks that the npm package names the server (`mcpName`).
    let npm_name = read_json("packaging/npm/package.json")?["mcpName"]
        .as_str()
        .map(str::to_owned);
    let server_name = s["name"].as_str().map(str::to_owned);
    c.require(npm_name.is_some() && npm_name == server_name, || {
        format!(
            "packaging/npm/package.json mcpName {npm_name:?} must equal server.json name {server_name:?}"
        )
    });
    // For the cargo package it reads an `mcp-name: <server name>` line in the crates.io README.
    if let Some(name) = server_name.as_deref() {
        let readme = fs::read_to_string(r.join("crates/openreadout-cli/README.md"))?;
        let marker = format!("mcp-name: {name}");
        c.require(readme.lines().any(|l| l.trim() == marker), || {
            format!("crates/openreadout-cli/README.md must have a line `{marker}`")
        });
    }
    c.expect(
        "mcpb/manifest.json version",
        read_json("mcpb/manifest.json")?["version"].as_str(),
    );
    Ok(())
}

/// The Claude Code plugin and marketplace, the Codex plugin, the Gemini CLI extension, the npm
/// and WebAssembly packages, CITATION.cff.
fn plugin_versions(c: &mut Checker, r: &Path) -> Result<()> {
    // Claude Code and Codex plugins and the Gemini CLI extension (the agent-plugins repository).
    let dir = crate::agent_plugins::SOURCE;
    for (label, version) in crate::agent_plugins::manifest_versions(&r.join(dir))? {
        c.expect(&format!("{dir}/{label}"), version.as_deref());
    }

    // npm wrapper, and the WebAssembly package.
    c.expect(
        "packaging/npm/package.json version",
        read_json("packaging/npm/package.json")?["version"].as_str(),
    );
    c.expect(
        "packaging/wasm/package.json version",
        read_json("packaging/wasm/package.json")?["version"].as_str(),
    );

    // CITATION.cff: `version: "X.Y.Z"` at the top level.
    let cff = fs::read_to_string(r.join("CITATION.cff")).context("read CITATION.cff")?;
    let cff_version = cff
        .lines()
        .find_map(|l| l.strip_prefix("version:"))
        .map(|v| v.trim().trim_matches('"').to_owned());
    c.expect("CITATION.cff version", cff_version.as_deref());
    Ok(())
}

/// The Python extension and the plugins released with it.
fn python_versions(c: &mut Checker, v: &str) -> Result<()> {
    // Python: the extension takes its version from Cargo.
    let py = read_toml("pyproject.toml")?;
    let dynamic = py
        .get("project")
        .and_then(|p| p.get("dynamic"))
        .and_then(toml::Value::as_array)
        .is_some_and(|d| d.iter().any(|x| x.as_str() == Some("version")));
    c.require(dynamic, || {
        "pyproject.toml: project.version must be dynamic (taken from Cargo)".into()
    });
    // The bioio and napari plugins are released in lockstep and require the matching series.
    for file in [
        "python/bioio-openreadout/pyproject.toml",
        "python/napari-openreadout/pyproject.toml",
    ] {
        let plugin = read_toml(file)?;
        let project = plugin.get("project");
        c.expect(
            &format!("{file} version"),
            project
                .and_then(|p| p.get("version"))
                .and_then(toml::Value::as_str),
        );
        let floor = format!(">={v}");
        let dep = project
            .and_then(|p| p.get("dependencies"))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str)
            .find(|d| d.starts_with("openreadout"));
        c.require(dep.is_some_and(|d| d.contains(&floor)), || {
            format!(
                "{file}: dependency on openreadout must require {floor} (found {})",
                dep.unwrap_or("none")
            )
        });
    }
    Ok(())
}

/// The R package and the versions workflow integrations pin.
fn pinned_versions(c: &mut Checker, r: &Path) -> Result<()> {
    // R package, and the version the workflow integrations pin (book/src/guides/r.md, book/src/guides/pipelines.md).
    let find = |file: &str, re_prefix: &str, re_suffix: &[char]| -> Result<Vec<String>> {
        let text = fs::read_to_string(r.join(file)).with_context(|| format!("reading {file}"))?;
        Ok(text
            .match_indices(re_prefix)
            .map(|(i, m)| {
                text[i + m.len()..]
                    .split(|ch: char| re_suffix.contains(&ch) || ch.is_whitespace())
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect())
    };
    for (file, prefix, stop) in [
        ("r/openreadout/DESCRIPTION", "\nVersion: ", &[][..]),
        (
            "integrations/galaxy/macros.xml",
            "name=\"@TOOL_VERSION@\">",
            &['<'][..],
        ),
        (
            "integrations/bioconda/meta.yaml",
            "set version = \"",
            &['"'][..],
        ),
        (
            "integrations/nextflow/modules/openreadout/info/environment.yml",
            "openreadout=",
            &[][..],
        ),
        (
            "integrations/nextflow/modules/openreadout/export/environment.yml",
            "openreadout=",
            &[][..],
        ),
        (
            "integrations/nextflow/modules/openreadout/batch/environment.yml",
            "openreadout=",
            &[][..],
        ),
        (
            "integrations/nextflow/modules/openreadout/info/main.nf",
            "openreadout:",
            &['-'][..],
        ),
        (
            "integrations/nextflow/modules/openreadout/export/main.nf",
            "openreadout:",
            &['-'][..],
        ),
        (
            "integrations/nextflow/modules/openreadout/batch/main.nf",
            "openreadout:",
            &['-'][..],
        ),
        (
            "integrations/snakemake/wrappers/openreadout/export/environment.yaml",
            "openreadout =",
            &[][..],
        ),
        (
            "integrations/snakemake/wrappers/openreadout/batch/environment.yaml",
            "openreadout =",
            &[][..],
        ),
    ] {
        let found = find(file, prefix, stop)?;
        c.require(!found.is_empty(), || format!("{file}: no version found"));
        for v in found {
            c.expect(file, Some(v.as_str()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn sums_text_and_binary_mode() {
        let s = parse_sums(&format!(
            "{A}  dist/x.tar.gz\n{}  *y.zip\n",
            A.to_uppercase()
        ))
        .unwrap();
        assert_eq!(s["x.tar.gz"], A);
        assert_eq!(s["y.zip"], A);
        assert!(parse_sums("nothex  x\n").is_err());
        assert!(parse_sums("\n").is_err());
    }

    #[test]
    fn render_placeholders() {
        let sums = parse_sums(&format!("{A}  x.tar.gz\n")).unwrap();
        let out = render("v{{version}} {{ sha256:x.tar.gz }} #{bin}", "1.2.3", &sums).unwrap();
        assert_eq!(out, format!("v1.2.3 {A} #{{bin}}"));
        assert!(render("{{sha256:missing.zip}}", "1.2.3", &sums).is_err());
        assert!(render("{{nope}}", "1.2.3", &sums).is_err());
        assert!(render("{{version", "1.2.3", &sums).is_err());
    }

    #[test]
    fn packaging_templates_render() {
        let targets = [
            "openreadout-x86_64-unknown-linux-musl.tar.gz",
            "openreadout-aarch64-unknown-linux-musl.tar.gz",
            "openreadout-aarch64-apple-darwin.tar.gz",
            "openreadout-x86_64-apple-darwin.tar.gz",
            "openreadout-x86_64-pc-windows-msvc.zip",
        ];
        let sums: BTreeMap<String, String> = targets
            .iter()
            .map(|t| ((*t).to_string(), A.to_string()))
            .collect();
        for t in [
            "homebrew/openreadout.rb",
            "scoop/openreadout.json",
            "winget/OpenReadout.OpenReadout.yaml",
            "winget/OpenReadout.OpenReadout.installer.yaml",
            "winget/OpenReadout.OpenReadout.locale.en-US.yaml",
        ] {
            let text = fs::read_to_string(root().join("packaging").join(t)).unwrap();
            let out = render_template(t, &text, "9.8.7", &sums).unwrap();
            assert!(!out.contains("{{"), "{t} still has placeholders");
            assert!(out.contains("9.8.7"), "{t} has no version");
            assert!(
                !out.contains("TEMPLATE"),
                "{t} still has the template's header"
            );
        }
    }

    #[test]
    fn rendered_files_get_their_own_header() {
        let sums = parse_sums(&format!("{A}  x.tar.gz\n")).unwrap();
        let rb = "# Formula TEMPLATE.\n#\n# Do not install this file directly.\nclass X < Formula\n  # kept\nend\n";
        let out = render_template("homebrew/openreadout.rb", rb, "1.2.3", &sums).unwrap();
        assert_eq!(
            out,
            "# OpenReadout 1.2.3: installs the prebuilt, statically linked release binary.\n\
             # Rendered from the template https://github.com/openreadout/openreadout/blob/main/packaging/homebrew/openreadout.rb\n\
             class X < Formula\n  # kept\nend\n"
        );

        let yaml = "# yaml-language-server: $schema=s\n# TEMPLATE: rendered by xtask.\nA: \"{{version}}\"\n";
        let out = render_template("winget/X.yaml", yaml, "1.2.3", &sums).unwrap();
        assert_eq!(
            out,
            "# yaml-language-server: $schema=s\n\
             # OpenReadout 1.2.3, rendered from the template https://github.com/openreadout/openreadout/blob/main/packaging/winget/X.yaml\n\
             A: \"1.2.3\"\n"
        );

        let json = "{\"version\": \"{{version}}\"}\n";
        let out = render_template("scoop/x.json", json, "1.2.3", &sums).unwrap();
        assert_eq!(out, "{\"version\": \"1.2.3\"}\n");
    }
}

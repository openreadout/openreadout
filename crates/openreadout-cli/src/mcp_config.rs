//! `openreadout mcp`: serve MCP (stdio, or Streamable HTTP with the `mcp-http` feature), print a
//! client configuration, or install it into a client's configuration file.
//!
//! Client formats were checked against each client's public documentation on 2026-09-22; the
//! sources are listed in `book/src/reference/mcp.md`.

use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde_json::{Map, Value, json};

/// Name of our entry in every client configuration.
const SERVER: &str = "openreadout";

/// Arguments of `mcp`.
#[derive(Debug, clap::Args)]
pub struct McpArgs {
    /// Print the configuration snippet for a client instead of serving.
    #[arg(long, value_enum, value_name = "CLIENT")]
    pub config: Option<McpClient>,
    /// Write the configuration into the client's config file: other servers and settings are
    /// kept, the previous file is backed up next to it, and nothing changes if already configured.
    #[arg(long, value_enum, value_name = "CLIENT", conflicts_with = "config")]
    pub install: Option<McpClient>,
    /// With `--install`: write the project-level file in the current directory (`.mcp.json`,
    /// `.cursor/mcp.json`, `.vscode/mcp.json`, `.gemini/settings.json`, `.zed/settings.json`,
    /// `.codex/config.toml`) instead of the user-level one.
    #[arg(long, requires = "install")]
    pub project: bool,
    /// With `--install`: the configuration file to edit instead of the client's default.
    #[arg(long, value_name = "PATH", requires = "install")]
    pub config_path: Option<PathBuf>,
    /// Serve over Streamable HTTP at `ADDR` (e.g. `127.0.0.1:8765`, endpoint `/mcp`) instead of
    /// stdio. Needs a build with the `mcp-http` feature. Loopback addresses only unless
    /// `--allow-remote`; set `OPENREADOUT_MCP_TOKEN` to require `Authorization: Bearer <token>`.
    #[arg(long, value_name = "ADDR", conflicts_with_all = ["config", "install"])]
    pub http: Option<String>,
    /// Allow `--http` to bind a non-loopback address (every file this user can read becomes
    /// readable by anyone who can reach the port; use a token and a firewall).
    #[arg(long, requires = "http")]
    pub allow_remote: bool,
}

/// MCP clients `--config` / `--install` know.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum McpClient {
    /// Claude Code (`.mcp.json`, `~/.claude.json`).
    #[value(alias = "claude-code")]
    Claude,
    /// Claude Desktop (`claude_desktop_config.json`).
    ClaudeDesktop,
    /// Cursor (`~/.cursor/mcp.json`, `.cursor/mcp.json`).
    Cursor,
    /// OpenAI Codex CLI / IDE (`~/.codex/config.toml`).
    Codex,
    /// VS Code / GitHub Copilot (`mcp.json`, top-level `servers`).
    Vscode,
    /// Gemini CLI (`~/.gemini/settings.json`).
    #[value(alias = "gemini-cli")]
    Gemini,
    /// Windsurf / Devin Desktop Cascade (`mcp_config.json`).
    Windsurf,
    /// Zed (`settings.json`, `context_servers`).
    Zed,
    /// Continue (`.continue/mcpServers/openreadout.yaml`).
    Continue,
    /// Cline (`cline_mcp_settings.json`).
    Cline,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or
/// `~/.config` elsewhere.
fn app_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home().map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    }
}

/// `~/.config` on macOS and Linux (or `$XDG_CONFIG_HOME`), `%APPDATA%` on Windows.
fn xdg_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config")))
    }
}

/// How an entry is stored.
enum Shape {
    /// A JSON object at `key_path`, our entry under `SERVER`.
    Json {
        key_path: &'static [&'static str],
        entry: Value,
    },
    /// A `[mcp_servers.openreadout]` TOML table (Codex).
    CodexToml,
    /// A Continue block file (YAML).
    ContinueYaml,
}

fn shape(client: McpClient, exe: &str) -> Shape {
    let args = json!(["mcp"]);
    match client {
        McpClient::Claude => Shape::Json {
            key_path: &["mcpServers"],
            entry: json!({"type": "stdio", "command": exe, "args": args, "env": {}}),
        },
        McpClient::ClaudeDesktop | McpClient::Gemini => Shape::Json {
            key_path: &["mcpServers"],
            entry: json!({"command": exe, "args": args}),
        },
        McpClient::Cursor => Shape::Json {
            key_path: &["mcpServers"],
            entry: json!({"type": "stdio", "command": exe, "args": args, "env": {}}),
        },
        McpClient::Vscode => Shape::Json {
            key_path: &["servers"],
            entry: json!({"type": "stdio", "command": exe, "args": args}),
        },
        McpClient::Windsurf => Shape::Json {
            key_path: &["mcpServers"],
            entry: json!({"command": exe, "args": args, "env": {}}),
        },
        McpClient::Zed => Shape::Json {
            key_path: &["context_servers"],
            entry: json!({"command": exe, "args": args, "env": {}}),
        },
        McpClient::Cline => Shape::Json {
            key_path: &["mcpServers"],
            entry: json!({"command": exe, "args": args, "env": {}, "disabled": false, "autoApprove": []}),
        },
        McpClient::Codex => Shape::CodexToml,
        McpClient::Continue => Shape::ContinueYaml,
    }
}

fn toml_str(s: &str) -> String {
    serde_json::to_string(s).expect("string serializes")
}

fn codex_table(exe: &str) -> String {
    format!(
        "[mcp_servers.{SERVER}]\ncommand = {}\nargs = [\"mcp\"]\n",
        toml_str(exe)
    )
}

fn continue_block(exe: &str) -> String {
    format!(
        "name: OpenReadout\nversion: 0.0.1\nschema: v1\nmcpServers:\n  - name: {SERVER}\n    type: stdio\n    command: {}\n    args:\n      - \"mcp\"\n",
        toml_str(exe)
    )
}

/// The default file `--install` edits.
fn default_path(client: McpClient, project: bool) -> Result<PathBuf, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot read the current directory: {e}"))?;
    let need =
        |o: Option<PathBuf>| o.ok_or_else(|| "cannot determine the home directory".to_string());
    let project_path = match client {
        McpClient::Claude => Some(cwd.join(".mcp.json")),
        McpClient::Cursor => Some(cwd.join(".cursor").join("mcp.json")),
        McpClient::Vscode => Some(cwd.join(".vscode").join("mcp.json")),
        McpClient::Gemini => Some(cwd.join(".gemini").join("settings.json")),
        McpClient::Zed => Some(cwd.join(".zed").join("settings.json")),
        McpClient::Codex => Some(cwd.join(".codex").join("config.toml")),
        McpClient::Continue => Some(
            cwd.join(".continue")
                .join("mcpServers")
                .join("openreadout.yaml"),
        ),
        McpClient::ClaudeDesktop | McpClient::Windsurf | McpClient::Cline => None,
    };
    if project {
        return project_path.ok_or_else(|| {
            format!("{client:?} has no project-level configuration; drop --project")
        });
    }
    Ok(match client {
        McpClient::Claude => need(home())?.join(".claude.json"),
        McpClient::ClaudeDesktop => need(app_config_dir())?
            .join("Claude")
            .join("claude_desktop_config.json"),
        McpClient::Cursor => need(home())?.join(".cursor").join("mcp.json"),
        McpClient::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .map_or_else(|| need(home()).map(|h| h.join(".codex")), Ok)?
            .join("config.toml"),
        McpClient::Vscode => need(app_config_dir())?
            .join("Code")
            .join("User")
            .join("mcp.json"),
        McpClient::Gemini => need(home())?.join(".gemini").join("settings.json"),
        McpClient::Windsurf => {
            let legacy = need(home())?
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json");
            if legacy.exists() {
                legacy
            } else {
                need(xdg_config_dir())?
                    .join("devin")
                    .join("mcp_config.json")
            }
        }
        McpClient::Zed => {
            if cfg!(windows) {
                need(app_config_dir())?.join("Zed").join("settings.json")
            } else {
                need(xdg_config_dir())?.join("zed").join("settings.json")
            }
        }
        // Continue documents MCP block files only in the workspace (`.continue/mcpServers/`).
        McpClient::Continue => cwd
            .join(".continue")
            .join("mcpServers")
            .join("openreadout.yaml"),
        McpClient::Cline => {
            let ext = need(app_config_dir())?
                .join("Code")
                .join("User")
                .join("globalStorage")
                .join("saoudrizwan.claude-dev")
                .join("settings");
            if ext.is_dir() {
                ext.join("cline_mcp_settings.json")
            } else {
                need(home())?.join(".cline").join("mcp.json")
            }
        }
    })
}

/// The snippet `--config` prints.
pub fn snippet(client: McpClient, exe: &str) -> String {
    let q = toml_str(exe);
    match shape(client, exe) {
        Shape::Json { key_path, entry } => {
            let mut v = Value::Object(Map::from_iter([(SERVER.to_string(), entry)]));
            for k in key_path.iter().rev() {
                v = Value::Object(Map::from_iter([((*k).to_string(), v)]));
            }
            let head = match client {
                McpClient::Claude => format!(
                    "# Claude Code: project .mcp.json (or ~/.claude.json), or run:\n#   claude mcp add --transport stdio --scope user {SERVER} -- {q} mcp"
                ),
                McpClient::ClaudeDesktop => "# Claude Desktop: merge into claude_desktop_config.json\n#   macOS: ~/Library/Application Support/Claude/claude_desktop_config.json\n#   Windows: %APPDATA%\\Claude\\claude_desktop_config.json\n# (or install the .mcpb bundle from the release page)".into(),
                McpClient::Cursor => "# Cursor: ~/.cursor/mcp.json (global) or .cursor/mcp.json (project)".into(),
                McpClient::Vscode => format!(
                    "# VS Code: .vscode/mcp.json (workspace) or the user mcp.json (\"MCP: Open User Configuration\"); the top-level key is `servers`. Or run:\n#   code --add-mcp '{{\"name\":\"{SERVER}\",\"command\":{q},\"args\":[\"mcp\"]}}'"
                ),
                McpClient::Gemini => format!(
                    "# Gemini CLI: ~/.gemini/settings.json (user) or .gemini/settings.json (project), or run:\n#   gemini mcp add -s user {SERVER} {q} mcp"
                ),
                McpClient::Windsurf => "# Windsurf / Devin Desktop (Cascade): ~/.codeium/windsurf/mcp_config.json (Windsurf) or ~/.config/devin/mcp_config.json (Devin Desktop; Windows %APPDATA%\\devin\\mcp_config.json)".into(),
                McpClient::Zed => "# Zed: merge into ~/.config/zed/settings.json (Windows %APPDATA%\\Zed\\settings.json) or .zed/settings.json".into(),
                McpClient::Cline => "# Cline: merge into cline_mcp_settings.json (MCP Servers panel -> Configure) or ~/.cline/mcp.json (Cline CLI)".into(),
                McpClient::Codex | McpClient::Continue => String::new(),
            };
            format!(
                "{head}\n{}",
                serde_json::to_string_pretty(&v).expect("json serializes")
            )
        }
        Shape::CodexToml => format!(
            "# Codex: add to ~/.codex/config.toml (or $CODEX_HOME/config.toml), or run:\n#   codex mcp add {SERVER} -- {q} mcp\n{}",
            codex_table(exe)
        ),
        Shape::ContinueYaml => format!(
            "# Continue: save as .continue/mcpServers/openreadout.yaml (MCP tools work in agent mode)\n{}",
            continue_block(exe)
        ),
    }
}

/// Merge our entry into a JSON document; `Ok(None)` when it is already there unchanged.
fn merge_json(text: &str, key_path: &[&str], entry: &Value) -> Result<Option<String>, String> {
    let mut doc: Value = if text.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(text).map_err(|e| {
            format!("not plain JSON ({e}); comments or trailing commas are not edited automatically, add the `--config` snippet by hand")
        })?
    };
    let mut cur = &mut doc;
    for k in key_path {
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| format!("`{k}`'s parent is not a JSON object"))?;
        cur = obj
            .entry((*k).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let servers = cur
        .as_object_mut()
        .ok_or_else(|| format!("`{}` is not a JSON object", key_path.join(".")))?;
    if servers.get(SERVER) == Some(entry) {
        return Ok(None);
    }
    servers.insert(SERVER.to_string(), entry.clone());
    let mut s = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    s.push('\n');
    Ok(Some(s))
}

/// Replace (or append) the `[mcp_servers.openreadout]` table, leaving the rest of the file as is.
fn merge_codex(text: &str, table: &str) -> Option<String> {
    let header = format!("[mcp_servers.{SERVER}]");
    let sub = format!("[mcp_servers.{SERVER}.");
    let mut out = String::new();
    let mut skipping = false;
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t == header {
            skipping = true;
            if !replaced {
                out.push_str(table);
                replaced = true;
            }
            continue;
        }
        if skipping && t.starts_with('[') && !t.starts_with(&sub) {
            skipping = false;
            out.push('\n');
        }
        if !skipping {
            out.push_str(line);
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with("\n\n") {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(table);
    }
    (out != text).then_some(out)
}

/// Write `content` to `path` through a verified temporary file, backing up the old file.
fn write_config(path: &Path, content: &str) -> Result<Option<PathBuf>, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let backup = if path.exists() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let name = path
            .file_name()
            .map_or_else(|| "config".into(), |n| n.to_string_lossy().to_string());
        let b = path.with_file_name(format!("{name}.bak.{secs}"));
        std::fs::copy(path, &b).map_err(|e| format!("cannot back up to {}: {e}", b.display()))?;
        Some(b)
    } else {
        None
    };
    let tmp = path.with_file_name(format!(
        ".{}.partial-{}",
        path.file_name()
            .map_or_else(|| "config".into(), |n| n.to_string_lossy().to_string()),
        std::process::id()
    ));
    std::fs::write(&tmp, content).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    if std::fs::read_to_string(&tmp).ok().as_deref() != Some(content) {
        let _ = std::fs::remove_file(&tmp);
        return Err("read-back verification failed; nothing changed".into());
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", path.display())
    })?;
    Ok(backup)
}

/// Install the configuration for `client` into `path`. Returns a message for the user.
pub fn install_at(client: McpClient, exe: &str, path: &Path) -> Result<String, String> {
    let old = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let new = match shape(client, exe) {
        Shape::Json { key_path, entry } => {
            merge_json(&old, key_path, &entry).map_err(|e| format!("{}: {e}", path.display()))?
        }
        Shape::CodexToml => merge_codex(&old, &codex_table(exe)),
        Shape::ContinueYaml => {
            let block = continue_block(exe);
            (old != block).then_some(block)
        }
    };
    let Some(new) = new else {
        return Ok(format!(
            "{} already configured in {}",
            SERVER,
            path.display()
        ));
    };
    let backup = write_config(path, &new)?;
    Ok(match backup {
        Some(b) => format!(
            "configured {SERVER} in {} (previous file saved as {})",
            path.display(),
            b.display()
        ),
        None => format!("configured {SERVER} in {} (new file)", path.display()),
    })
}

fn exe() -> String {
    std::env::current_exe().map_or_else(|_| "openreadout".into(), |p| p.display().to_string())
}

#[cfg(feature = "mcp")]
fn serve(a: &McpArgs) -> i32 {
    let Some(addr) = &a.http else {
        return match openreadout_mcp::serve_stdio(crate::registry::registry) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("mcp server error: {e}");
                1
            }
        };
    };
    let addr: std::net::SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: bad --http address '{addr}' ({e}); expected e.g. 127.0.0.1:8765");
            return 2;
        }
    };
    if !addr.ip().is_loopback() && !a.allow_remote {
        eprintln!(
            "error: {addr} is not a loopback address; the MCP HTTP server binds 127.0.0.1 / ::1 only unless --allow-remote"
        );
        return 2;
    }
    serve_http(addr, a.allow_remote)
}

#[cfg(all(feature = "mcp", feature = "mcp-http"))]
fn serve_http(addr: std::net::SocketAddr, allow_remote: bool) -> i32 {
    let token = std::env::var("OPENREADOUT_MCP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    match openreadout_mcp::serve_http(crate::registry::registry, &{
        let mut opts = openreadout_mcp::HttpOptions::new(addr);
        opts.allow_remote = allow_remote;
        opts.token = token;
        opts
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mcp server error: {e}");
            1
        }
    }
}

#[cfg(all(feature = "mcp", not(feature = "mcp-http")))]
fn serve_http(_: std::net::SocketAddr, _: bool) -> i32 {
    eprintln!(
        "error: this build has no Streamable HTTP transport\nhint: rebuild with `cargo install openreadout --features mcp-http`, or use stdio (`openreadout mcp`)"
    );
    6
}

#[cfg(not(feature = "mcp"))]
fn serve(_: &McpArgs) -> i32 {
    eprintln!("this build was compiled without the `mcp` feature");
    6
}

/// Run `mcp`; returns the exit code.
pub fn run(a: &McpArgs) -> i32 {
    if let Some(client) = a.config {
        println!("{}", snippet(client, &exe()));
        return 0;
    }
    if let Some(client) = a.install {
        let path = match a
            .config_path
            .clone()
            .map_or_else(|| default_path(client, a.project), Ok)
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        return match install_at(client, &exe(), &path) {
            Ok(msg) => {
                println!("{msg}");
                0
            }
            Err(e) => {
                eprintln!(
                    "error: {e}\nhint: `openreadout mcp --config {}` prints the snippet to add by hand",
                    client
                        .to_possible_value()
                        .map_or_else(String::new, |v| v.get_name().to_string())
                );
                5
            }
        };
    }
    serve(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/usr/local/bin/openreadout";

    #[test]
    fn json_merge_keeps_other_servers_and_is_idempotent() {
        let old = r#"{"theme": "dark", "mcpServers": {"other": {"command": "x"}}}"#;
        let Shape::Json { key_path, entry } = shape(McpClient::ClaudeDesktop, EXE) else {
            unreachable!()
        };
        let new = merge_json(old, key_path, &entry).unwrap().unwrap();
        let v: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert_eq!(v["mcpServers"]["openreadout"]["args"], json!(["mcp"]));
        assert!(merge_json(&new, key_path, &entry).unwrap().is_none());
        assert!(merge_json("// comment\n{}", key_path, &entry).is_err());
    }

    #[test]
    fn codex_table_is_replaced_in_place() {
        let old = "model = \"o3\"\n\n[mcp_servers.openreadout]\ncommand = \"old\"\n\n[mcp_servers.openreadout.env]\nA = \"1\"\n\n[mcp_servers.other]\ncommand = \"y\"\n";
        let new = merge_codex(old, &codex_table(EXE)).unwrap();
        assert!(new.starts_with(
            "model = \"o3\"\n\n[mcp_servers.openreadout]\ncommand = \"/usr/local/bin/openreadout\""
        ));
        assert!(new.contains("[mcp_servers.other]\ncommand = \"y\""));
        assert!(!new.contains("old"));
        assert!(merge_codex(&new, &codex_table(EXE)).is_none());
        let appended = merge_codex("model = \"o3\"\n", &codex_table(EXE)).unwrap();
        assert_eq!(
            appended,
            "model = \"o3\"\n\n[mcp_servers.openreadout]\ncommand = \"/usr/local/bin/openreadout\"\nargs = [\"mcp\"]\n"
        );
    }

    #[test]
    fn snippets_use_each_clients_keys() {
        assert!(snippet(McpClient::Vscode, EXE).contains("\"servers\""));
        assert!(snippet(McpClient::Zed, EXE).contains("\"context_servers\""));
        assert!(snippet(McpClient::Cline, EXE).contains("\"autoApprove\""));
        assert!(snippet(McpClient::Codex, EXE).contains("[mcp_servers.openreadout]"));
        assert!(snippet(McpClient::Continue, EXE).contains("schema: v1"));
        assert!(snippet(McpClient::Gemini, EXE).contains("gemini mcp add"));
    }

    #[test]
    fn install_writes_backup_and_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("mcp.json");
        let m = install_at(McpClient::Cursor, EXE, &p).unwrap();
        assert!(m.contains("new file"), "{m}");
        let m = install_at(McpClient::Cursor, EXE, &p).unwrap();
        assert!(m.contains("already configured"), "{m}");
        std::fs::write(&p, r#"{"mcpServers": {"a": {"command": "b"}}}"#).unwrap();
        let m = install_at(McpClient::Cursor, EXE, &p).unwrap();
        assert!(m.contains("previous file saved"), "{m}");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["a"]["command"], "b");
        assert_eq!(v["mcpServers"]["openreadout"]["type"], "stdio");
    }
}

//! `info --view full --sidecar`: write each input's full metadata as `<file>.openreadout.json` next to it (or under
//! `-o DIR`, mirroring the input tree), never touching the input. The sidecar is written to a
//! temporary file, read back and compared, then renamed into place; an existing sidecar written
//! by the same `openreadout` version with the same options for a source of the same size and
//! modification time is left alone.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use openreadout_core::envelope::ToolId;
use openreadout_core::model::Dump;
use openreadout_core::{Error, Registry, Result, SCHEMA_VERSION};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::batch;

/// Appended to the input's file name (directory walks skip such files).
pub const SUFFIX: &str = openreadout_core::batch::SIDECAR_SUFFIX;

/// Most directory entries walked to fingerprint a directory data set (`.D`, `.d`, TopSpin).
const MAX_WALK: usize = 100_000;

/// The source a sidecar describes, as it was when the sidecar was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SidecarSource {
    /// File (or directory) name of the source.
    pub name: String,
    /// Size in bytes (a directory data set: the sum of its files).
    pub size_bytes: u64,
    /// Modification time in nanoseconds since the Unix epoch (a directory: the newest file).
    pub modified_ns: u64,
}

/// The `info --view full` options the sidecar was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SidecarOptions {
    /// The vendor metadata tree is included.
    pub vendor: bool,
    /// The provenance map is included.
    pub provenance: bool,
    /// Every per-frame record is included (`--all-frames`).
    pub all_frames: bool,
}

/// Sidecar bookkeeping: what it was made from and when.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SidecarMeta {
    /// The source's fingerprint (compared to decide whether to rewrite).
    pub source: SidecarSource,
    /// The options used.
    pub options: SidecarOptions,
    /// When the sidecar was written (ISO-8601 UTC).
    pub written_at: String,
}

/// The content of a `.openreadout.json` sidecar: the `info --view full --json` envelope plus
/// `sidecar`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SidecarFile {
    /// Always true.
    pub ok: bool,
    /// The envelope schema version.
    pub schema_version: String,
    /// The producing tool and its version.
    pub tool: ToolId,
    /// What the sidecar was made from.
    pub sidecar: SidecarMeta,
    /// The `info --view full` output.
    pub data: Dump,
}

/// Output of `info --view full --sidecar` for one input.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SidecarReport {
    /// The input file.
    pub input: String,
    /// The sidecar path.
    pub output: String,
    /// `written` or `unchanged` (an up-to-date sidecar was already there).
    pub status: String,
    /// Format id of the input (absent when the sidecar was unchanged and the file not opened).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Size of the sidecar in bytes.
    pub bytes: u64,
    /// True when a written sidecar was read back and matched.
    pub verified: bool,
}

impl batch::Item for SidecarReport {
    fn format(&self) -> Option<String> {
        self.format.clone()
    }
    fn contents(&self) -> Option<String> {
        let name = Path::new(&self.output)
            .file_name()
            .map_or_else(|| self.output.clone(), |n| n.to_string_lossy().to_string());
        Some(format!("{} {name}", self.status))
    }
}

fn nanos(t: std::time::SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// Size and modification time of a file, or of every file under a directory data set.
fn fingerprint(path: &Path) -> Result<SidecarSource> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().to_string(),
    );
    if !meta.is_dir() {
        return Ok(SidecarSource {
            name,
            size_bytes: meta.len(),
            modified_ns: meta.modified().map_or(0, nanos),
        });
    }
    let (mut size, mut newest, mut seen) = (0u64, 0u64, 0usize);
    let mut stack = vec![path.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| Error::io(&d, e))?;
        for e in entries.flatten() {
            seen += 1;
            if seen > MAX_WALK {
                break;
            }
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                size = size.saturating_add(m.len());
                newest = newest.max(m.modified().map_or(0, nanos));
            }
        }
    }
    Ok(SidecarSource {
        name,
        size_bytes: size,
        modified_ns: newest.max(meta.modified().map_or(0, nanos)),
    })
}

/// Where the sidecar of `input` goes: next to it, or under `out_dir` mirroring its relative path.
pub fn sidecar_path(input: &batch::Input<'_>, out_dir: Option<&Path>) -> PathBuf {
    let base = match out_dir {
        Some(d) => d.join(&input.item.relative),
        None => input.path.to_path_buf(),
    };
    let name = base
        .file_name()
        .map_or_else(|| "input".into(), |n| n.to_string_lossy().to_string());
    base.with_file_name(format!("{name}{SUFFIX}"))
}

/// Whether the sidecar at `path` was written for this source, options and tool version.
fn up_to_date(path: &Path, source: &SidecarSource, options: SidecarOptions) -> bool {
    let Ok(text) = std::fs::read(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&text) else {
        return false;
    };
    let version = v.pointer("/tool/version").and_then(|x| x.as_str());
    let src: Option<SidecarSource> = v
        .pointer("/sidecar/source")
        .and_then(|x| serde_json::from_value(x.clone()).ok());
    let opts: Option<SidecarOptions> = v
        .pointer("/sidecar/options")
        .and_then(|x| serde_json::from_value(x.clone()).ok());
    version == Some(env!("CARGO_PKG_VERSION"))
        && src.as_ref() == Some(source)
        && opts == Some(options)
}

/// Write (or keep) the sidecar of one input.
pub fn write(
    reg: &Registry,
    input: &batch::Input<'_>,
    options: SidecarOptions,
    out_dir: Option<&Path>,
) -> Result<SidecarReport> {
    let file = input.path;
    let output = sidecar_path(input, out_dir);
    if output == file {
        return Err(Error::Usage(
            "the sidecar path must differ from the input; raw files are never modified".into(),
        ));
    }
    let source = fingerprint(file)?;
    if up_to_date(&output, &source, options) {
        let bytes = std::fs::metadata(&output).map_or(0, |m| m.len());
        return Ok(SidecarReport {
            input: file.display().to_string(),
            output: output.display().to_string(),
            status: "unchanged".into(),
            format: None,
            bytes,
            verified: false,
        });
    }
    // A sidecar is a file, not an agent's context: every image.
    let dump = super::info::dump(reg, file, options, Some(0))?;
    let format = dump.file.format.id.clone();
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let doc = SidecarFile {
        ok: true,
        schema_version: SCHEMA_VERSION.into(),
        tool: crate::output::tool_id(),
        sidecar: SidecarMeta {
            source,
            options,
            written_at: openreadout_core::time::unix_to_iso8601(
                now.as_secs() as i64,
                now.subsec_millis(),
            ),
        },
        data: dump,
    };
    let mut text = serde_json::to_vec_pretty(&doc)
        .map_err(|e| Error::Other(format!("serializing the sidecar: {e}")))?;
    text.push(b'\n');
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let name = output
        .file_name()
        .map_or_else(|| "sidecar".into(), |n| n.to_string_lossy().to_string());
    let tmp = output.with_file_name(format!(".{name}.partial-{}", std::process::id()));
    let result = (|| -> Result<()> {
        {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
            f.write_all(&text).map_err(|e| Error::io(&tmp, e))?;
            f.sync_all().map_err(|e| Error::io(&tmp, e))?;
        }
        let back = std::fs::read(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let parsed: serde_json::Value = serde_json::from_slice(&back)
            .map_err(|e| Error::Other(format!("sidecar read-back: {e}")))?;
        if back != text || parsed.pointer("/data/file/format/id").is_none() {
            return Err(Error::Other(
                "sidecar read-back did not match what was written".into(),
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        std::fs::remove_file(&tmp).ok();
        return Err(e);
    }
    std::fs::rename(&tmp, &output).map_err(|e| Error::io(&output, e))?;
    Ok(SidecarReport {
        input: file.display().to_string(),
        output: output.display().to_string(),
        status: "written".into(),
        format: Some(format),
        bytes: text.len() as u64,
        verified: true,
    })
}

/// Human form of one report.
pub fn render(r: &SidecarReport) -> String {
    match r.status.as_str() {
        "unchanged" => format!("unchanged {} (up to date for {})", r.output, r.input),
        _ => format!(
            "wrote {} ({} bytes, verified={})",
            r.output, r.bytes, r.verified
        ),
    }
}

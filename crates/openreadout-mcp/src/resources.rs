//! MCP resources (`openreadout://formats`, `openreadout://file/{path}`,
//! `openreadout://preview/{path}`) and prompts ("summarize this file", "check this file and
//! explain problems", "convert to an open format").

use std::path::PathBuf;

use base64::Engine as _;
use openreadout_core::Registry;
use openreadout_core::model::FormatsOutput;
use rmcp::ErrorData as McpError;
use rmcp::model::{
    GetPromptResult, JsonObject, Prompt, PromptArgument, PromptMessage, ReadResourceResult,
    Resource, ResourceContents, ResourceTemplate, Role,
};

use crate::mcp_err;

pub const FORMATS_URI: &str = "openreadout://formats";
pub const FILE_PREFIX: &str = "openreadout://file/";
pub const PREVIEW_PREFIX: &str = "openreadout://preview/";
pub const FILE_TEMPLATE: &str = "openreadout://file/{path}";
pub const PREVIEW_TEMPLATE: &str = "openreadout://preview/{path}";

/// Byte budget of a preview returned inline (tool image content or resource blob).
pub const PREVIEW_BUDGET: usize = 750_000;

pub fn list() -> Vec<Resource> {
    vec![
        Resource::new(FORMATS_URI, "formats")
            .with_title("Supported formats")
            .with_description(
                "Every format OpenReadout reads or writes, with confidence and known gaps (the JSON of `openreadout self formats --json`).",
            )
            .with_mime_type("application/json"),
    ]
}

pub fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(FILE_TEMPLATE, "file")
            .with_title("Instrument file summary")
            .with_description(
                "Header-only summary of an instrument file: the openreadout_info JSON plus its plain-English explanation (view=explain). `path` is an absolute path, percent-encoded or not (openreadout://file//data/a.czi or openreadout://file/%2Fdata%2Fa.czi).",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(PREVIEW_TEMPLATE, "preview")
            .with_title("Instrument file preview")
            .with_description(
                "A PNG (or JPEG, when large) preview of the file's default view: image 0 (middle z, channel 0), trace 0 sweep 0, spectrum 0 or plate table 0, at most 768 px. Use the openreadout_preview tool for other views.",
            )
            .with_mime_type("image/png"),
    ]
}

/// Percent-decode a URI path component (invalid escapes are kept literally).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The file a resource URI names: the decoded path as given, or with a leading `/` added when the
/// client dropped it (`openreadout://file/data/a.czi` → `/data/a.czi`).
pub fn resolve_path(rest: &str) -> Result<PathBuf, McpError> {
    let decoded = percent_decode(rest);
    let p = PathBuf::from(&decoded);
    if p.exists() {
        return Ok(p);
    }
    let rooted = PathBuf::from(format!("/{decoded}"));
    if !decoded.starts_with('/') && rooted.exists() {
        return Ok(rooted);
    }
    Err(McpError::resource_not_found(
        format!("no such file: {decoded}"),
        Some(
            serde_json::json!({"hint": "Give an absolute path, e.g. openreadout://file//data/a.czi or openreadout://file/%2Fdata%2Fa.czi."}),
        ),
    ))
}

fn json_text<T: serde::Serialize>(v: &T) -> Result<String, McpError> {
    serde_json::to_string(v).map_err(|e| McpError::internal_error(e.to_string(), None))
}

pub fn read(registry: fn() -> Registry, uri: &str) -> Result<ReadResourceResult, McpError> {
    if uri == FORMATS_URI {
        let out = FormatsOutput {
            formats: registry().descriptors(),
        };
        return Ok(ReadResourceResult::new(vec![
            ResourceContents::text(json_text(&out)?, uri).with_mime_type("application/json"),
        ]));
    }
    if let Some(rest) = uri.strip_prefix(FILE_PREFIX) {
        let path = resolve_path(rest)?;
        let reg = registry();
        let (_, ds) = reg.open(&path).map_err(|e| mcp_err(&e))?;
        let mut info = ds.info().map_err(|e| mcp_err(&e))?;
        let experiment = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
        let mut summary = openreadout_core::InfoOutput {
            file: info.clone(),
            experiment: (!experiment.is_empty()).then(|| experiment.clone()),
            acquisition: None,
            plate: None,
            images_total: None,
            assurance: Some(openreadout_core::assurance::assess_dataset(
                ds.as_ref(),
                &info,
            )),
        };
        // Resources are read whole: keep a plate's thousands of alike field images out.
        summary.cap_images(Some(openreadout_core::experiment::PLATE_IMAGES_LISTED * 4));
        let summary = json_text(&summary)?;
        let _ = openreadout_core::reader::attach_frames(ds.as_ref(), &mut info, Some(1));
        let ex = openreadout_ops::explain::explain_with(&info, Some(&experiment), None);
        let mut text = format!("{}\n", ex.summary);
        for p in &ex.paragraphs {
            text.push('\n');
            text.push_str(p);
            text.push('\n');
        }
        for c in &ex.caveats {
            text.push_str(&format!("\nCaveat: {c}"));
        }
        return Ok(ReadResourceResult::new(vec![
            ResourceContents::text(summary, uri).with_mime_type("application/json"),
            ResourceContents::text(text.trim_end(), uri).with_mime_type("text/plain"),
        ]));
    }
    if let Some(rest) = uri.strip_prefix(PREVIEW_PREFIX) {
        let path = resolve_path(rest)?;
        let reg = registry();
        let (_, mut ds) = reg.open(&path).map_err(|e| mcp_err(&e))?;
        let info = ds.info().map_err(|e| mcp_err(&e))?;
        let req = {
            let mut preview_request = openreadout_preview::PreviewRequest::default();
            preview_request.max_size = 768;
            preview_request
        };
        let r = openreadout_preview::render(ds.as_mut(), &info, &req).map_err(|e| mcp_err(&e))?;
        let (out, bytes) = openreadout_preview::finish_within(
            &r,
            openreadout_preview::Encoding::Png,
            85,
            PREVIEW_BUDGET,
        )
        .map_err(|e| mcp_err(&e))?;
        let mime = if out.encoding == "png" {
            "image/png"
        } else {
            "image/jpeg"
        };
        return Ok(ReadResourceResult::new(vec![
            ResourceContents::blob(
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                uri,
            )
            .with_mime_type(mime),
        ]));
    }
    Err(McpError::resource_not_found(
        format!("unknown resource {uri}"),
        Some(
            serde_json::json!({"hint": format!("Known: {FORMATS_URI}, {FILE_TEMPLATE}, {PREVIEW_TEMPLATE}")}),
        ),
    ))
}

// ---------- prompts ----------

pub fn prompts() -> Vec<Prompt> {
    let file = || {
        PromptArgument::new("file")
            .with_description("Path to the instrument file (or folder dataset)")
            .with_required(true)
    };
    vec![
        Prompt::new(
            "summarize_file",
            Some("Summarize an instrument file: what it is, what was measured or imaged, how and when, with a look at the data."),
            Some(vec![file()]),
        )
        .with_title("Summarize this file"),
        Prompt::new(
            "check_file",
            Some("Check an instrument file's integrity and explain any problems in plain language."),
            Some(vec![file()]),
        )
        .with_title("Check this file and explain problems"),
        Prompt::new(
            "convert_to_open_format",
            Some("Convert an instrument file to the open format that fits its data (OME-TIFF or OME-Zarr for images, mzML for mass spectra, Allotrope ASM JSON for plate reads, CSV, Parquet or Arrow for tables and traces, NWB for electrophysiology, JCAMP-DX for NMR and other spectra), with read-back verification."),
            Some(vec![
                file(),
                PromptArgument::new("format")
                    .with_description("Target format: ome-tiff, ome-zarr, mzml, asm, csv, parquet, arrow, nwb or jcamp (default: chosen from the data)")
                    .with_required(false),
                PromptArgument::new("output")
                    .with_description("Output path (default: next to the input)")
                    .with_required(false),
            ]),
        )
        .with_title("Convert to an open format"),
    ]
}

fn arg(args: Option<&JsonObject>, name: &str) -> Option<String> {
    args.and_then(|a| a.get(name))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
}

pub fn get_prompt(name: &str, args: Option<&JsonObject>) -> Result<GetPromptResult, McpError> {
    let file = arg(args, "file")
        .ok_or_else(|| McpError::invalid_params("argument `file` is required", None))?;
    let (desc, text) = match name {
        "summarize_file" => (
            "Summarize this file",
            format!(
                "Summarize the lab-instrument file `{file}` for me.\n\n\
                 1. Call openreadout_info with view=explain for a plain-English account and openreadout_info for exact values.\n\
                 2. Call openreadout_preview to look at the data (a representative image plane or channel composite, a trace, a spectrum or a plate map) and describe what you see.\n\
                 3. Report what the file is, what was imaged or measured and how (instrument, channels or signals, sizes and units), when, and anything unusual (pass on the caveats and notes).\n\
                 4. Suggest sensible next steps (the explanation's suggested_commands are a good start).\n\n\
                 Only state values the tools returned; say when something is not recorded."
            ),
        ),
        "check_file" => (
            "Check this file and explain problems",
            format!(
                "Check whether the instrument file `{file}` is intact.\n\n\
                 1. Call openreadout_check. `ok: false` means at least one finding has severity error.\n\
                 2. For each finding, explain in plain language what it means (severity, code, message, byte offset), the likely cause (for example an acquisition or copy that was interrupted, a truncated transfer, a disk error) and which data are affected (which planes, sweeps or spectra).\n\
                 3. Call openreadout_info to say what is still readable, and suggest how to salvage it (for example openreadout_export with a selection of intact planes, or re-copying the original).\n\
                 4. If the file is fine, say so and list what was checked (checks_performed)."
            ),
        ),
        "convert_to_open_format" => {
            let out = arg(args, "output")
                .map(|o| format!(" and output `{o}`"))
                .unwrap_or_default();
            let target = arg(args, "format").map_or_else(
                || {
                    "Pick the format from the data: images → OME-TIFF (OME-Zarr for very large or \
                     multi-resolution images); mass spectra → mzML; plate-reader exports → Allotrope ASM JSON; \
                     tables (FCS events, spike/event files) and traces (electrophysiology, NMR, JCAMP-DX, \
                     chromatograms) → CSV for a quick look, Parquet (format=\"parquet\") for analysis in \
                     pandas/polars/DuckDB; electrophysiology → NWB (format=\"nwb\") for DANDI-style archives; \
                     NMR FIDs and spectra → JCAMP-DX (format=\"jcamp\")."
                        .to_string()
                },
                |f| format!("The requested format is `{f}`."),
            );
            (
                "Convert to an open format",
                format!(
                    "Convert the instrument file `{file}` to an open format. {target}\n\n\
                     1. Call openreadout_info to see what the file holds (images, spectra, tables, traces) and how big it is.\n\
                     2. Tell me the size of the job (images and planes, or spectra, rows, sweeps). For very large image files suggest a selection (image, c/z/t) or OME-Zarr.\n\
                     3. Call openreadout_export with the format{out}. \
                     It writes a new file, reads it back and verifies it, and never modifies the source. \
                     For CSV, pick the table or the trace and sweep (table, or trace and sweep).\n\
                     4. Report the output path, what was written, bytes, and whether `verified` is true."
                ),
            )
        }
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "unknown prompt '{other}' (summarize_file, check_file, convert_to_open_format)"
                ),
                None,
            ));
        }
    };
    Ok(
        GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
            .with_description(desc),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_paths() {
        assert_eq!(percent_decode("%2Fdata%2Fa%20b.czi"), "/data/a b.czi");
        assert_eq!(percent_decode("/x/%zz"), "/x/%zz");
        assert_eq!(percent_decode("abc%"), "abc%");
    }

    #[test]
    fn prompts_render() {
        let mut a = JsonObject::new();
        a.insert("file".into(), serde_json::json!("/d/a.czi"));
        for p in prompts() {
            let r = get_prompt(&p.name, Some(&a)).unwrap();
            assert_eq!(r.messages.len(), 1);
        }
        assert!(get_prompt("summarize_file", None).is_err());
        assert!(get_prompt("nope", Some(&a)).is_err());
    }
}

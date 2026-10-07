//! `openreadout_check` (integrity), `openreadout_compare` (two files) and `openreadout_report`
//! (the diagnostic bundle).

use std::path::{Path, PathBuf};

use openreadout_core::Error;
use openreadout_core::model::CheckReport;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_check`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckArgs {
    /// Absolute or working-directory-relative path to the file or data-set directory.
    pub file: String,
    /// Check headers and structure only (offsets, declared sizes and counts, missing parts):
    /// no decompression, decoding or checksums over the data.
    #[serde(default)]
    pub headers_only: bool,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Arguments for `openreadout_compare`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompareArgs {
    /// The first file (e.g. the raw file).
    pub file: String,
    /// The second file (e.g. an OME-TIFF export of `file`).
    pub against: String,
    /// Only this image index (in both files).
    pub image: Option<u32>,
    /// Plane selection strings such as `c=0`, `z=2-5`, `t=0,3`. A second file holding only the
    /// selected planes (an export with the same selection) is matched to them in order.
    #[serde(default)]
    pub select: Vec<String>,
    /// Pyramid level (0 = full resolution, the default).
    #[serde(default)]
    pub level: u32,
    /// Largest absolute sample difference that still counts as equal. Default: bit-identical
    /// planes (same xxh3-128).
    pub tolerance: Option<f64>,
    /// JSON pointers (into openreadout_info output) to leave out of the metadata diff; `*`
    /// matches one segment.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Metadata and geometry only, no pixels.
    #[serde(default)]
    pub no_pixels: bool,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Arguments for `openreadout_report`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReportArgs {
    /// The file (or data-set directory) that is refused, fails or is not validated.
    pub file: String,
    /// Also write the bundle to this new local file. Default: only return it.
    pub output: Option<String>,
    /// Keep free text from the file (sample, image and channel names, comments); the path and
    /// personal data are still replaced. Default false: only with the user's consent.
    #[serde(default)]
    pub include_text: bool,
}

#[tool_router(router = check_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_check",
        annotations(title = "Check integrity", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<CheckReport>(),
        description = "Integrity check: ok plus findings (severity, code, message) for truncation, missing planes or parts, bad blocks; a plate folder reports missing files per well; a file still being written reports acquisition_in_progress."
    )]
    pub(crate) fn check(
        &self,
        Parameters(a): Parameters<CheckArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let path = Path::new(&a.file);
        let (_, mut ds) = reg
            .open(path)
            .map_err(|e| mcp_err(&openreadout_core::live::annotate_error(path, e)))?;
        let mut r: CheckReport = if a.headers_only {
            ds.check_headers()
        } else {
            ds.check()
        }
        .map_err(|e| mcp_err(&e))?;
        if let Some(acq) = openreadout_core::live::assess_dataset(ds.as_ref(), path) {
            openreadout_core::live::apply_to_check(&mut r, &acq);
        }
        r.assurance = ds.file_assurance();
        ok_json(&r)
    }

    #[tool(
        name = "openreadout_compare",
        annotations(title = "Compare two files", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_ops::compare::CompareOutput>(),
        description = "Compare two files, e.g. a raw file and its export: metadata differences, geometry, channel names, physical sizes, and per-plane hashes or the largest difference within tolerance. identical says whether they hold the same data."
    )]
    pub(crate) fn compare(
        &self,
        Parameters(a): Parameters<CompareArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let (_, mut da) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
        let (_, mut db) = reg.open(Path::new(&a.against)).map_err(|e| mcp_err(&e))?;
        let ia = da.info().map_err(|e| mcp_err(&e))?;
        let ib = db.info().map_err(|e| mcp_err(&e))?;
        let out = openreadout_ops::compare::compare(da.as_mut(), &ia, db.as_mut(), &ib, &{
            let mut compare_request = openreadout_ops::compare::CompareRequest::default();
            compare_request.image = a.image;
            compare_request.select = a.select;
            compare_request.level = a.level;
            compare_request.tolerance = a.tolerance;
            compare_request.ignore = a.ignore;
            compare_request.no_pixels = a.no_pixels;
            compare_request
        })
        .map_err(|e| mcp_err(&e))?;
        ok_json(&out)
    }

    #[tool(
        name = "openreadout_report",
        annotations(title = "Diagnostic bundle", read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_index::report::ReportOutput>(),
        description = "A privacy-reviewed diagnostic bundle for a file that is refused, fails or is not validated, for a new-variant issue: no data values, free text only with include_text, and the issue_url to file it at. Nothing is sent; the user files it."
    )]
    pub(crate) fn report(
        &self,
        Parameters(a): Parameters<ReportArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(None);
        let input = PathBuf::from(&a.file);
        let mut opts = openreadout_index::report::ReportOptions::default();
        opts.include_text = a.include_text;
        let out = a.output.as_deref().map(PathBuf::from);
        if let Some(p) = &out
            && p.exists()
        {
            return Err(mcp_err(&Error::Usage(format!(
                "{} exists; pick another output path",
                p.display()
            ))));
        }
        let r = openreadout_index::report::build(
            &reg,
            &input,
            &opts,
            out.as_deref().map(|p| (p, false)),
        )
        .map_err(|e| mcp_err(&e))?;
        let mut o = openreadout_index::report::ReportOutput {
            output: None,
            bytes: None,
            issue_url: openreadout_index::report::ISSUE_URL.into(),
            report: r,
        };
        if let Some(p) = out {
            let mut bytes = serde_json::to_vec_pretty(&o.report)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            bytes.push(b'\n');
            openreadout_index::report::write_verified(&p, &bytes, false)
                .map_err(|e| mcp_err(&e))?;
            o.output = Some(p.display().to_string());
            o.bytes = Some(bytes.len() as u64);
        }
        ok_json(&o)
    }
}

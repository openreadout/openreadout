//! `openreadout_info`: what a file holds, in five views, with a thumbnail of image 0; and
//! `openreadout_formats`.

use std::path::Path;

use base64::Engine as _;
use openreadout_core::InfoOutput;
use openreadout_core::model::{DetectOutput, Dump, FileInfo, FormatsOutput, Listing};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::object_output;
use crate::{InstrumentServer, mcp_err, ok_json};

/// What to return.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InfoView {
    /// the header summary (default)
    #[default]
    Summary,
    /// the summary with per-frame records (time stamps, stage positions, exposure) and per-field provenance; vendor=true adds the vendor's raw metadata tree
    Full,
    /// the container: images, blocks/segments/chunks, attachments (label, thumbnail, previews), pyramid levels, with offsets and sizes
    Structure,
    /// a plain-English account with caveats and suggested next steps (ask answers questions)
    Explain,
    /// the format only, from the file's signature (cheapest; works on damaged files)
    Format,
}

/// Arguments for `openreadout_info`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InfoArgs {
    /// Absolute or working-directory-relative path to the instrument file or data-set directory.
    pub file: String,
    /// What to return.
    #[serde(default)]
    pub view: InfoView,
    /// A question in plain words ("what was the gradient?", "which channel is DAPI?", "how
    /// many MS/MS scans?"): answered under answers[] with the fields each came from. Implies
    /// view=explain.
    pub ask: Option<String>,
    /// List at most this many images (0 = all). Default: all, except screening plates (16; their
    /// field images are all alike and plate.wells[].images indexes every field).
    pub max_images: Option<usize>,
    /// view=summary: attach a small picture of image 0 (~384 px, with full-resolution pixel
    /// rulers) as image content. Default true; skipped (with a note) when it would decode too much.
    pub thumbnail: Option<bool>,
    /// view=full: include the vendor's raw metadata tree (can be megabytes). Default false.
    #[serde(default)]
    pub vendor: bool,
    /// view=full: per-frame records per image under images[].extra.frames. Default 100; 0
    /// omits them; -1 embeds all.
    pub max_frames: Option<i64>,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate (assurance.strict_refuses). Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// What `openreadout_info` returns, by view.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
#[allow(dead_code, clippy::large_enum_variant)]
enum InfoToolOutput {
    /// view=summary.
    Summary(InfoOutput),
    /// view=full.
    Full(Dump),
    /// view=structure.
    Structure(Listing),
    /// view=explain (or ask).
    Explain(openreadout_ops::Explanation),
    /// view=format.
    Format(DetectOutput),
}

/// Content blocks for the `openreadout_info` thumbnail: a small picture of image 0 with
/// rulers, and one line on how to read and zoom it; or only a line saying why there is none.
fn info_thumbnail(ds: &mut dyn openreadout_core::Dataset, info: &FileInfo) -> Vec<ContentBlock> {
    use openreadout_preview as pv;
    let Some(im0) = info.images.first() else {
        return Vec::new();
    };
    let skipped = |why: &str| {
        vec![ContentBlock::text(format!(
            "no thumbnail of image {}: {why}. To look at it call openreadout_preview with region {{x, y, width, height}} (full-res px) or a coarser level.",
            im0.index
        ))]
    };
    let r = match pv::thumbnail(
        ds,
        info,
        pv::DEFAULT_THUMBNAIL_SIZE,
        pv::DEFAULT_THUMBNAIL_READ_BYTES,
    ) {
        Ok(r) => r,
        Err(e) => return skipped(&e.to_string()),
    };
    let png = pv::finish(&r, pv::Encoding::Png, pv::DEFAULT_JPEG_QUALITY);
    let jpeg = pv::finish(&r, pv::Encoding::Jpeg, 85);
    let (out, bytes, mime) = match (png, jpeg) {
        (Ok(p), Ok(j)) if j.1.len() < p.1.len() => (j.0, j.1, "image/jpeg"),
        (Ok(p), _) => (p.0, p.1, "image/png"),
        (Err(_), Ok(j)) => (j.0, j.1, "image/jpeg"),
        (Err(e), Err(_)) => return skipped(&e.to_string()),
    };
    let Some(im) = out.image else {
        return Vec::new();
    };
    let what = if im.composite {
        format!("composite of c={:?}", im.c)
    } else {
        format!("c={}", im.c.first().copied().unwrap_or(0))
    };
    vec![
        ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            mime,
        ),
        ContentBlock::text(format!(
            "thumbnail of image {} ({what}, z={}, t={}; level {}): rulers in full-res px, 1 px = {:.1} source px. Zoom with openreadout_preview region {{x, y, width, height}} read off the rulers; numbers come from openreadout_stats.",
            im.image,
            im.z.first().copied().unwrap_or(0),
            im.t.first().copied().unwrap_or(0),
            im.level,
            im.source_per_pixel
        )),
    ]
}

#[tool_router(router = info_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_info",
        annotations(title = "What a file holds", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<InfoToolOutput>(),
        description = "What an instrument file or data-set directory holds, from its headers (fast on huge files): format, images[] (sizes, pixel type, µm/px, channels and dyes, objective), tables[], traces[], spectra[] (MS runs), plate, experiment (sample, instrument, method, operator, start) and assurance. view=summary attaches a ~384 px thumbnail of image 0 with full-resolution pixel rulers. ask answers a question in words, naming the fields it used."
    )]
    pub(crate) fn info(
        &self,
        Parameters(a): Parameters<InfoArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let path = Path::new(&a.file);
        let view = if a.ask.is_some() {
            InfoView::Explain
        } else {
            a.view
        };
        let live = |e| mcp_err(&openreadout_core::live::annotate_error(path, e));
        if view == InfoView::Format {
            let (reader, det) = reg.detect(path).map_err(|e| mcp_err(&e))?;
            return ok_json(&DetectOutput {
                path: a.file.clone(),
                format: det.format_id.into(),
                name: reader.descriptor().name,
                confidence: det.confidence,
                note: det.note,
            });
        }
        let (det, mut ds) = reg.open(path).map_err(live)?;
        match view {
            InfoView::Structure => ok_json(&Listing {
                path: a.file.clone(),
                format: det.format_id.into(),
                entries: ds.entries().map_err(|e| mcp_err(&e))?,
                acquisition: openreadout_core::live::assess_dataset(ds.as_ref(), path),
            }),
            InfoView::Explain => {
                let mut info = ds.info().map_err(|e| mcp_err(&e))?;
                let experiment = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
                // One record per image tells whether per-frame records exist (and how many).
                let _ = openreadout_core::reader::attach_frames(ds.as_ref(), &mut info, Some(1));
                let assurance = openreadout_core::assurance::assess_dataset(ds.as_ref(), &info);
                ok_json(
                    &openreadout_ops::explain::explain_with(
                        &info,
                        Some(&experiment),
                        a.ask.as_deref(),
                    )
                    .with_assurance(assurance),
                )
            }
            InfoView::Full => {
                let mut info = ds.info().map_err(|e| mcp_err(&e))?;
                let limit = match a.max_frames {
                    None => Some(openreadout_core::reader::DEFAULT_FRAME_RECORDS),
                    Some(n) if n < 0 => None,
                    Some(n) => Some(n as usize),
                };
                if limit != Some(0) {
                    openreadout_core::reader::attach_frames(ds.as_ref(), &mut info, limit)
                        .map_err(|e| mcp_err(&e))?;
                }
                let mut summary = InfoOutput::new(ds.as_ref(), info);
                summary.cap_images(a.max_images);
                ok_json(&Dump {
                    file: summary,
                    vendor: if a.vendor {
                        Some(ds.vendor_metadata().map_err(|e| mcp_err(&e))?)
                    } else {
                        None
                    },
                    provenance: ds.provenance(),
                })
            }
            InfoView::Summary | InfoView::Format => {
                let info: FileInfo = ds.info().map_err(live)?;
                let thumb = if a.thumbnail.unwrap_or(true) {
                    info_thumbnail(ds.as_mut(), &info)
                } else {
                    Vec::new()
                };
                let mut out = InfoOutput::new(ds.as_ref(), info);
                out.acquisition = openreadout_core::live::assess_dataset(ds.as_ref(), path);
                // A screening plate has one image per field of view (thousands, all alike):
                // keep the answer small; `plate.wells[].images` still indexes every field.
                out.cap_images(a.max_images);
                let mut r = ok_json(&out)?;
                r.content.extend(thumb);
                Ok(r)
            }
        }
    }

    #[tool(
        name = "openreadout_formats",
        annotations(title = "Supported formats", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<FormatsOutput>(),
        description = "Supported formats with read/write support, confidence and known gaps (what is not decoded)."
    )]
    pub(crate) fn formats(&self) -> Result<CallToolResult, McpError> {
        ok_json(&FormatsOutput {
            formats: (self.registry)().descriptors(),
        })
    }
}

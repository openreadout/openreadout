//! Multi-well plate (high-content screening) options of `export` and `stats`: well selection,
//! partial copies, one OME-TIFF per field, and per-well statistics (`stats --per well`).

use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::plate::{WellStatsOutput, WellStatsRequest, plate_layout, well_stats};
use openreadout_core::{Dataset, Error, Registry, Result};
use schemars::JsonSchema;
use serde::Serialize;

use crate::ui::{self, Align};

/// Plate flags of `export`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct PlateExportArgs {
    /// Multi-well plates: export only the fields of this well (`C05`, `c5`). Repeatable.
    /// OME-Zarr and `--per-image` OME-TIFF.
    #[arg(long = "well", value_name = "WELL")]
    pub wells: Vec<String>,
    /// Multi-well plates: leave out fields whose plane files are missing (a partial copy)
    /// instead of failing; they are listed under `images_skipped`.
    #[arg(long)]
    pub skip_incomplete: bool,
    /// OME-Zarr of a plate: write a `bioformats2raw.layout` collection instead of the
    /// OME-NGFF HCS plate layout (row/column/field groups).
    #[arg(long)]
    pub no_plate: bool,
    /// OME-TIFF: one file per image (field of view), written into the directory given by
    /// `-o` (default `<input stem>.ome-tiffs/`), named `<well>_f<field>.ome.tiff` for plates.
    #[arg(long)]
    pub per_image: bool,
}

/// One file of a `--per-image` export.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PerImageFile {
    /// Image index in the input.
    pub image: u32,
    /// Well of the image (plates only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub well: Option<String>,
    /// Field number of the image (plates only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<u64>,
    /// The OME-TIFF written.
    pub output: String,
    /// Planes written.
    pub planes_written: u64,
    /// Size of the file in bytes.
    pub bytes_written: u64,
    /// True when every plane was read back and hashed equal to the source plane.
    pub verified: bool,
}

/// Output of `export --format ome-tiff --per-image`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PerImageExport {
    /// The file that was read.
    pub input: String,
    /// The directory the files were written into.
    pub output: String,
    /// Always `ome-tiff`.
    pub format: String,
    /// One entry per image written.
    pub files: Vec<PerImageFile>,
    /// Images left out because plane files they need are missing (`--skip-incomplete`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images_skipped: Vec<u32>,
    /// True when every file was verified.
    pub verified: bool,
}

/// Default output directory of `--per-image`.
pub fn default_dir(input: &Path) -> PathBuf {
    let base = if input.is_dir() {
        input.to_path_buf()
    } else {
        input.with_file_name(
            input
                .file_stem()
                .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string()),
        )
    };
    let name = base
        .file_name()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    base.with_file_name(format!("{name}.ome-tiffs"))
}

/// Export every selected image to its own OME-TIFF in `dir`.
pub fn export_per_image(
    reg: &Registry,
    file: &Path,
    ds: &mut dyn Dataset,
    dir: &Path,
    args: &PlateExportArgs,
    base: &openreadout_ometiff::ExportOptions,
    ctx: &ReadContext<'_>,
) -> Result<PerImageExport> {
    let _ = reg;
    let info = ds.info()?;
    let layout = plate_layout(ds, &info);
    let mut wanted: Option<std::collections::HashSet<u32>> = None;
    if !args.wells.is_empty() {
        let l = layout.as_ref().ok_or_else(|| {
            Error::Usage("--well applies to multi-well plates; this file is not one".into())
        })?;
        let mut set = std::collections::HashSet::new();
        for w in &args.wells {
            let pw = l
                .well(w)
                .ok_or_else(|| Error::Usage(format!("well '{w}' was not imaged on this plate")))?;
            set.extend(pw.images.iter().copied());
        }
        wanted = Some(set);
    }
    let sel = openreadout_core::select::Selection::parse(&base.select)?;
    if dir.exists() && !dir.is_dir() {
        return Err(Error::Usage(format!(
            "{} exists and is not a directory",
            dir.display()
        )));
    }
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for im in &info.images {
        if base.image.is_some_and(|i| i != im.index)
            || wanted.as_ref().is_some_and(|w| !w.contains(&im.index))
        {
            continue;
        }
        let (missing, all) = openreadout_core::plate::missing_planes(im);
        if all || missing.iter().any(|&(c, z, t)| sel.contains(c, z, t)) {
            if args.skip_incomplete {
                skipped.push(im.index);
                continue;
            }
            return Err(Error::Unsupported {
                format: "ome-tiff export",
                feature: format!(
                    "image {} ({}) has missing plane files",
                    im.index,
                    im.name.as_deref().unwrap_or("unnamed")
                ),
                hint: Some("This copy of the plate is incomplete (`openreadout check` lists the missing files). Pass --skip-incomplete to export only complete fields, or choose wells with --well.".into()),
            });
        }
        let well = im
            .extra
            .get("well")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let field = im.extra.get("field").and_then(serde_json::Value::as_u64);
        let name = match (&well, field) {
            (Some(w), Some(f)) => format!("{w}_f{f}.ome.tiff"),
            _ => format!("image{}.ome.tiff", im.index),
        };
        let out = dir.join(name);
        let mut o = base.clone();
        o.image = Some(im.index);
        let r = openreadout_ometiff::export_ome_tiff_with(ds, file, &out, &o, ctx)?;
        files.push(PerImageFile {
            image: im.index,
            well,
            field,
            output: r.output,
            planes_written: r.planes_written,
            bytes_written: r.bytes_written,
            verified: r.verified,
        });
    }
    if files.is_empty() {
        return Err(Error::Usage(if skipped.is_empty() {
            "the selection matches no image".into()
        } else {
            format!(
                "every selected image has missing plane files ({} skipped)",
                skipped.len()
            )
        }));
    }
    Ok(PerImageExport {
        input: file.display().to_string(),
        output: dir.display().to_string(),
        format: "ome-tiff".into(),
        verified: files.iter().all(|f| f.verified),
        files,
        images_skipped: skipped,
    })
}

/// Human rendering of a `--per-image` export.
pub fn render_per_image(r: &PerImageExport) -> String {
    let mut s = format!(
        "wrote {} OME-TIFF file(s) into {} (verified={})",
        r.files.len(),
        r.output,
        r.verified
    );
    for f in r.files.iter().take(20) {
        s.push_str(&format!(
            "\n  image {}{}: {} planes -> {}",
            f.image,
            f.well
                .as_ref()
                .map(|w| format!(" ({w} field {})", f.field.unwrap_or(0)))
                .unwrap_or_default(),
            f.planes_written,
            f.output
        ));
    }
    if r.files.len() > 20 {
        s.push_str(&format!("\n  ... {} more", r.files.len() - 20));
    }
    if !r.images_skipped.is_empty() {
        s.push_str(&format!(
            "\n  skipped {} image(s) with missing plane files",
            r.images_skipped.len()
        ));
    }
    s
}

/// The plate block of human `info` output.
pub fn render_plate_summary(p: &openreadout_core::plate::PlateSummary, images: usize) -> String {
    let mut s = format!(
        "  plate: {}{}  {} x {} wells, {} imaged, up to {} field(s) per well ({} images; the first is shown, all in --json)\n",
        p.id.as_deref().unwrap_or("(no id)"),
        p.plate_type
            .as_deref()
            .map(|t| format!(" ({t})"))
            .unwrap_or_default(),
        p.rows,
        p.columns,
        p.wells.len(),
        p.field_count,
        images
    );
    let names: Vec<&str> = p.wells.iter().map(|w| w.well.as_str()).collect();
    let list = if names.len() > 24 {
        format!(
            "{} ... {}",
            names[..12].join(" "),
            names[names.len() - 6..].join(" ")
        )
    } else {
        names.join(" ")
    };
    s.push_str(&format!("  wells: {list}\n"));
    if p.complete {
        s.push_str(&format!(
            "  complete: every plane file the index names is present{}\n",
            if p.planes_absent > 0 {
                format!(" ({} planes not acquired or not recorded)", p.planes_absent)
            } else {
                String::new()
            }
        ));
    } else {
        s.push_str(&format!(
            "  INCOMPLETE: {} of {} planes have no file on disk (`check` lists them per well)\n",
            p.planes_missing, p.planes_expected
        ));
    }
    s
}

/// `stats --per well`.
pub fn by_well(
    ds: &mut dyn Dataset,
    req: &WellStatsRequest,
    ctx: &ReadContext<'_>,
) -> Result<WellStatsOutput> {
    let info = ds.info()?;
    well_stats(ds, &info, req, ctx)
}

fn num(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(x) if x.fract() == 0.0 && x.abs() < 1e15 => format!("{x:.0}"),
        Some(x) => format!("{x:.3}"),
    }
}

impl super::batch::Item for WellStatsOutput {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(format!("{} well rows", self.rows.len()))
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.to_string();
    }
}

/// Human rendering of `stats --per well`.
pub fn render_by_well(o: &WellStatsOutput) -> String {
    let head = [
        "well", "field", "c", "channel", "fields", "planes", "missing", "mean", "std", "min",
        "median", "max",
    ];
    let align = [
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
    ];
    let rows: Vec<Vec<String>> = o
        .rows
        .iter()
        .map(|r| {
            vec![
                r.well.clone(),
                r.field.map_or_else(|| "all".into(), |f| f.to_string()),
                r.c.to_string(),
                r.channel.clone().unwrap_or_default(),
                r.fields.to_string(),
                r.planes.to_string(),
                r.planes_missing.to_string(),
                num(r.mean),
                num(r.std),
                num(r.min),
                num(r.median),
                num(r.max),
            ]
        })
        .collect();
    let mut s = format!(
        "{} ({}){}\n",
        o.path,
        o.format,
        o.plate
            .as_deref()
            .map(|p| format!("  plate {p}"))
            .unwrap_or_default()
    );
    s.push_str(&ui::table(&head, &align, &rows));
    for n in &o.notes {
        s.push_str(&format!("\nnote: {n}"));
    }
    if !o.wells_without_data.is_empty() {
        s.push_str(&format!(
            "\nwells without a readable plane: {}",
            o.wells_without_data.join(", ")
        ));
    }
    s
}

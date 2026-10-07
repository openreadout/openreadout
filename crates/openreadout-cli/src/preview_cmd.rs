//! `openreadout preview`: render a PNG/JPEG preview of an image, trace, spectrum or plate table.

use std::io::Write;
use std::path::PathBuf;

use openreadout_core::{Error, Registry, Result};
use openreadout_preview::{
    Axes, Axis, Contrast, Encoding, Lut, PreviewOutput, PreviewRequest, default_output, finish,
    render, write_verified,
};

use crate::output::{emit, fail};

/// Arguments of `preview`.
#[derive(Debug, clap::Args)]
pub struct PreviewArgs {
    pub file: PathBuf,
    #[command(flatten)]
    pub process: crate::commands::nmr::ProcessArgs,
    /// Output file (`.png` or `.jpg`); `-` writes the image to stdout. Default:
    /// `<input stem>.preview.png` next to the input.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Image index (see `info`). Default 0.
    #[arg(long)]
    pub image: Option<u32>,
    /// Plane selection: `c=1`, `z=4`, `t=0`, or combined `c=0,1,z=2`. Repeatable. Default: c=0,
    /// the middle z, t=0. Several channels imply `--composite`.
    #[arg(long = "select")]
    pub select: Vec<String>,
    /// Maximum-intensity projection over `z` (or `t`), limited to the selected range if any.
    #[arg(long, value_name = "AXIS")]
    pub mip: Option<String>,
    /// Blend all (or the selected) channels additively in their colours.
    #[arg(long)]
    pub composite: bool,
    /// Pyramid level to read. Default: the level nearest `--max-size`.
    #[arg(long)]
    pub level: Option<u32>,
    /// Only this rectangle, `X,Y,WIDTH,HEIGHT`: in full-resolution pixels (the level is then
    /// chosen so the region renders at about `--max-size`), or in the pixels of `--level` when
    /// one is given. Zoom into a whole-slide image without reading all of it.
    #[arg(long, value_name = "X,Y,W,H")]
    pub region: Option<String>,
    /// Image previews: `rulers` (coordinate rulers in full-resolution pixels and a scale bar),
    /// `grid` (rulers plus faint grid lines over the data at the ruler ticks) or `none` (the
    /// bare downsampled plane).
    #[arg(long, default_value = "rulers", value_name = "rulers|grid|none")]
    pub axes: String,
    /// Longest side of the output in pixels (16..=8192), rulers included.
    #[arg(long, default_value_t = openreadout_preview::DEFAULT_PREVIEW_SIZE)]
    pub max_size: u32,
    /// `auto` (0.1–99.9 percentiles; raw for 8-bit RGB), `min-max`, `percentile:LO,HI` or `raw`.
    #[arg(long, default_value = "auto")]
    pub contrast: String,
    /// `gray` or `channel-color`. Default: gray for one channel, channel colours for composites.
    #[arg(long)]
    pub lut: Option<String>,
    /// `png` or `jpeg`. Default: from the output extension, else png.
    #[arg(long)]
    pub format: Option<String>,
    /// JPEG quality (1–100).
    #[arg(long, default_value_t = openreadout_preview::DEFAULT_JPEG_QUALITY)]
    pub quality: u8,
    /// Trace preview: trace index (see `info` → `traces[]`).
    #[arg(long)]
    pub trace: Option<u32>,
    /// Trace preview: sweep index.
    #[arg(long)]
    pub sweep: Option<u32>,
    /// Trace preview: channel index; repeatable. Default: the first 8.
    #[arg(long = "channel")]
    pub channels: Vec<u32>,
    /// Spectrum preview: run index.
    #[arg(long)]
    pub run: Option<u32>,
    /// Spectrum preview: zero-based spectrum index.
    #[arg(long, conflicts_with = "scan")]
    pub spectrum: Option<u64>,
    /// Spectrum preview: instrument scan number.
    #[arg(long)]
    pub scan: Option<u64>,
    /// Spectrum preview: the instrument's centroid list instead of the profile.
    #[arg(long)]
    pub centroid: bool,
    /// Plate preview: table index.
    #[arg(long)]
    pub table: Option<u32>,
    /// Plate preview (long layout): value column name.
    #[arg(long)]
    pub column: Option<String>,
    /// Replace an existing output file.
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long)]
    pub json: bool,
}

impl PreviewArgs {
    /// The library request these arguments describe.
    pub fn request(&self) -> Result<PreviewRequest> {
        Ok({
            let mut preview_request = PreviewRequest::default();
            preview_request.image = self.image;
            preview_request.select = self.select.clone();
            preview_request.mip = self.mip.as_deref().map(str::parse::<Axis>).transpose()?;
            preview_request.composite = self.composite;
            preview_request.level = self.level;
            preview_request.region = self
                .region
                .as_deref()
                .map(openreadout_core::Region::parse)
                .transpose()?;
            preview_request.max_size = self.max_size;
            preview_request.contrast = self.contrast.parse::<Contrast>()?;
            preview_request.lut = self.lut.as_deref().map(str::parse::<Lut>).transpose()?;
            preview_request.trace = self.trace;
            preview_request.sweep = self.sweep;
            preview_request.channels = self.channels.clone();
            preview_request.run = self.run;
            preview_request.spectrum = self.spectrum;
            preview_request.scan = self.scan;
            preview_request.centroid = self.centroid;
            preview_request.table = self.table;
            preview_request.column = self.column.clone();
            self.axes.parse::<Axes>()?.apply(&mut preview_request);
            preview_request
        })
    }
}

fn run_preview(reg: &Registry, a: &PreviewArgs) -> Result<(PreviewOutput, Option<Vec<u8>>)> {
    let req = a.request()?;
    let to_stdout = a.output.as_deref().is_some_and(|p| p.as_os_str() == "-");
    if to_stdout && a.json {
        return Err(Error::Usage(
            "`-o -` writes the image to stdout; drop --json or write to a file".into(),
        ));
    }
    let encoding = match &a.format {
        Some(f) => f.parse::<Encoding>()?,
        None => a
            .output
            .as_deref()
            .filter(|_| !to_stdout)
            .map_or(Encoding::Png, Encoding::from_path),
    };
    let (_, ds) = reg.open(&a.file)?;
    let mut ds = a.process.wrap(ds)?;
    let info = ds.info()?;
    let rendered = render(ds.as_mut(), &info, &req)?;
    let (mut out, bytes) = finish(&rendered, encoding, a.quality)?;
    if to_stdout {
        return Ok((out, Some(bytes)));
    }
    let output = a
        .output
        .clone()
        .unwrap_or_else(|| default_output(&a.file, encoding));
    out.verified = write_verified(&a.file, &output, &bytes, a.overwrite)?;
    out.output = Some(output.display().to_string());
    out.hint = Some(look_hint(&out, &output.display().to_string()));
    Ok((out, None))
}

/// How to look at the written picture, and how to zoom into it.
fn look_hint(o: &PreviewOutput, path: &str) -> String {
    match &o.image {
        Some(im) if im.axes => format!(
            "view it: open (or Read) {path}; zoom with --region X,Y,W,H in full-res px read off the rulers (now showing {})",
            im.full_res_region
        ),
        Some(im) => format!(
            "view it: open (or Read) {path}; zoom with --region X,Y,W,H in full-res px (now showing {})",
            im.full_res_region
        ),
        None => format!("view it: open (or Read) {path}"),
    }
}

fn human(o: &PreviewOutput) -> String {
    let what = if let Some(im) = &o.image {
        format!(
            "image {} level {}{} c={:?} z={:?} t={:?}{}{} contrast {}",
            im.image,
            im.level,
            im.region
                .map(|r| format!(" region {r}"))
                .unwrap_or_default(),
            im.c,
            im.z,
            im.t,
            im.projection
                .as_deref()
                .map(|p| format!(" {p}"))
                .unwrap_or_default(),
            if im.composite { " composite" } else { "" },
            im.contrast
        )
    } else if let Some(t) = &o.trace {
        format!(
            "trace {} sweep {} ({} of {} samples, {} channels)",
            t.trace,
            t.sweep,
            t.sample_count,
            t.sweep_sample_count,
            t.channels.len()
        )
    } else if let Some(s) = &o.spectrum {
        format!(
            "spectrum {} (scan {}, MS{}, {} points, {})",
            s.index, s.scan_number, s.ms_level, s.point_count, s.style
        )
    } else if let Some(p) = &o.plate {
        format!(
            "plate {}x{} from table {} ({} wells, {})",
            p.rows, p.columns, p.table, p.wells, p.value
        )
    } else {
        String::new()
    };
    let mut s = format!(
        "wrote {} ({}x{} {}, {} bytes, verified={}): {what}",
        o.output.as_deref().unwrap_or("-"),
        o.width,
        o.height,
        o.encoding,
        o.bytes,
        o.verified
    );
    for n in &o.notes {
        s.push_str(&format!("\n  note: {n}"));
    }
    if let Some(h) = &o.hint {
        s.push_str(&format!("\n{h}"));
    }
    s
}

/// Run `preview`; returns the exit code.
pub fn run(reg: &Registry, a: &PreviewArgs) -> i32 {
    match run_preview(reg, a) {
        Ok((_, Some(bytes))) => {
            let mut so = std::io::stdout().lock();
            if so.write_all(&bytes).and_then(|()| so.flush()).is_err() {
                return 5;
            }
            0
        }
        Ok((out, None)) => emit(a.json, &out, human),
        Err(e) => fail(a.json, &e),
    }
}

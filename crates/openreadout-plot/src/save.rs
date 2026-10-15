//! Saving a figure (a plot or a preview) to a file: PNG, JPEG and TIFF at a pixel scale, with
//! the print resolution recorded when one is given, or SVG and PDF as vectors. Every write goes
//! through a temporary file that is read back and compared before it is renamed into place.

use std::io::Cursor;
use std::path::Path;

use openreadout_core::{Error, Result};
use openreadout_figure::{Figure, Raster};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File formats a figure can be saved as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// PNG (lossless).
    Png,
    /// JPEG (quality 92).
    Jpeg,
    /// TIFF (RGB, LZW), the format many journals ask for.
    Tiff,
    /// SVG (vector; text stays text).
    Svg,
    /// PDF (vector; text as outlines).
    Pdf,
}

/// The extensions [`FileFormat::from_path`] knows, for messages.
pub const EXTENSIONS: &str = ".png, .jpg, .tiff, .svg or .pdf";

impl FileFormat {
    /// From a path's extension.
    pub fn from_path(p: &Path) -> Result<FileFormat> {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        ext.as_deref()
            .and_then(FileFormat::from_name)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "figure file `{}`: name it {EXTENSIONS}",
                    p.display()
                ))
            })
    }

    /// From a name or extension (`png`, `jpg`/`jpeg`, `tif`/`tiff`, `svg`, `pdf`).
    pub fn from_name(s: &str) -> Option<FileFormat> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "png" => FileFormat::Png,
            "jpg" | "jpeg" => FileFormat::Jpeg,
            "tif" | "tiff" => FileFormat::Tiff,
            "svg" => FileFormat::Svg,
            "pdf" => FileFormat::Pdf,
            _ => return None,
        })
    }

    /// `png`, `jpeg`, `tiff`, `svg` or `pdf`.
    pub fn as_str(self) -> &'static str {
        match self {
            FileFormat::Png => "png",
            FileFormat::Jpeg => "jpeg",
            FileFormat::Tiff => "tiff",
            FileFormat::Svg => "svg",
            FileFormat::Pdf => "pdf",
        }
    }

    /// The usual extension (`png`, `jpg`, `tiff`, `svg`, `pdf`).
    pub fn extension(self) -> &'static str {
        match self {
            FileFormat::Jpeg => "jpg",
            f => f.as_str(),
        }
    }

    /// True for the vector formats (SVG, PDF).
    pub fn is_vector(self) -> bool {
        matches!(self, FileFormat::Svg | FileFormat::Pdf)
    }
}

/// Default pixel scale of saved PNG, JPEG and TIFF files.
pub const DEFAULT_SCALE: f32 = 2.0;
/// Largest pixel scale.
pub const MAX_SCALE: f32 = 8.0;
/// Most pixels a saved raster may have (about 100 megapixels).
pub const MAX_PIXELS: u64 = 100_000_000;

/// How to save a figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaveOptions {
    /// Pixels per figure unit of PNG, JPEG and TIFF files (default 2; 0.25 to 8). Text and
    /// lines are drawn again at this size, so they stay sharp.
    pub scale: f32,
    /// Print resolution in dots per inch. Raster files record it, so they open at
    /// pixels ÷ dpi inches; PDF and SVG pages get the size the raster would print at
    /// (figure units × scale ÷ dpi inches).
    pub dpi: Option<u32>,
    /// Replace an existing file.
    pub overwrite: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        SaveOptions {
            scale: DEFAULT_SCALE,
            dpi: None,
            overwrite: false,
        }
    }
}

impl SaveOptions {
    fn check(&self) -> Result<()> {
        if !(self.scale.is_finite() && (0.25..=MAX_SCALE).contains(&self.scale)) {
            return Err(Error::Usage(format!(
                "scale {} is out of range: use 0.25 to {MAX_SCALE} (2 is the default; for print, give dpi as well)",
                self.scale
            )));
        }
        if let Some(d) = self.dpi
            && !(36..=2400).contains(&d)
        {
            return Err(Error::Usage(format!(
                "dpi {d} is out of range: use 36 to 2400 (300 is usual for print)"
            )));
        }
        Ok(())
    }
}

/// A figure file written.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct WrittenFile {
    /// Path.
    pub path: String,
    /// `png`, `jpeg`, `tiff`, `svg` or `pdf`.
    pub format: String,
    /// Width in pixels (PNG, JPEG, TIFF) or figure units (SVG, PDF).
    pub width: u32,
    /// Height, as `width`.
    pub height: u32,
    /// Print resolution recorded in the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    /// Printed size `[width, height]` in inches, when `dpi` is given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_size_in: Option<[f64; 2]>,
    /// Size in bytes.
    pub bytes: u64,
    /// Read back and compared after writing.
    pub verified: bool,
}

/// Encode `fig` as `format`. Returns the bytes and the size in pixels (figure units for
/// vectors).
pub fn encode(fig: &Figure, format: FileFormat, o: &SaveOptions) -> Result<(Vec<u8>, u32, u32)> {
    o.check()?;
    let (fw, fh) = (fig.width.max(1.0), fig.height.max(1.0));
    if format.is_vector() {
        // A page as large as the raster would print; without dpi one unit is one point (PDF)
        // or one pixel (SVG).
        let inches = o.dpi.map(|d| o.scale / d as f32);
        let bytes = match format {
            FileFormat::Svg => {
                let svg = fig.to_svg();
                match inches {
                    Some(k) => sized_svg(&svg, fw * k, fh * k),
                    None => svg,
                }
                .into_bytes()
            }
            _ => crate::pdf::pdf(fig, inches.map_or(1.0, |k| k * 72.0)),
        };
        return Ok((bytes, fw.round() as u32, fh.round() as u32));
    }
    let px = (f64::from(fw) * f64::from(o.scale)).round() as u64
        * (f64::from(fh) * f64::from(o.scale)).round() as u64;
    if px > MAX_PIXELS {
        return Err(Error::Usage(format!(
            "a {fw:.0} × {fh:.0} figure at scale {} is {px} pixels, over the {MAX_PIXELS} limit: use a smaller scale",
            o.scale
        )));
    }
    let r = fig.to_raster(o.scale);
    let bytes = match format {
        FileFormat::Png => png(&r, o.dpi)?,
        FileFormat::Jpeg => jpeg(&r, o.dpi)?,
        _ => tiff(&r, o.dpi)?,
    };
    Ok((bytes, r.width, r.height))
}

/// Write `fig` to `path` (format from the extension) through a temporary file that is read back
/// and compared before it is renamed into place. Refuses to replace an existing file unless
/// `overwrite`, and creates the folder when it is missing.
pub fn write(fig: &Figure, path: &Path, o: &SaveOptions) -> Result<WrittenFile> {
    let format = FileFormat::from_path(path)?;
    let (bytes, width, height) = encode(fig, format, o)?;
    openreadout_assay::write_verified(path, &bytes, o.overwrite)?;
    let print_size_in = o.dpi.map(|d| {
        let k = if format.is_vector() {
            f64::from(o.scale)
        } else {
            1.0
        };
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        [
            r2(f64::from(width) * k / f64::from(d)),
            r2(f64::from(height) * k / f64::from(d)),
        ]
    });
    Ok(WrittenFile {
        path: path.display().to_string(),
        format: format.as_str().into(),
        width,
        height,
        dpi: o.dpi,
        print_size_in,
        bytes: bytes.len() as u64,
        verified: true,
    })
}

/// An SVG document whose root says its printed size in inches (the viewBox keeps the drawing's
/// own units).
fn sized_svg(svg: &str, w_in: f32, h_in: f32) -> String {
    let Some(end) = svg.find('>') else {
        return svg.to_string();
    };
    let (head, rest) = svg.split_at(end);
    let mut head = head.to_string();
    for (attr, v) in [("width", w_in), ("height", h_in)] {
        let key = format!(" {attr}=\"");
        if let Some(i) = head.find(&key) {
            let start = i + key.len();
            if let Some(len) = head[start..].find('"') {
                head.replace_range(start..start + len, &format!("{v:.3}in"));
            }
        }
    }
    head + rest
}

fn png(r: &Raster, dpi: Option<u32>) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let gray = r.is_gray();
    {
        let mut e = png::Encoder::new(&mut buf, r.width, r.height);
        e.set_color(if gray {
            png::ColorType::Grayscale
        } else {
            png::ColorType::Rgb
        });
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::Balanced);
        if let Some(d) = dpi {
            // pixels per metre, rounded
            let ppm = (f64::from(d) / 0.0254).round() as u32;
            e.set_pixel_dims(Some(png::PixelDimensions {
                xppu: ppm,
                yppu: ppm,
                unit: png::Unit::Meter,
            }));
        }
        let bad = |e: png::EncodingError| Error::Other(format!("PNG encoding failed: {e}"));
        let mut w = e.write_header().map_err(bad)?;
        let data: Vec<u8> = if gray {
            r.rgb.as_chunks::<3>().0.iter().map(|p| p[0]).collect()
        } else {
            r.rgb.clone()
        };
        w.write_image_data(&data).map_err(bad)?;
        w.finish().map_err(bad)?;
    }
    Ok(buf)
}

fn jpeg(r: &Raster, dpi: Option<u32>) -> Result<Vec<u8>> {
    let mut b = r.to_jpeg(92)?;
    // JFIF APP0: FFD8 FFE0 len "JFIF\0" version units Xdensity Ydensity
    if let Some(d) = dpi
        && b.len() > 18
        && b[2..4] == [0xFF, 0xE0]
        && &b[6..11] == b"JFIF\0"
    {
        let d = u16::try_from(d).unwrap_or(u16::MAX).to_be_bytes();
        b[13] = 1;
        b[14..16].copy_from_slice(&d);
        b[16..18].copy_from_slice(&d);
    }
    Ok(b)
}

fn tiff(r: &Raster, dpi: Option<u32>) -> Result<Vec<u8>> {
    use tiff::encoder::{Compression, Predictor, Rational, TiffEncoder, colortype};
    let bad = |e: tiff::TiffError| Error::Other(format!("TIFF encoding failed: {e}"));
    let mut buf = Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut buf)
            .map_err(bad)?
            .with_compression(Compression::Lzw)
            .with_predictor(Predictor::Horizontal);
        let mut im = enc
            .new_image::<colortype::RGB8>(r.width, r.height)
            .map_err(bad)?;
        im.resolution(
            tiff::tags::ResolutionUnit::Inch,
            Rational {
                n: dpi.unwrap_or(72),
                d: 1,
            },
        );
        im.write_data(&r.rgb).map_err(bad)?;
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod tests {
    use openreadout_figure::{Stroke, TextStyle, Theme};

    use super::*;

    fn fig() -> Figure {
        let t = Theme::light();
        let mut f = Figure::new(200.0, 100.0, Some(t.paper));
        f.line(10.0, 10.0, 190.0, 90.0, Stroke::new(t.accent, 2.0));
        f.text(20.0, 50.0, "OD600", &TextStyle::new(14.0, t.ink));
        f
    }

    #[test]
    fn every_format_with_scale_and_dpi() {
        let f = fig();
        let o = SaveOptions {
            scale: 3.0,
            dpi: Some(300),
            overwrite: false,
        };
        let (png, w, h) = encode(&f, FileFormat::Png, &o).unwrap();
        assert_eq!((w, h), (600, 300));
        assert!(png.starts_with(b"\x89PNG"));
        assert!(png.windows(4).any(|x| x == b"pHYs"));
        let (jpg, ..) = encode(&f, FileFormat::Jpeg, &o).unwrap();
        assert_eq!((jpg[13], &jpg[14..16]), (1, &300u16.to_be_bytes()[..]));
        let (tif, ..) = encode(&f, FileFormat::Tiff, &o).unwrap();
        assert!(tif.starts_with(b"II*\0") || tif.starts_with(b"MM\0*"));
        let mut d = tiff::decoder::Decoder::new(Cursor::new(&tif)).unwrap();
        assert_eq!(d.dimensions().unwrap(), (600, 300));
        let (svg, ..) = encode(&f, FileFormat::Svg, &o).unwrap();
        let svg = String::from_utf8(svg).unwrap();
        assert!(svg.contains("width=\"2.000in\""), "{}", &svg[..200]);
        assert!(svg.contains("viewBox=\"0 0 200 100\""));
        let (pdf, ..) = encode(&f, FileFormat::Pdf, &o).unwrap();
        assert!(String::from_utf8_lossy(&pdf).contains("/MediaBox [0 0 144 72]"));
    }

    #[test]
    fn writes_verified_and_refuses_bad_options() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub/fig.tiff");
        let o = SaveOptions {
            dpi: Some(300),
            ..SaveOptions::default()
        };
        let w = write(&fig(), &p, &o).unwrap();
        assert!(w.verified);
        assert_eq!((w.width, w.print_size_in), (400, Some([1.33, 0.67])));
        assert!(
            write(&fig(), &p, &o).is_err(),
            "never over a file without overwrite"
        );
        assert!(write(&fig(), &d.path().join("fig.bmp"), &o).is_err());
        for bad in [
            SaveOptions { scale: 0.0, ..o },
            SaveOptions { dpi: Some(5), ..o },
        ] {
            assert!(encode(&fig(), FileFormat::Png, &bad).is_err());
        }
    }
}

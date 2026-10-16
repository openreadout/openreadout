//! Views of a volume: an anatomical plane (axial, coronal or sagittal, oriented the way
//! radiology shows them), a plane through the stack (`xz`, `yz`), or the three orthogonal
//! planes side by side. A view is a [`Dataset`] over the source: its planes are the slices of
//! the view, so contrast, projections, scale bars and annotations work on it unchanged.
//!
//! Orientation comes from `images[].extra.orientation` (`columns_toward`, `rows_toward`,
//! `slices_toward`: the patient direction, in the letters L/R, A/P, S/I, that columns, rows and
//! slices advance toward), which the DICOM and NIfTI readers fill. Radiological display: the
//! patient's left on the right of the picture; anterior at the top of axial slices; superior at
//! the top of coronal and sagittal ones; anterior on the left of sagittal ones.

use std::collections::HashMap;

use openreadout_core::model::{CheckReport, FileInfo, ImageInfo, LsEntry};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{Error, PixelType, Plane, Result};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::annotate::{Annotation, AnnotationType};

/// Which plane of a volume an image preview shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum View {
    /// the planes as stored
    Native,
    /// head-foot slices, patient's left on the right, anterior up
    Axial,
    /// front-back slices, patient's left on the right, head up
    Coronal,
    /// left-right slices, anterior on the left, head up
    Sagittal,
    /// axial, coronal and sagittal planes through the volume's centre (or the selected z)
    Orthogonal,
    /// columns against z: a cut along the stack
    Xz,
    /// rows against z: a cut along the stack
    Yz,
}

impl std::str::FromStr for View {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "native" | "xy" => View::Native,
            "axial" | "transverse" => View::Axial,
            "coronal" => View::Coronal,
            "sagittal" => View::Sagittal,
            "orthogonal" | "ortho" | "mpr" => View::Orthogonal,
            "xz" => View::Xz,
            "yz" => View::Yz,
            other => {
                return Err(Error::Usage(format!(
                    "bad view '{other}': use native, axial, coronal, sagittal, orthogonal, xz or yz"
                )));
            }
        })
    }
}

impl View {
    fn name(self) -> &'static str {
        match self {
            View::Native => "native",
            View::Axial => "axial",
            View::Coronal => "coronal",
            View::Sagittal => "sagittal",
            View::Orthogonal => "orthogonal",
            View::Xz => "xz",
            View::Yz => "yz",
        }
    }
}

/// A display axis: the data axis (0 x, 1 y, 2 z) it walks and whether it walks it backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ax {
    data: usize,
    flip: bool,
}

/// The display axes of a view: columns (u), rows (v) and slices (w).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame3 {
    u: Ax,
    v: Ax,
    w: Ax,
}

const IDENTITY: Frame3 = Frame3 {
    u: Ax {
        data: 0,
        flip: false,
    },
    v: Ax {
        data: 1,
        flip: false,
    },
    w: Ax {
        data: 2,
        flip: false,
    },
};

/// The anatomical axis (0 left-right, 1 front-back, 2 head-foot) of a direction letter, and
/// whether it is the positive (L, P, S) direction.
fn letter(c: &str) -> Option<(usize, bool)> {
    Some(match c {
        "L" => (0, true),
        "R" => (0, false),
        "P" => (1, true),
        "A" => (1, false),
        "S" => (2, true),
        "I" => (2, false),
        _ => return None,
    })
}

/// For each data axis (x, y, z), its anatomical axis and direction, when the image records
/// them all and they are a permutation.
fn anatomy(im: &ImageInfo) -> Option<[(usize, bool); 3]> {
    let o = im.extra.get("orientation")?;
    let get = |k: &str| o.get(k).and_then(|v| v.as_str()).and_then(letter);
    let x = get("columns_toward")?;
    let y = get("rows_toward")?;
    let z = match get("slices_toward") {
        Some(z) => z,
        // a single slice: the remaining axis
        None => (3 - x.0 - y.0, true),
    };
    let mut seen = [x.0, y.0, z.0];
    seen.sort_unstable();
    (seen == [0, 1, 2]).then_some([x, y, z])
}

/// The display frame of an anatomical view: (horizontal axis, positive-to-the-right?),
/// (vertical axis, positive-downward?), slice axis.
fn anatomical_frame(view: View, an: [(usize, bool); 3]) -> Option<Frame3> {
    // (anatomical axis shown left-to-right, its positive letter goes right?), (top-to-bottom, positive goes down?), normal
    let (h, vv, n) = match view {
        // L to the right, P down, slices along S
        View::Axial => ((0, true), (1, true), 2),
        // L to the right, I down (S up)
        View::Coronal => ((0, true), (2, false), 1),
        // P to the right (A on the left), I down
        View::Sagittal => ((1, true), (2, false), 0),
        _ => return None,
    };
    let data_of = |a: usize| an.iter().position(|(x, _)| *x == a);
    let axis = |(a, pos_right): (usize, bool)| -> Option<Ax> {
        let d = data_of(a)?;
        // the data axis advances toward the positive letter when an[d].1
        let advances_positive = an[d].1;
        Some(Ax {
            data: d,
            flip: advances_positive != pos_right,
        })
    };
    let u = axis(h)?;
    let v = axis(vv)?;
    let w = Ax {
        data: data_of(n)?,
        flip: false,
    };
    Some(Frame3 { u, v, w })
}

/// The anatomical plane of an image's own slices (`orientation.plane`), when it is one.
fn acquisition_view(im: &ImageInfo) -> Option<View> {
    let p = im.extra.get("orientation")?.get("plane")?.as_str()?;
    Some(match p {
        "axial" => View::Axial,
        "coronal" => View::Coronal,
        "sagittal" => View::Sagittal,
        _ => return None,
    })
}

/// The view a preview of `im` uses: the requested one, else (for medical images whose
/// orientation is recorded) the anatomical plane they were acquired in, shown the radiology
/// way. `None` keeps the stored planes.
pub(crate) fn effective(info: &FileInfo, im: &ImageInfo, req: Option<View>) -> Option<View> {
    match req {
        Some(View::Native) => None,
        Some(v) => Some(v),
        None if info.format.family == "medical-imaging" && im.pyramid_levels <= 1 => {
            let v = acquisition_view(im)?;
            let an = anatomy(im)?;
            // only when it changes the picture (a flip or a transpose)
            (anatomical_frame(v, an)? != IDENTITY).then_some(v)
        }
        None => None,
    }
}

/// Physical step (µm) along each data axis.
fn steps(im: &ImageInfo) -> [Option<f64>; 3] {
    [im.physical_size.x, im.physical_size.y, im.physical_size.z]
}

fn sizes(im: &ImageInfo) -> [u32; 3] {
    [im.size_x, im.size_y, im.size_z]
}

/// Most bytes of a volume a view holds in memory.
const MAX_VOLUME_BYTES: u64 = 2 << 30;

/// The stored planes of one (c, t), cached for cuts through the stack.
struct Volume {
    planes: Vec<Vec<u8>>,
    pixel_type: PixelType,
    spp: u32,
}

/// A view over a dataset's image.
pub(crate) struct ViewDataset<'a> {
    inner: &'a mut dyn Dataset,
    image: u32,
    src: ImageInfo,
    frame: Frame3,
    view: View,
    cache: HashMap<(u32, u32), Volume>,
    /// Orthogonal views: the crosshair point (data voxel) and the panel layout.
    ortho: Option<Ortho>,
}

/// Layout of the orthogonal composite: three panels left to right on a square-µm grid.
#[derive(Debug, Clone)]
struct Ortho {
    point: [u32; 3],
    /// µm per composite pixel.
    step: f64,
    /// Per panel: view, frame, x offset, width, height in composite pixels.
    panels: Vec<(View, Frame3, u32, u32, u32)>,
    width: u32,
    height: u32,
}

const GAP: u32 = 8;

impl<'a> ViewDataset<'a> {
    /// A view of `image` of `info`, or an error saying why the image has no such view.
    pub(crate) fn new(
        inner: &'a mut dyn Dataset,
        info: &FileInfo,
        image: u32,
        view: View,
        select_z: Option<u32>,
    ) -> Result<(ViewDataset<'a>, ImageInfo, Vec<Annotation>, Vec<String>)> {
        let src = info
            .images
            .iter()
            .find(|i| i.index == image)
            .cloned()
            .ok_or_else(|| Error::Usage(format!("image {image} not found")))?;
        let mut notes = Vec::new();
        let an = anatomy(&src);
        let frame_for = |v: View, notes: &mut Vec<String>| -> Result<Frame3> {
            Ok(match v {
                View::Native | View::Orthogonal => IDENTITY,
                View::Xz => Frame3 {
                    u: Ax {
                        data: 0,
                        flip: false,
                    },
                    v: Ax {
                        data: 2,
                        flip: false,
                    },
                    w: Ax {
                        data: 1,
                        flip: false,
                    },
                },
                View::Yz => Frame3 {
                    u: Ax {
                        data: 1,
                        flip: false,
                    },
                    v: Ax {
                        data: 2,
                        flip: false,
                    },
                    w: Ax {
                        data: 0,
                        flip: false,
                    },
                },
                View::Axial | View::Coronal | View::Sagittal => {
                    if let Some(f) = an.and_then(|a| anatomical_frame(v, a)) {
                        f
                    } else {
                        {
                            // no recorded orientation: the stack's own axes, z taken as head-foot
                            notes.push(format!(
                            "the image records no patient orientation: the {} view is the stack's {} plane",
                            v.name(),
                            match v {
                                View::Axial => "xy",
                                View::Coronal => "xz",
                                _ => "yz",
                            }
                        ));
                            match v {
                                View::Axial => IDENTITY,
                                View::Coronal => Frame3 {
                                    u: Ax {
                                        data: 0,
                                        flip: false,
                                    },
                                    v: Ax {
                                        data: 2,
                                        flip: true,
                                    },
                                    w: Ax {
                                        data: 1,
                                        flip: false,
                                    },
                                },
                                _ => Frame3 {
                                    u: Ax {
                                        data: 1,
                                        flip: false,
                                    },
                                    v: Ax {
                                        data: 2,
                                        flip: true,
                                    },
                                    w: Ax {
                                        data: 0,
                                        flip: false,
                                    },
                                },
                            }
                        }
                    }
                }
            })
        };
        let n = sizes(&src);
        let needs_stack = |f: &Frame3| f.w.data != 2;
        let frame = frame_for(view, &mut notes)?;
        if (needs_stack(&frame) || view == View::Orthogonal) && n[2] < 2 {
            return Err(Error::Usage(format!(
                "the {} view needs a volume; image {image} has a single z plane",
                view.name()
            )));
        }
        if src.samples_per_pixel > 1 && (needs_stack(&frame) || view == View::Orthogonal) {
            return Err(Error::Usage(
                "views across the stack are drawn for single-sample (grayscale) images".into(),
            ));
        }
        let st = steps(&src);
        let mut out = src.clone();
        out.resolution_levels.clear();
        out.pyramid_levels = 1;
        let mut annotations = Vec::new();
        let mut ortho = None;
        if view == View::Orthogonal {
            // panels on a square grid of the finest step
            let s = st.iter().flatten().copied().fold(f64::INFINITY, f64::min);
            let s = if s.is_finite() && s > 0.0 { s } else { 1.0 };
            let ext = |d: usize| f64::from(n[d]) * st[d].unwrap_or(s);
            let mut step = s;
            // keep the composite to about 2048 px wide
            let total = |step: f64| -> f64 {
                [View::Axial, View::Coronal, View::Sagittal]
                    .iter()
                    .filter_map(|v| frame_for(*v, &mut Vec::new()).ok())
                    .map(|f| ext(f.u.data) / step)
                    .sum::<f64>()
            };
            while total(step) > 2048.0 {
                step *= 1.25;
            }
            let point = [
                n[0] / 2,
                n[1] / 2,
                select_z.unwrap_or(n[2] / 2).min(n[2] - 1),
            ];
            let mut panels = Vec::new();
            let mut x = 0u32;
            let mut height = 0u32;
            for v in [View::Axial, View::Coronal, View::Sagittal] {
                let f = frame_for(v, &mut notes)?;
                let w = (ext(f.u.data) / step).round().max(1.0) as u32;
                let h = (ext(f.v.data) / step).round().max(1.0) as u32;
                panels.push((v, f, x, w, h));
                x += w + GAP;
                height = height.max(h);
            }
            let width = x - GAP;
            out.size_x = width;
            out.size_y = height;
            out.size_z = 1;
            out.physical_size.x = st[0].is_some().then_some(step);
            out.physical_size.y = out.physical_size.x;
            out.physical_size.z = None;
            out.pixel_type = PixelType::Float;
            // panel names and crosshairs at the point
            for (v, f, x0, w, h) in &panels {
                // panels are centred vertically
                let y0 = f64::from((height - h) / 2);
                annotations.push(Annotation::outline(
                    f64::from(*x0),
                    y0,
                    f64::from(*w),
                    f64::from(*h),
                ));
                annotations.push(Annotation::text(
                    f64::from(*x0) + f64::from(*w) / 2.0,
                    y0 + 14.0,
                    v.name(),
                ));
                let pos = |a: Ax, len: u32| -> f64 {
                    let i = f64::from(point[a.data]) + 0.5;
                    let frac = i / f64::from(n[a.data]);
                    let frac = if a.flip { 1.0 - frac } else { frac };
                    frac * f64::from(len)
                };
                let cx = f64::from(*x0) + pos(f.u, *w);
                let cy = y0 + pos(f.v, *h);
                annotations.push(Annotation::line(cx, y0, cx, y0 + f64::from(*h)));
                annotations.push(Annotation::line(f64::from(*x0), cy, f64::from(*x0 + w), cy));
            }
            ortho = Some(Ortho {
                point,
                step,
                panels,
                width,
                height,
            });
        } else {
            out.size_x = n[frame.u.data];
            out.size_y = n[frame.v.data];
            out.size_z = n[frame.w.data];
            out.physical_size.x = st[frame.u.data];
            out.physical_size.y = st[frame.v.data];
            out.physical_size.z = st[frame.w.data];
            if view != View::Native && needs_stack(&frame) && st[2].is_none() {
                notes.push("the z step is not recorded: z is drawn with the xy pixel size".into());
                let fill = st[0].or(st[1]);
                if frame.u.data == 2 {
                    out.physical_size.x = fill;
                }
                if frame.v.data == 2 {
                    out.physical_size.y = fill;
                }
            }
        }
        out.plane_count = u64::from(out.size_z) * u64::from(out.size_c) * u64::from(out.size_t);
        out.extra
            .insert("view".into(), serde_json::Value::from(view.name()));
        Ok((
            ViewDataset {
                inner,
                image,
                src,
                frame,
                view,
                cache: HashMap::new(),
                ortho,
            },
            out,
            annotations,
            notes,
        ))
    }

    fn volume(&mut self, c: u32, t: u32) -> Result<&Volume> {
        if !self.cache.contains_key(&(c, t)) {
            let n = sizes(&self.src);
            let bpp = self.src.pixel_type.bytes_per_sample() as u64
                * u64::from(self.src.samples_per_pixel.max(1));
            let bytes = u64::from(n[0]) * u64::from(n[1]) * u64::from(n[2]) * bpp;
            if bytes > MAX_VOLUME_BYTES {
                return Err(Error::unsupported(
                    "preview",
                    format!("a view across a {bytes}-byte volume"),
                    "Cuts through the stack hold the whole volume in memory (2 GiB at most); preview the stored planes (view: native) or a region.",
                ));
            }
            // one cut at a time: drop other volumes first
            self.cache.clear();
            let mut planes = Vec::with_capacity(n[2] as usize);
            let mut pt = self.src.pixel_type;
            let mut spp = self.src.samples_per_pixel.max(1);
            for z in 0..n[2] {
                let p = self.inner.read_plane(self.image, PlaneIndex { c, z, t })?;
                pt = p.pixel_type;
                spp = p.samples_per_pixel.max(1);
                planes.push(p.data);
            }
            self.cache.insert(
                (c, t),
                Volume {
                    planes,
                    pixel_type: pt,
                    spp,
                },
            );
        }
        self.cache
            .get(&(c, t))
            .ok_or_else(|| Error::Other("volume cache".into()))
    }

    /// Plane `k` along `f.w` of (c, t), in the stored sample type.
    fn cut(&mut self, f: Frame3, k: u32, c: u32, t: u32) -> Result<Plane> {
        let n = sizes(&self.src);
        let (nu, nv) = (n[f.u.data], n[f.v.data]);
        if f.w.data == 2 && f.u.data == 0 && f.v.data == 1 {
            // a stored plane, maybe flipped
            let p = self
                .inner
                .read_plane(self.image, PlaneIndex { c, z: k, t })?;
            if !f.u.flip && !f.v.flip {
                return Ok(p);
            }
            let bpp = p.pixel_type.bytes_per_sample() * p.samples_per_pixel.max(1) as usize;
            let (w, h) = (p.width as usize, p.height as usize);
            let mut out = vec![0u8; p.data.len()];
            for y in 0..h {
                let sy = if f.v.flip { h - 1 - y } else { y };
                for x in 0..w {
                    let sx = if f.u.flip { w - 1 - x } else { x };
                    let s = (sy * w + sx) * bpp;
                    let d = (y * w + x) * bpp;
                    if let (Some(src), Some(dst)) =
                        (p.data.get(s..s + bpp), out.get_mut(d..d + bpp))
                    {
                        dst.copy_from_slice(src);
                    }
                }
            }
            return Ok(Plane { data: out, ..p });
        }
        let vol = self.volume(c, t)?;
        let bpp = vol.pixel_type.bytes_per_sample() * vol.spp as usize;
        let (pt, spp) = (vol.pixel_type, vol.spp);
        let mut out = vec![0u8; nu as usize * nv as usize * bpp];
        let mut idx = [0u32; 3];
        idx[f.w.data] = if f.w.flip { n[f.w.data] - 1 - k } else { k };
        for v in 0..nv {
            idx[f.v.data] = if f.v.flip { nv - 1 - v } else { v };
            for u in 0..nu {
                idx[f.u.data] = if f.u.flip { nu - 1 - u } else { u };
                let plane = &vol.planes[idx[2] as usize];
                let s = (idx[1] as usize * n[0] as usize + idx[0] as usize) * bpp;
                let d = (v as usize * nu as usize + u as usize) * bpp;
                if let (Some(src), Some(dst)) = (plane.get(s..s + bpp), out.get_mut(d..d + bpp)) {
                    dst.copy_from_slice(src);
                }
            }
        }
        Ok(Plane {
            width: nu,
            height: nv,
            pixel_type: pt,
            samples_per_pixel: spp,
            data: out,
        })
    }

    /// The orthogonal composite of (c, t) as float samples (NaN between panels).
    fn composite(&mut self, c: u32, t: u32) -> Result<Plane> {
        let o = self
            .ortho
            .clone()
            .ok_or_else(|| Error::Other("not orthogonal".into()))?;
        let n = sizes(&self.src);
        let mut out = vec![f32::NAN; o.width as usize * o.height as usize];
        for (_, f, x0, w, h) in &o.panels {
            let k = o.point[f.w.data];
            let p = self.cut(*f, k, c, t)?;
            let vals: Vec<f32> = p
                .pixel_type
                .samples_f64(&p.data)
                .map(|v| v as f32)
                .collect();
            let (pw, ph) = (p.width as usize, p.height as usize);
            let y_off = ((o.height - h) / 2) as usize;
            // bilinear resampling of the cut onto the panel's square-µm grid
            for y in 0..*h as usize {
                let fy = ((y as f64 + 0.5) / f64::from(*h) * ph as f64 - 0.5)
                    .clamp(0.0, (ph - 1) as f64);
                let (y0, ty) = (fy.floor() as usize, (fy - fy.floor()) as f32);
                let y1 = (y0 + 1).min(ph - 1);
                for x in 0..*w as usize {
                    let fx = ((x as f64 + 0.5) / f64::from(*w) * pw as f64 - 0.5)
                        .clamp(0.0, (pw - 1) as f64);
                    let (x0i, tx) = (fx.floor() as usize, (fx - fx.floor()) as f32);
                    let x1 = (x0i + 1).min(pw - 1);
                    let g =
                        |yy: usize, xx: usize| vals.get(yy * pw + xx).copied().unwrap_or(f32::NAN);
                    let top = g(y0, x0i) * (1.0 - tx) + g(y0, x1) * tx;
                    let bottom = g(y1, x0i) * (1.0 - tx) + g(y1, x1) * tx;
                    out[(y + y_off) * o.width as usize + *x0 as usize + x] =
                        top * (1.0 - ty) + bottom * ty;
                }
            }
        }
        let _ = n;
        Ok(Plane {
            width: o.width,
            height: o.height,
            pixel_type: PixelType::Float,
            samples_per_pixel: 1,
            data: out.iter().flat_map(|v| v.to_le_bytes()).collect(),
        })
    }

    /// Which view this is (for the preview's report).
    pub(crate) fn view(&self) -> View {
        self.view
    }

    /// The crosshair point of an orthogonal view, in data voxels.
    pub(crate) fn point(&self) -> Option<[u32; 3]> {
        self.ortho.as_ref().map(|o| o.point)
    }

    /// µm per composite pixel of an orthogonal view.
    pub(crate) fn ortho_step(&self) -> Option<f64> {
        self.ortho.as_ref().map(|o| o.step)
    }
}

impl Dataset for ViewDataset<'_> {
    fn info(&self) -> Result<FileInfo> {
        self.inner.info()
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        self.inner.vendor_metadata()
    }
    fn provenance(&self) -> ProvenanceMap {
        self.inner.provenance()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.inner.entries()
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if image != self.image {
            return self.inner.read_plane(image, index);
        }
        if self.ortho.is_some() {
            return self.composite(index.c, index.t);
        }
        let f = self.frame;
        self.cut(f, index.z, index.c, index.t)
    }
    fn check(&mut self) -> Result<CheckReport> {
        self.inner.check()
    }
}

fn bare(kind: AnnotationType) -> Annotation {
    Annotation {
        kind,
        x: None,
        y: None,
        x2: None,
        y2: None,
        width: None,
        height: None,
        points: Vec::new(),
        label: None,
        color: None,
        channel: None,
        well: None,
    }
}

impl Annotation {
    /// A label alone at (x, y).
    pub(crate) fn text(x: f64, y: f64, label: &str) -> Annotation {
        Annotation {
            x: Some(x),
            y: Some(y),
            label: Some(label.into()),
            color: Some("white".into()),
            ..bare(AnnotationType::Text)
        }
    }

    /// A thin outline of a panel.
    pub(crate) fn outline(x: f64, y: f64, w: f64, h: f64) -> Annotation {
        Annotation {
            x: Some(x),
            y: Some(y),
            width: Some(w),
            height: Some(h),
            color: Some("muted".into()),
            ..bare(AnnotationType::Box)
        }
    }

    /// A straight line from (x0, y0) to (x1, y1).
    pub(crate) fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Annotation {
        Annotation {
            points: vec![[x0, y0], [x1, y1]],
            ..bare(AnnotationType::Line)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letters(c: &str, r: &str, s: &str) -> [(usize, bool); 3] {
        [letter(c).unwrap(), letter(r).unwrap(), letter(s).unwrap()]
    }

    #[test]
    fn dicom_axial_needs_no_change_and_nifti_ras_is_flipped() {
        // DICOM axial CT: columns toward L, rows toward P
        let f = anatomical_frame(View::Axial, letters("L", "P", "S")).unwrap();
        assert_eq!(f, IDENTITY);
        // NIfTI RAS: i toward R, j toward A → both flipped
        let f = anatomical_frame(View::Axial, letters("R", "A", "S")).unwrap();
        assert!(f.u.flip && f.v.flip && f.w.data == 2);
        // coronal of an axial stack: x across, z down with S up
        let f = anatomical_frame(View::Coronal, letters("L", "P", "S")).unwrap();
        assert_eq!((f.u.data, f.v.data, f.v.flip, f.w.data), (0, 2, true, 1));
        // sagittal: rows (A/P) across with A on the left, z down with S up
        let f = anatomical_frame(View::Sagittal, letters("L", "P", "S")).unwrap();
        assert_eq!((f.u.data, f.u.flip, f.v.data, f.w.data), (1, false, 2, 0));
    }
}

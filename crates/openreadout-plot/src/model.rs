//! The data of a figure once it is read: series of points in data units, with what they are
//! (quantity and unit of each axis), where they came from and how they want to be drawn.

use openreadout_figure::Color;

use crate::spec::MarkKind;

/// What one axis of a series measures.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Quantity {
    /// `time`, `retention time`, `m/z`, `chemical shift`, `OD600`, `current`, ...
    pub name: String,
    /// Unit (`s` for every time axis; the panel picks the unit it shows).
    pub unit: Option<String>,
}

impl Quantity {
    pub fn new(name: impl Into<String>, unit: Option<&str>) -> Quantity {
        let name: String = name.into();
        let name = name.replace('_', " ");
        // the real part of a processed spectrum is its intensity
        let name = match name.to_ascii_lowercase().as_str() {
            "real" | "re" | "real part" => "intensity".to_string(),
            _ => name,
        };
        Quantity {
            name,
            unit: unit.filter(|u| !u.trim().is_empty()).map(pretty_unit),
        }
    }

    pub fn is_time(&self) -> bool {
        self.unit.as_deref() == Some("s") && matches!(self.name.as_str(), "time" | "retention time")
    }

    /// `name (unit)`.
    pub fn label(&self) -> String {
        match &self.unit {
            Some(u) => format!("{} ({u})", self.name),
            None => self.name.clone(),
        }
    }
}

/// Units as they are printed (IBM Plex has no superscript minus, so wavenumbers are `cm-1`).
fn pretty_unit(u: &str) -> String {
    match u.trim() {
        "1/cm" | "cm-1" | "cm^-1" | "1/CM" | "CM-1" | "cm⁻¹" => "cm-1".into(),
        "uM" => "µM".into(),
        "uL" => "µL".into(),
        "um" => "µm".into(),
        "Volts" | "volts" => "V".into(),
        other => other.to_string(),
    }
}

/// The geometry of a series, in data units.
#[derive(Debug, Clone)]
pub(crate) enum Geom {
    /// A polyline.
    Line(Vec<(f64, f64)>),
    /// A mean line with a spread band: (x, mean, low, high).
    Band(Vec<(f64, f64, f64, f64)>),
    /// Points, with optional error half-widths (same length, or empty).
    Points { pts: Vec<(f64, f64)>, err: Vec<f64> },
    /// Bars over named categories, with optional error half-widths.
    Bars {
        cats: Vec<String>,
        vals: Vec<f64>,
        err: Vec<f64>,
    },
    /// Events: two parameters per event.
    Events { x: Vec<f64>, y: Vec<f64> },
    /// Values to bin into a histogram.
    Hist(Vec<f64>),
}

impl Geom {
    pub fn len(&self) -> usize {
        match self {
            Geom::Line(p) => p.len(),
            Geom::Band(p) => p.len(),
            Geom::Points { pts, .. } => pts.len(),
            Geom::Bars { vals, .. } => vals.len(),
            Geom::Events { x, .. } => x.len(),
            Geom::Hist(v) => v.len(),
        }
    }

    /// Every (x, y) the series covers, for ranges (bands count low and high).
    pub fn extent(&self) -> Extent {
        let mut e = Extent::default();
        match self {
            Geom::Line(p) => p.iter().for_each(|&(x, y)| e.add(x, y)),
            Geom::Band(p) => p.iter().for_each(|&(x, m, lo, hi)| {
                e.add(x, m);
                e.add(x, lo);
                e.add(x, hi);
            }),
            Geom::Points { pts, err } => {
                for (i, &(x, y)) in pts.iter().enumerate() {
                    let d = err.get(i).copied().filter(|d| d.is_finite()).unwrap_or(0.0);
                    e.add(x, y - d);
                    e.add(x, y + d);
                }
            }
            Geom::Bars { vals, err, .. } => {
                for (i, &v) in vals.iter().enumerate() {
                    let d = err.get(i).copied().filter(|d| d.is_finite()).unwrap_or(0.0);
                    e.add(i as f64, 0.0);
                    e.add(i as f64, v + d);
                }
            }
            Geom::Events { x, y } => x.iter().zip(y).for_each(|(&a, &b)| e.add(a, b)),
            Geom::Hist(v) => v.iter().for_each(|&a| e.add(a, 0.0)),
        }
        e
    }

    /// Scale x by `k` (time unit changes).
    pub fn scale_x(&mut self, k: f64) {
        match self {
            Geom::Line(p) | Geom::Points { pts: p, .. } => p.iter_mut().for_each(|q| q.0 *= k),
            Geom::Band(p) => p.iter_mut().for_each(|q| q.0 *= k),
            Geom::Events { x, .. } => x.iter_mut().for_each(|v| *v *= k),
            Geom::Hist(v) => v.iter_mut().for_each(|a| *a *= k),
            Geom::Bars { .. } => {}
        }
    }

    /// Add `d` to every y.
    pub fn shift_y(&mut self, d: f64) {
        match self {
            Geom::Line(p) | Geom::Points { pts: p, .. } => p.iter_mut().for_each(|q| q.1 += d),
            Geom::Band(p) => p.iter_mut().for_each(|q| {
                q.1 += d;
                q.2 += d;
                q.3 += d;
            }),
            Geom::Events { y, .. } => y.iter_mut().for_each(|v| *v += d),
            Geom::Bars { vals, .. } => vals.iter_mut().for_each(|v| *v += d),
            Geom::Hist(_) => {}
        }
    }
}

/// Finite data ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Extent {
    pub x: (f64, f64),
    pub y: (f64, f64),
    /// Smallest positive x and y (log axes).
    pub x_pos: f64,
    pub y_pos: f64,
}

impl Default for Extent {
    fn default() -> Self {
        Extent {
            x: (f64::INFINITY, f64::NEG_INFINITY),
            y: (f64::INFINITY, f64::NEG_INFINITY),
            x_pos: f64::INFINITY,
            y_pos: f64::INFINITY,
        }
    }
}

impl Extent {
    pub fn add(&mut self, x: f64, y: f64) {
        if x.is_finite() {
            self.x = (self.x.0.min(x), self.x.1.max(x));
            if x > 0.0 {
                self.x_pos = self.x_pos.min(x);
            }
        }
        if y.is_finite() {
            self.y = (self.y.0.min(y), self.y.1.max(y));
            if y > 0.0 {
                self.y_pos = self.y_pos.min(y);
            }
        }
    }

    pub fn merge(&mut self, o: &Extent) {
        self.x = (self.x.0.min(o.x.0), self.x.1.max(o.x.1));
        self.y = (self.y.0.min(o.y.0), self.y.1.max(o.y.1));
        self.x_pos = self.x_pos.min(o.x_pos);
        self.y_pos = self.y_pos.min(o.y_pos);
    }

    pub fn has_x(&self) -> bool {
        self.x.0 <= self.x.1
    }

    pub fn has_y(&self) -> bool {
        self.y.0 <= self.y.1
    }
}

/// A labelled spot on a series: a peak, an apex, a spike.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Callout {
    pub x: f64,
    pub y: f64,
    /// Text (empty: a marker only).
    pub text: String,
    /// The text is the x position (rewritten when the axis changes unit).
    pub auto: bool,
}

/// How strongly a series is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Weight {
    /// The data the figure is about.
    #[default]
    Main,
    /// One of many (wells, events): thin and partly transparent.
    Many,
    /// Context behind the main data: faint.
    Context,
    /// A fitted curve.
    Fit,
}

/// One series, ready to draw.
#[derive(Debug, Clone)]
pub(crate) struct Series {
    /// Name (legend, direct label).
    pub name: String,
    pub geom: Geom,
    pub mark: MarkKind,
    /// Colour asked for.
    pub color: Option<Color>,
    /// Colour class: series of one class share a colour and one legend entry.
    pub class: Option<String>,
    pub weight: Weight,
    pub x: Quantity,
    pub y: Quantity,
    /// Category names of a categorical x (bars, points over groups).
    pub x_cats: Option<Vec<String>>,
    pub callouts: Vec<Callout>,
    /// Facet this series belongs to (well name, sweep, file).
    pub facet: Option<String>,
    /// Plate position (zero-based row, column).
    pub well: Option<(u32, u32)>,
    /// Where the data came from.
    pub source: Source,
    /// Suggested axis directions and scales.
    pub x_reverse: bool,
    pub x_log: bool,
    pub y_log: bool,
    /// Ticks of a transformed axis (FCS logicle): position and label.
    pub x_ticks: Option<Vec<(f64, String)>>,
    pub y_ticks: Option<Vec<(f64, String)>>,
    /// The x range worth showing when the axis is not set (a spectrum's peaks).
    pub x_focus: Option<(f64, f64)>,
    /// Label this many of the highest peaks inside the axis ranges shown.
    pub peaks: usize,
}

impl Series {
    pub fn new(name: impl Into<String>, geom: Geom, mark: MarkKind, source: Source) -> Series {
        Series {
            name: name.into(),
            geom,
            mark,
            color: None,
            class: None,
            weight: Weight::Main,
            x: Quantity::default(),
            y: Quantity::default(),
            x_cats: None,
            callouts: Vec::new(),
            facet: None,
            well: None,
            source,
            x_reverse: false,
            x_log: false,
            y_log: false,
            x_ticks: None,
            y_ticks: None,
            x_focus: None,
            peaks: 0,
        }
    }

    pub fn axes(mut self, x: Quantity, y: Quantity) -> Series {
        self.x = x;
        self.y = y;
        self
    }
}

/// Where a series' numbers come from.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Source {
    /// The file read (none for inline values).
    pub file: Option<String>,
    /// What was read from it: `trace 0 sweep 3 channel 0`, `TIC`, `wells A1:H11`, ...
    pub selector: String,
}

impl Source {
    pub fn file(file: &str, selector: impl Into<String>) -> Source {
        Source {
            file: Some(file.to_string()),
            selector: selector.into(),
        }
    }
}

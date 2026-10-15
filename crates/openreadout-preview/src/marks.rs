//! Marks the data asks for: the largest peaks of a spectrum or chromatogram, labelled with
//! their position, and the action potentials of a current-clamp sweep.

use openreadout_figure::{
    Anchor, Baseline, Face, Figure, LabelPlacer, Panel, Path, Rect, Stroke, TextStyle, Theme,
    format_value,
};

use crate::DataMark;

/// Most peaks labelled in one panel.
pub(crate) const MAX_PEAKS: usize = 8;
/// Least prominence of a labelled peak, in noise standard deviations.
pub(crate) const MIN_PEAK_SNR: f64 = 8.0;

/// One pixel column of a trace: its extremes and where the maximum is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Col {
    pub min: f64,
    pub max: f64,
    pub first: f64,
    pub last: f64,
    /// x value of the maximum.
    pub x_at_max: f64,
}

impl Col {
    pub(crate) fn new(x: f64, v: f64) -> Col {
        Col {
            min: v,
            max: v,
            first: v,
            last: v,
            x_at_max: x,
        }
    }

    pub(crate) fn add(&mut self, x: f64, v: f64) {
        self.min = self.min.min(v);
        if v > self.max {
            self.max = v;
            self.x_at_max = x;
        }
        self.last = v;
    }
}

/// The columns with the most prominent local maxima, most prominent first: at most `n`, each
/// at least `min_gap` columns from the others and standing at least `min_share` of the data's
/// range above its surroundings.
pub(crate) fn prominent(
    cols: &[Option<Col>],
    n: usize,
    min_gap: usize,
    min_share: f64,
) -> Vec<usize> {
    let v: Vec<f64> = cols.iter().map(|c| c.map_or(f64::NAN, |c| c.max)).collect();
    let lo = cols
        .iter()
        .flatten()
        .map(|c| c.min)
        .fold(f64::INFINITY, f64::min);
    let hi = v
        .iter()
        .copied()
        .filter(|x| x.is_finite())
        .fold(f64::NEG_INFINITY, f64::max);
    if !(lo.is_finite() && hi.is_finite()) || hi <= lo {
        return Vec::new();
    }
    // noise: the larger of the robust spread around the local baseline (as NMR peak picking
    // measures it) and the median step between neighbouring columns (spiky noise); a peak must
    // stand well clear of it, so ripples on a noisy baseline stay unlabelled
    let finite: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    let mut steps: Vec<f64> = finite.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    steps.sort_by(f64::total_cmp);
    let step_noise = if steps.len() >= 8 {
        steps[steps.len() / 2]
    } else {
        0.0
    };
    let noise = openreadout_signal::nmr::noise_sd(&finite).max(step_noise);
    let floor = ((hi - lo) * min_share).max(noise * MIN_PEAK_SNR);
    let len = v.len();
    let mut cands: Vec<(usize, f64)> = Vec::new();
    for i in 0..len {
        let x = v[i];
        if !x.is_finite() {
            continue;
        }
        let left = (0..i).rev().map(|j| v[j]).find(|y| y.is_finite());
        let right = (i + 1..len).map(|j| v[j]).find(|y| y.is_finite());
        // a local maximum (plateaus count once, at their left end)
        if left.is_some_and(|l| l >= x) || right.is_some_and(|r| r > x) {
            continue;
        }
        // prominence: the drop to the higher of the two lowest points before a taller column
        let mut lmin = x;
        let mut j = i;
        let mut lwall = false;
        while j > 0 {
            j -= 1;
            let y = cols[j].map_or(f64::NAN, |c| c.min).min(v[j]);
            if v[j].is_finite() && v[j] > x {
                lwall = true;
                break;
            }
            if y.is_finite() {
                lmin = lmin.min(y);
            }
        }
        let mut rmin = x;
        let mut k = i;
        let mut rwall = false;
        while k + 1 < len {
            k += 1;
            let y = cols[k].map_or(f64::NAN, |c| c.min).min(v[k]);
            if v[k].is_finite() && v[k] > x {
                rwall = true;
                break;
            }
            if y.is_finite() {
                rmin = rmin.min(y);
            }
        }
        let base = match (lwall, rwall) {
            (true, true) => lmin.max(rmin),
            (true, false) => lmin.max(rmin),
            (false, true) => lmin.max(rmin),
            (false, false) => lmin.min(rmin),
        };
        let prom = x - base;
        if prom >= floor {
            cands.push((i, prom));
        }
    }
    cands.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    // and at least a tenth as prominent as the most prominent peak
    let top = cands.first().map_or(0.0, |c| c.1);
    cands.retain(|c| c.1 >= top * 0.1);
    let mut out: Vec<usize> = Vec::new();
    for (i, _) in cands {
        if out.len() >= n {
            break;
        }
        if out.iter().all(|&j| i.abs_diff(j) >= min_gap) {
            out.push(i);
        }
    }
    out
}

/// How many decimals a peak label needs so neighbouring peaks `resolution` apart differ.
pub(crate) fn decimals_for(resolution: f64) -> usize {
    if !(resolution.is_finite() && resolution > 0.0) {
        return 2;
    }
    let d = (-resolution.log10()).ceil();
    d.clamp(0.0, 4.0) as usize
}

/// Format a peak position with `decimals` decimals.
pub(crate) fn peak_label(x: f64, decimals: usize) -> String {
    if x.abs() >= 1e5 {
        return format_value(x);
    }
    format!("{x:.decimals$}")
}

/// Label peaks at `(x, y)` data points in `panel`: a small tick above each peak and its label
/// above that, skipping labels that would collide. Returns the marks drawn.
pub(crate) fn label_peaks(
    fig: &mut Figure,
    panel: &Panel,
    theme: &Theme,
    peaks: &[(f64, f64, String)],
    channel: Option<u32>,
    placer: &mut LabelPlacer,
) -> Vec<DataMark> {
    let fs = theme.small_size();
    let st = TextStyle::new(fs, theme.ink)
        .face(Face::Mono)
        .align(Anchor::Middle, Baseline::Alphabetic)
        .halo(theme.paper, (fs * 0.18).max(1.5));
    let mut out = Vec::new();
    for (x, y, label) in peaks {
        let Some((px, py)) = panel.pt(*x, *y) else {
            continue;
        };
        if !panel.rect.inset(-1.0).contains(px, py) {
            continue;
        }
        let gap = fs * 0.45;
        let ly = (py - gap).max(panel.rect.y + fs);
        let b = st.bounds(px, ly, label);
        // try just above the peak; else skip the label (the mark stays)
        let r = Rect::new(b.x - 1.0, b.y - 1.0, b.w + 2.0, b.h + 2.0);
        let fits = placer.free(&r) && fig.rect().contains_rect(&r);
        fig.stroke(
            Path::segment(px, py - 2.0, px, py - gap * 0.9),
            Stroke::new(theme.accent, theme.hairline()),
        );
        if fits {
            placer.reserve(r);
            fig.text(px, ly, label, &st);
        }
        out.push(DataMark {
            kind: "peak".into(),
            x: *x,
            y: *y,
            label: fits.then(|| label.clone()),
            channel,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_prominent_peaks() {
        // peaks on a quiet baseline
        let mut ys = vec![0.0; 40];
        ys[1] = 1.0;
        ys[4] = 5.0;
        ys[7] = 0.3;
        ys[9] = 3.0;
        for (i, y) in ys.iter_mut().enumerate() {
            *y += 0.001 * (i % 3) as f64;
        }
        let cols: Vec<Option<Col>> = ys
            .iter()
            .enumerate()
            .map(|(i, &v)| Some(Col::new(i as f64, v)))
            .collect();
        assert_eq!(prominent(&cols, 2, 1, 0.1), vec![4, 9]);
        assert_eq!(prominent(&cols, 5, 1, 0.1), vec![4, 9, 1]);
        assert_eq!(prominent(&cols, 5, 6, 0.1), vec![4]);
        // spiky noise with one spike twice its size: nothing stands out enough to label
        let noisy: Vec<Option<Col>> = (0..60)
            .map(|i| {
                let v = if i == 30 { 2.0 } else { f64::from(i % 2) };
                Some(Col::new(f64::from(i), v))
            })
            .collect();
        assert!(prominent(&noisy, 5, 1, 0.04).is_empty());
        assert_eq!(decimals_for(0.01), 2);
        assert_eq!(peak_label(1.23456, 2), "1.23");
    }
}

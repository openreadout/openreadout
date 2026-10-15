//! Draws a few sample figures with the engine:
//! `cargo run -p openreadout-figure --example gallery -- OUT_DIR`.

#![allow(clippy::unreadable_literal, clippy::many_single_char_names)]

use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, EndLabel, Envelope, Face, Figure, GridLines, Marker,
    Panel, Path, Pill, Rect, Stroke, TextStyle, Theme, color_bar, end_labels, grid, key_row,
};

fn growth(t: f64, lag: f64, k: f64) -> f64 {
    1.1 / (1.0 + (-(t - lag) * k).exp())
}

fn lines(theme: &Theme, w: f32, h: f32) -> Figure {
    let mut fig = Figure::new(w, h, Some(theme.paper));
    let spec = AxesSpec {
        x_label: Some("time (h)".into()),
        y_label: Some("OD600".into()),
        right: theme.font_size * 6.0,
        ..AxesSpec::default()
    };
    let p = Panel::fit(
        fig.rect().inset(theme.font_size),
        Domain::linear(0.0, 16.0),
        Domain::linear(0.0, 1.2),
        theme,
        &spec,
    );
    p.axes(&mut fig, theme, &spec);
    p.band_x(&mut fig, 7.4, 8.6, theme.band);
    let mut labels = Vec::new();
    for (i, (name, lag, k)) in [("λ", 3.0, 1.0), ("HK97", 9.0, 0.9), ("no phage", 2.5, 1.2)]
        .iter()
        .enumerate()
    {
        let c = theme.series(i);
        let pts: Vec<(f64, f64)> = (0..=160)
            .map(|j| {
                let t = f64::from(j) / 10.0;
                (t, growth(t, *lag, *k))
            })
            .collect();
        p.line(
            &mut fig,
            pts.iter().copied(),
            Stroke::new(c, theme.line_width).round(),
        );
        labels.push(EndLabel {
            y: p.y.px(pts[160].1).unwrap_or(0.0),
            text: (*name).to_string(),
            color: c,
        });
    }
    end_labels(
        &mut fig,
        theme,
        p.rect.right(),
        &labels,
        p.rect.y,
        p.rect.bottom(),
    );
    p.text(
        &mut fig,
        8.0,
        1.15,
        "came back 7.4–8.6 h",
        &TextStyle::new(theme.font_size, theme.accent).align(Anchor::Middle, Baseline::Middle),
    );
    p.markers(
        &mut fig,
        [(4.0, growth(4.0, 3.0, 1.0))],
        &Marker::dot(theme.accent, theme.font_size * 0.6),
    );
    fig
}

fn multiples(theme: &Theme) -> Figure {
    let mut fig = Figure::new(1200.0, 700.0, Some(theme.paper));
    let cells = grid(Rect::new(70.0, 50.0, 1110.0, 560.0), 8, 12, 8.0, 8.0);
    for (i, c) in cells.iter().enumerate() {
        let card = Path::round_rect(c.x, c.y, c.w, c.h, 5.0);
        fig.shape(
            card,
            Some(theme.panel),
            Some(Stroke::new(theme.rule, 1.0)),
            None,
        );
        let p = Panel::new(
            c.inset(5.0),
            Domain::linear(0.0, 16.0),
            Domain::linear(0.0, 1.2),
        );
        let crashed = i % 12 >= 8;
        let col = if crashed { theme.accent } else { theme.ink };
        let mut env = Envelope::new(p.rect.w as usize, 0.0, 16.0);
        for j in 0..=1600 {
            let t = f64::from(j) / 100.0;
            let v = if crashed {
                growth(t, 3.0, 1.0) * (1.0 - growth(t, 6.0, 2.0))
                    + growth(t, 10.0 + (i % 4) as f64, 1.0) * 0.8
            } else {
                growth(t, 3.0, 1.0)
            };
            env.push(t, v + ((j * 7919) % 13) as f64 * 0.002);
        }
        p.line_px(&mut fig, env.points(&p), Stroke::new(col, 1.4));
    }
    key_row(
        &mut fig,
        theme,
        70.0,
        650.0,
        1000.0,
        &[
            (theme.ink, "grew".to_string()),
            (theme.accent, "crashed".to_string()),
        ],
        false,
    );
    fig
}

fn dose(theme: &Theme) -> Figure {
    let mut fig = Figure::new(700.0, 520.0, Some(theme.paper));
    let spec = AxesSpec {
        x_label: Some("concentration (nM)".into()),
        y_label: Some("response (%)".into()),
        grid: GridLines::Both,
        ..AxesSpec::default()
    };
    let p = Panel::fit(
        fig.rect().inset_sides(10.0, 10.0, 20.0, 70.0),
        Domain::log(0.01, 10000.0),
        Domain::linear(0.0, 100.0),
        theme,
        &spec,
    );
    p.axes(&mut fig, theme, &spec);
    let pts: Vec<(f64, f64)> = (0..=60)
        .map(|i| {
            let c = 10f64.powf(-2.0 + f64::from(i) / 10.0);
            (c, 100.0 / (1.0 + (3.0 / c).powf(1.2)))
        })
        .collect();
    p.line(
        &mut fig,
        pts.iter().copied(),
        Stroke::new(theme.accent, theme.line_width),
    );
    p.markers(
        &mut fig,
        pts.iter().step_by(6).copied(),
        &Marker::dot(theme.ink, 7.0),
    );
    color_bar(
        &mut fig,
        theme,
        Rect::new(120.0, 480.0, 240.0, 10.0),
        &theme.sequential,
        0.0,
        2.5,
        Some("OD600"),
    );
    fig
}

fn pills(theme: &Theme) -> Figure {
    let mut fig = Figure::new(500.0, 200.0, Some(Color::hex(0x4A3F7A)));
    let pill = Pill {
        text: TextStyle::new(15.0, theme.ink).face(Face::SansMedium),
        fill: theme.paper.alpha(0.92),
        stroke: None,
        pad: (9.0, 5.0),
    };
    for (i, t) in ["Purkinje cells", "Granular layer", "Molecular layer"]
        .iter()
        .enumerate()
    {
        let (w, h) = pill.size(t);
        pill.draw(
            &mut fig,
            Rect::new(30.0 + i as f32 * 20.0, 30.0 + i as f32 * 50.0, w, h),
            t,
        );
    }
    fig.text(
        300.0,
        150.0,
        "halo text 1234 µm",
        &TextStyle::new(14.0, theme.ink).halo(theme.paper, 2.5),
    );
    fig
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let light = Theme::light().for_size(900.0);
    let dark = Theme::dark().for_size(900.0);
    let figs = [
        ("lines-light", lines(&light, 900.0, 520.0)),
        ("lines-dark", lines(&dark, 900.0, 520.0)),
        (
            "lines-small",
            lines(&Theme::light().for_size(480.0), 480.0, 300.0),
        ),
        ("multiples", multiples(&Theme::light().for_size(1200.0))),
        ("dose", dose(&Theme::light().for_size(700.0))),
        ("pills", pills(&light)),
    ];
    for (name, f) in figs {
        std::fs::write(format!("{out}/{name}.png"), f.to_png(1.0)?)?;
        std::fs::write(format!("{out}/{name}.svg"), f.to_svg())?;
    }
    Ok(())
}

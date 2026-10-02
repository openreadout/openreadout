//! Terminal presentation: colour, progress, quiet mode and plain-text tables.
//!
//! Colour goes through `anstream`, which strips styles when the stream is not a terminal,
//! when `NO_COLOR` is set (it wins over `CLICOLOR_FORCE`), and honours `CLICOLOR_FORCE`;
//! `--color always|never` overrides the environment. JSON is never styled. Progress is drawn
//! on stderr only (never stdout): automatically when stderr is a terminal, always with
//! `--progress` (as plain lines when stderr is not a terminal), never with `--no-progress`
//! or `--quiet`.

use std::io::IsTerminal;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anstyle::{AnsiColor, Style};

/// `--color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorWhen {
    /// Colour when the output is a terminal (honours `NO_COLOR` and `CLICOLOR_FORCE`).
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Default)]
struct Settings {
    quiet: bool,
    /// `Some(true)`: `--progress`; `Some(false)`: `--no-progress`; `None`: automatic.
    progress: Option<bool>,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// Apply the global flags (call once, before any output).
pub fn init(color: ColorWhen, quiet: bool, progress: Option<bool>) {
    anstream::ColorChoice::write_global(match color {
        ColorWhen::Auto => anstream::ColorChoice::Auto,
        ColorWhen::Always => anstream::ColorChoice::Always,
        ColorWhen::Never => anstream::ColorChoice::Never,
    });
    let _ = SETTINGS.set(Settings { quiet, progress });
}

fn settings() -> Settings {
    SETTINGS.get().copied().unwrap_or_default()
}

/// `--quiet`: no human-readable success output, no progress, no batch summary.
pub fn quiet() -> bool {
    settings().quiet
}

pub const OK: Style = AnsiColor::Green.on_default();
pub const ERR: Style = AnsiColor::Red.on_default().bold();
pub const WARN: Style = AnsiColor::Yellow.on_default();
pub const BOLD: Style = Style::new().bold();
pub const DIM: Style = Style::new().dimmed();

/// `text` wrapped in `style` (stripped again by `anstream` where colour is off).
pub fn paint(style: Style, text: impl std::fmt::Display) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}

/// Visible width of a string (styles excluded; one column per `char`).
fn width(s: &str) -> usize {
    let mut n = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\u{1b}' {
            in_escape = true;
        } else {
            n += 1;
        }
    }
    n
}

/// Column alignment for [`table`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// A plain-text table: bold header, a rule, aligned columns separated by two spaces. Cells may
/// contain styles. The last column is not padded.
pub fn table(headers: &[&str], align: &[Align], rows: &[Vec<String>]) -> String {
    let n = headers.len();
    let mut w: Vec<usize> = headers.iter().map(|h| width(h)).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate().take(n) {
            w[i] = w[i].max(width(c));
        }
    }
    let line = |cells: &[String], out: &mut String| {
        for (i, c) in cells.iter().enumerate().take(n) {
            let pad = w[i].saturating_sub(width(c));
            let last = i + 1 == n;
            if align.get(i) == Some(&Align::Right) {
                out.push_str(&" ".repeat(pad));
                out.push_str(c);
            } else {
                out.push_str(c);
                if !last {
                    out.push_str(&" ".repeat(pad));
                }
            }
            if !last {
                out.push_str("  ");
            }
        }
        out.push('\n');
    };
    let mut s = String::new();
    let head: Vec<String> = headers.iter().map(|h| paint(BOLD, h)).collect();
    line(&head, &mut s);
    let rule: Vec<String> = w.iter().map(|&k| paint(DIM, "─".repeat(k))).collect();
    line(&rule, &mut s);
    for r in rows {
        line(r, &mut s);
    }
    s.trim_end().to_string()
}

/// A progress indicator on stderr (or nothing).
#[derive(Debug)]
pub enum Progress {
    None,
    Bar(indicatif::ProgressBar),
    /// Forced progress on a non-terminal stderr: one line per 10 %.
    Lines {
        label: String,
        total: u64,
        last_decile: AtomicU64,
    },
}

impl Progress {
    /// A progress indicator for `total` steps of `label` (e.g. "planes", "files").
    pub fn new(total: u64, label: &str) -> Progress {
        let s = settings();
        if s.quiet || s.progress == Some(false) || total == 0 {
            return Progress::None;
        }
        let tty = std::io::stderr().is_terminal();
        if tty {
            let bar = indicatif::ProgressBar::with_draw_target(
                Some(total),
                indicatif::ProgressDrawTarget::stderr(),
            );
            bar.set_style(
                indicatif::ProgressStyle::with_template(
                    "{spinner} {prefix} [{bar:30}] {pos}/{len} {msg} ({elapsed}, eta {eta})",
                )
                .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
                .progress_chars("=> "),
            );
            bar.set_prefix(label.to_string());
            bar.enable_steady_tick(Duration::from_millis(200));
            Progress::Bar(bar)
        } else if s.progress == Some(true) {
            Progress::Lines {
                label: label.to_string(),
                total,
                last_decile: AtomicU64::new(0),
            }
        } else {
            Progress::None
        }
    }

    /// A progress indicator for work of unknown length (a crawl): a spinner with a count on a
    /// terminal; with `--progress` on a non-terminal, a plain line at most every 5 seconds.
    pub fn spinner(label: &str) -> Progress {
        let s = settings();
        if s.quiet || s.progress == Some(false) {
            return Progress::None;
        }
        if std::io::stderr().is_terminal() {
            let bar = indicatif::ProgressBar::with_draw_target(
                None,
                indicatif::ProgressDrawTarget::stderr(),
            );
            bar.set_style(
                indicatif::ProgressStyle::with_template(
                    "{spinner} {prefix} {pos} {msg} ({elapsed}, {per_sec})",
                )
                .unwrap_or_else(|_| indicatif::ProgressStyle::default_spinner()),
            );
            bar.set_prefix(label.to_string());
            bar.enable_steady_tick(Duration::from_millis(200));
            Progress::Bar(bar)
        } else if s.progress == Some(true) {
            Progress::Lines {
                label: label.to_string(),
                total: 0,
                last_decile: AtomicU64::new(0),
            }
        } else {
            Progress::None
        }
    }

    /// Set the current position (and an optional message, e.g. the file being processed).
    pub fn set(&self, done: u64, msg: Option<&str>) {
        match self {
            Progress::None => {}
            Progress::Bar(b) => {
                b.set_position(done);
                if let Some(m) = msg {
                    b.set_message(m.to_string());
                }
            }
            Progress::Lines {
                label,
                total,
                last_decile,
            } if *total == 0 => {
                // Spinner in lines mode: `last_decile` holds the last print time (seconds).
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                if now >= last_decile.load(Ordering::Relaxed) + 5 {
                    last_decile.store(now, Ordering::Relaxed);
                    eprintln!(
                        "openreadout: {label} {done}{}",
                        msg.map(|m| format!(" {m}")).unwrap_or_default()
                    );
                }
            }
            Progress::Lines {
                label,
                total,
                last_decile,
            } => {
                let decile = done * 10 / (*total).max(1);
                if decile > last_decile.load(Ordering::Relaxed) || done == 0 {
                    last_decile.store(decile, Ordering::Relaxed);
                    eprintln!(
                        "openreadout: {label} {done}/{total}{}",
                        msg.map(|m| format!(" {m}")).unwrap_or_default()
                    );
                }
            }
        }
    }

    /// Run `f` (which prints) with the bar hidden, so output and bar do not interleave.
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        match self {
            Progress::Bar(b) => b.suspend(f),
            _ => f(),
        }
    }

    /// Remove the bar (lines mode prints nothing more).
    pub fn finish(&self) {
        if let Progress::Bar(b) = self {
            b.finish_and_clear();
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_styled_cells() {
        let t = table(
            &["a", "bb"],
            &[Align::Left, Align::Right],
            &[
                vec![paint(OK, "xyz"), "1".into()],
                vec!["w".into(), "22".into()],
            ],
        );
        let plain: String = t
            .lines()
            .map(|l| {
                let mut o = String::new();
                let mut esc = false;
                for ch in l.chars() {
                    if esc {
                        esc = !ch.is_ascii_alphabetic();
                    } else if ch == '\u{1b}' {
                        esc = true;
                    } else {
                        o.push(ch);
                    }
                }
                o + "\n"
            })
            .collect();
        assert_eq!(plain, "a    bb\n───  ──\nxyz   1\nw    22\n");
    }
}

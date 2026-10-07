//! Files that are still being written: the reader's structural evidence ([`WriteState`]) and
//! the in-progress heuristic ([`assess`]) that turns it into an [`Acquisition`] status.
//!
//! A file an instrument is still writing lacks the structures its software writes last (a CZI
//! subblock directory, the ND2 chunk map, OME-XML at the end of an OME-TIFF, Zarr chunks not
//! yet on disk). Readers that know their format's write order report what is missing and which
//! planes are already complete through [`crate::Dataset::write_state`]. [`assess`] then decides:
//!
//! - `in_progress`: the missing parts are only those written last, everything before them is
//!   intact, the partial tail is no bigger than about one write unit, and the file (or, for a
//!   directory store, its newest member) was modified within the live window
//!   ([`window`], default [`DEFAULT_WINDOW_S`] seconds, `--live-window` / `OPENREADOUT_LIVE_WINDOW`).
//! - `interrupted`: the same structural evidence, but the file has not changed for longer than
//!   the window: an acquisition or copy that stopped. Readers report it as any truncated file
//!   (truncation findings, exit 4 where they apply).
//!
//! Damage in the middle of a file (a bad segment id before the tail, a tail much larger than a
//! write unit) is never in progress. Failure modes are documented in `book/src/guides/lab-shares.md`: a
//! truncated copy made less than the window ago looks in progress, and a time-lapse whose
//! interval is longer than the window looks interrupted between frames.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{CheckReport, Finding, Severity};
use crate::reader::{Dataset, PlaneIndex};

/// Default live window in seconds: a file modified less than this long ago can be in progress.
pub const DEFAULT_WINDOW_S: u64 = 300;

/// Environment variable that sets the live window in seconds (`0` turns detection off).
pub const WINDOW_ENV: &str = "OPENREADOUT_LIVE_WINDOW";

/// Slack added to the size of one write unit when judging a partial tail (headers, padding).
const TAIL_SLACK: u64 = 1 << 20;

/// Largest partial tail accepted when the reader does not know its write unit.
const MAX_TAIL: u64 = 64 << 20;

/// A partial tail up to this size is always plausible (metadata blocks written at the end).
const MIN_TAIL: u64 = 16 << 20;

static WINDOW: OnceLock<Duration> = OnceLock::new();

/// Set the live window for this process (first call wins; the CLI calls it from
/// `--live-window`). Without a call, [`window`] reads [`WINDOW_ENV`] or uses the default.
pub fn set_window(d: Duration) {
    let _ = WINDOW.set(d);
}

/// The live window: how recently a file must have changed to be judged in progress.
pub fn window() -> Duration {
    *WINDOW.get_or_init(|| {
        std::env::var(WINDOW_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map_or(
                Duration::from_secs(DEFAULT_WINDOW_S),
                Duration::from_secs_f64,
            )
    })
}

/// What a reader found about a file that is not finished: the parts its software writes last
/// that are absent, and the planes already complete. Returned by
/// [`crate::Dataset::write_state`] only for files that are incomplete.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriteState {
    /// Structures written at the end of an acquisition that are absent, in the format's own
    /// words (`chunk map`, `subblock directory`, `OME-XML`, `chunk files`).
    pub missing: Vec<String>,
    /// Planes whose data is entirely on disk, in the order they were written.
    pub complete: Vec<(u32, PlaneIndex)>,
    /// Planes the file will hold when finished, when its metadata says so.
    pub expected_planes: Option<u64>,
    /// Bytes after the last complete unit: a plane, chunk or segment still being written.
    pub tail_bytes: u64,
    /// Size of one write unit (a frame chunk, a subblock segment, a page), for judging the tail.
    pub unit_bytes: Option<u64>,
    /// Everything before the tail is intact (no damage in the middle of the file), so the
    /// incompleteness is explained by writing that has not finished.
    pub append_consistent: bool,
    /// Newest modification time among the files of the data set (directory stores); `None`
    /// uses the path's own modification time.
    pub modified: Option<SystemTime>,
    /// The observations behind this state, in plain words.
    pub evidence: Vec<String>,
}

impl WriteState {
    /// An empty state that is append-consistent until a reader says otherwise.
    pub fn new() -> Self {
        WriteState {
            append_consistent: true,
            ..Default::default()
        }
    }
}

/// Whether an incomplete file is still being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AcquisitionState {
    /// Recently modified and missing only what is written last: still acquiring.
    InProgress,
    /// Missing what is written last, but unchanged for longer than the live window: the
    /// acquisition (or a copy) stopped before the end.
    Interrupted,
}

impl AcquisitionState {
    /// The snake_case name used in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            AcquisitionState::InProgress => "in_progress",
            AcquisitionState::Interrupted => "interrupted",
        }
    }
}

/// `acquisition` in `info`, `info --view structure`, `check` and `planes`: present only for
/// files that are not finished. See `book/src/guides/lab-shares.md`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Acquisition {
    /// `in_progress` or `interrupted`.
    pub state: AcquisitionState,
    /// Planes whose data is entirely on disk.
    pub complete_planes: u64,
    /// Planes the finished file will hold, when the metadata written so far says so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_planes: Option<u64>,
    /// Seconds since the file (or the newest member of a directory store) was modified.
    pub modified_ago_s: f64,
    /// The live window used for the decision, in seconds.
    pub window_s: f64,
    /// Structures written at the end of an acquisition that are absent.
    pub missing: Vec<String>,
    /// Bytes after the last complete unit (a unit still being written).
    pub tail_bytes: u64,
    /// The observations behind the decision.
    pub evidence: Vec<String>,
}

impl Acquisition {
    /// True when the file is still being written.
    pub fn in_progress(&self) -> bool {
        self.state == AcquisitionState::InProgress
    }
}

/// Seconds since `t` (negative when `t` is in the future, e.g. clock skew on a share).
pub fn seconds_since(t: SystemTime) -> f64 {
    match SystemTime::now().duration_since(t) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// Decide whether an incomplete file is in progress, with the process-wide [`window`].
/// `None` when the evidence does not describe an unfinished write (damage in the middle, a
/// tail too large for one write unit): the reader's own findings then stand.
pub fn assess(path: &Path, ws: &WriteState) -> Option<Acquisition> {
    assess_with(path, ws, window())
}

/// [`assess`] with an explicit window.
pub fn assess_with(path: &Path, ws: &WriteState, window: Duration) -> Option<Acquisition> {
    if !ws.append_consistent {
        return None;
    }
    let limit = ws.unit_bytes.map_or(MAX_TAIL, |u| {
        u.saturating_mul(2).saturating_add(TAIL_SLACK).max(MIN_TAIL)
    });
    if ws.tail_bytes > limit {
        return None;
    }
    let modified = ws
        .modified
        .or_else(|| std::fs::metadata(path).and_then(|m| m.modified()).ok())?;
    let ago = seconds_since(modified);
    let mut evidence = ws.evidence.clone();
    let w = window.as_secs_f64();
    // Up to a minute in the future is clock skew between the instrument PC and this machine.
    let recent = w > 0.0 && ago <= w && ago >= -60.0;
    let state = if recent {
        evidence.push(format!(
            "modified {ago:.1} s ago, within the {w:.0} s live window"
        ));
        AcquisitionState::InProgress
    } else {
        evidence.push(format!(
            "unchanged for {ago:.0} s, longer than the {w:.0} s live window"
        ));
        AcquisitionState::Interrupted
    };
    Some(Acquisition {
        state,
        complete_planes: ws.complete.len() as u64,
        expected_planes: ws.expected_planes,
        modified_ago_s: (ago * 10.0).round() / 10.0,
        window_s: w,
        missing: ws.missing.clone(),
        tail_bytes: ws.tail_bytes,
        evidence,
    })
}

/// [`assess`] on an opened data set: `None` for finished files and formats that do not report
/// a [`WriteState`].
pub fn assess_dataset(ds: &dyn Dataset, path: &Path) -> Option<Acquisition> {
    ds.write_state().and_then(|ws| assess(path, &ws))
}

/// Code of the `check` finding that marks a file still being written.
pub const IN_PROGRESS_CODE: &str = "acquisition_in_progress";

/// Adjust a `check` report for a file that is still being written: the findings about the
/// unfinished tail become warnings (prefixed `in progress:`), an `acquisition_in_progress`
/// finding explains why, and `ok` then covers the complete part only. Interrupted files keep
/// their errors.
pub fn apply_to_check(report: &mut CheckReport, acq: &Acquisition) {
    report.acquisition = Some(acq.clone());
    if !acq.in_progress() {
        return;
    }
    for f in &mut report.findings {
        if f.severity == Severity::Error {
            f.severity = Severity::Warning;
            f.message = format!("in progress: {}", f.message);
        }
    }
    report.ok = !report
        .findings
        .iter()
        .any(|f| f.severity == Severity::Error);
    let expected = acq
        .expected_planes
        .map_or_else(String::new, |e| format!(" of {e}"));
    report.findings.insert(
        0,
        Finding::warning(
            IN_PROGRESS_CODE,
            format!(
                "the file is still being written (modified {:.1} s ago; missing: {}): {}{} planes complete; `ok` covers the complete part only, run `check` again when the acquisition ends",
                acq.modified_ago_s,
                if acq.missing.is_empty() {
                    "nothing yet".into()
                } else {
                    acq.missing.join(", ")
                },
                acq.complete_planes,
                expected
            ),
        ),
    );
}

/// For formats that cannot be read while growing: when opening failed as corrupt and the file
/// changed within the live window, say so in the error (the exit code stays 4).
pub fn annotate_error(path: &Path, err: crate::Error) -> crate::Error {
    let crate::Error::Corrupt {
        format,
        detail,
        offset,
    } = err
    else {
        return err;
    };
    let w = window().as_secs_f64();
    let ago = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(seconds_since);
    match ago {
        Some(a) if w > 0.0 && a <= w && a >= -60.0 => crate::Error::Corrupt {
            format,
            detail: format!(
                "{detail} [modified {a:.1} s ago: the file may still be being written; this reader cannot read it until the acquisition ends (`openreadout watch` reports dataset_complete)]"
            ),
            offset,
        },
        _ => crate::Error::Corrupt {
            format,
            detail,
            offset,
        },
    }
}

/// The complete planes of `ws` as a set (`planes` on a file still being written reads only
/// these).
pub fn complete_set(ws: &WriteState) -> std::collections::HashSet<(u32, PlaneIndex)> {
    ws.complete.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(tail: u64) -> WriteState {
        let mut w = WriteState::new();
        w.missing.push("chunk map".into());
        w.complete.push((0, PlaneIndex::default()));
        w.tail_bytes = tail;
        w.unit_bytes = Some(4096);
        w.modified = Some(SystemTime::now());
        w
    }

    #[test]
    fn recent_and_consistent_is_in_progress() {
        let a = assess_with(Path::new("x"), &ws(100), Duration::from_secs(60)).unwrap();
        assert_eq!(a.state, AcquisitionState::InProgress);
        assert_eq!(a.complete_planes, 1);
    }

    #[test]
    fn old_is_interrupted() {
        let mut w = ws(100);
        w.modified = Some(SystemTime::now() - Duration::from_secs(3600));
        let a = assess_with(Path::new("x"), &w, Duration::from_secs(60)).unwrap();
        assert_eq!(a.state, AcquisitionState::Interrupted);
    }

    #[test]
    fn damage_or_huge_tail_is_not_an_acquisition() {
        let mut w = ws(100);
        w.append_consistent = false;
        assert!(assess_with(Path::new("x"), &w, Duration::from_secs(60)).is_none());
        let w = ws(1 << 30);
        assert!(assess_with(Path::new("x"), &w, Duration::from_secs(60)).is_none());
    }

    #[test]
    fn zero_window_disables() {
        let a = assess_with(Path::new("x"), &ws(0), Duration::ZERO).unwrap();
        assert_eq!(a.state, AcquisitionState::Interrupted);
    }

    #[test]
    fn check_findings_downgraded_only_in_progress() {
        let mut r = CheckReport::new("x", "nd2");
        r.push(Finding::error("truncated", "chunk map missing"));
        let a = assess_with(Path::new("x"), &ws(0), Duration::from_secs(60)).unwrap();
        apply_to_check(&mut r, &a);
        assert!(r.ok);
        assert_eq!(r.findings[0].code, IN_PROGRESS_CODE);
        assert!(r.findings[1].message.starts_with("in progress:"));
    }
}

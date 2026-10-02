//! The [`Measure`] trait: what a command computes for one data set, as rows of a tidy table.
//!
//! The runner ([`crate::run()`]) finds the data sets, opens each one, calls
//! [`Measure::rows`], adds the identity columns (`path`, `format`, …) and the error columns, and
//! assembles one table. A command plugs into batch mode by implementing this trait; see
//! `book/src/guides/batch.md` for a worked example.

use std::path::Path;

use openreadout_core::model::FileInfo;
use openreadout_core::{Dataset, Registry, Result};

use crate::table::{ColumnDoc, Row};

/// One opened data set handed to a measure.
pub struct Item<'a> {
    /// The data set's path (a file, or a directory data set).
    pub path: &'a Path,
    /// The readers (to reopen the file, e.g. for parallel plane reads).
    pub registry: &'a Registry,
    /// The open data set.
    pub dataset: &'a mut dyn Dataset,
    /// Its header summary (`info`).
    pub info: &'a FileInfo,
}

impl std::fmt::Debug for Item<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Item")
            .field("path", &self.path)
            .field("format", &self.info.format.id)
            .finish_non_exhaustive()
    }
}

/// A computation that turns one data set into rows.
///
/// Rows hold only the measure's own columns: first its grain keys (which image and channel,
/// which population, which well…), then values. Use stable, lower-case names with the unit in
/// the name when it never changes (`duration_s`, `sample_rate_hz`); put a per-row unit in a
/// `unit` column. Return an error (not an empty list) when the data set holds nothing the
/// measure applies to: it becomes an error row with the error's code and message, so an agent
/// sees why a file has no values.
pub trait Measure: Send + Sync {
    /// Short id (`stats`, `trace`, `table`, `gate`, `info`, `peaks`, …).
    fn id(&self) -> &'static str;
    /// The key columns that identify a row within one data set (`["image", "channel"]`), in
    /// order. Empty: one row per data set.
    fn grain(&self) -> Vec<String>;
    /// Units, roles and descriptions of the columns it produces (`prefix*` covers a family of
    /// columns). Columns not listed are values without a unit.
    fn columns(&self) -> Vec<ColumnDoc> {
        Vec::new()
    }
    /// True when [`Measure::rows`] reads headers only: the runner then measures while it
    /// probes each data set, instead of opening it a second time.
    fn headers_only(&self) -> bool {
        false
    }
    /// A string that changes whenever the options change the output (for resuming a run
    /// from its journal: rows computed with other options are not reused).
    fn fingerprint(&self) -> String;
    /// The rows of one data set.
    fn rows(&self, item: &mut Item<'_>) -> Result<Vec<Row>>;
}

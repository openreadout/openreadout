//! A catalog of every instrument data set on a file share, as open files the user owns.
//!
//! `openreadout index ROOT… -o INDEX_DIR` walks the roots, reads only headers (never pixels,
//! samples or spectra), and writes three Parquet tables and a manifest into `INDEX_DIR`:
//!
//! - `experiments.parquet`: one row per data set (a file, a multi-file set such as a
//!   multi-file OME-TIFF or a CZI with its parts, or a directory data set such as a Bruker
//!   `.d`), with the experiment model flattened into columns (sample, instrument, technique
//!   term, method, operator, start time), image/table/trace/spectra summaries, integrity status
//!   and personal-data flags;
//! - `files.parquet`: one row per file seen, pointing at its data set (or `unknown`);
//! - `problems.parquet`: integrity findings, unreadable files, walk errors and personal-data
//!   flags;
//! - `index.json`: [`IndexManifest`]: roots, settings, crawl counters, totals per format,
//!   family, year and unknown extension, and the column list of every table.
//!
//! The crawl is parallel (the rayon pool), resumable (a journal with checkpoints; a killed run
//! continues where it stopped), incremental (unchanged files are not reopened; moved files are
//! recognised by their content fingerprint), and its memory does not grow with the size of the
//! tree (records stream to disk; the walk holds one directory listing per level). See
//! [`crawl`] for the state files; multi-file grouping happens when the tables are written.
//!
//! On top of an index: [`search()`] (a small query language, [`query`]), [`health()`]
//! (storage health report) and [`export_dataset`] (an ML-ready slice with a datasheet). [`pii`] finds personal
//! data in header metadata and redacts it. [`report`] builds the privacy-reviewed diagnostic
//! bundle of `openreadout report` for a file a reader refused or could not validate.
//!
//! # Example
//!
//! ```no_run
//! use openreadout_core::Registry;
//! use openreadout_index::{IndexOptions, index, search, SearchRequest};
//!
//! let registry = Registry::new(); // the CLI registers every format reader here
//! let mut opts = IndexOptions::default();
//! opts.roots = vec!["/lab/share".into()];
//! opts.index_dir = "/lab/index".into();
//! let manifest = index(&registry, &opts, None)?;
//! println!("{} data sets", manifest.datasets);
//! let mut req = SearchRequest::default();
//! req.query = "format=czi objective=63x acquired<2020".into();
//! let hits = search("/lab/index".as_ref(), &req)?;
//! println!("{} matches", hits.total);
//! # Ok::<(), openreadout_core::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod crawl;
pub mod export;
pub(crate) mod finalize;
pub mod fingerprint;
pub mod formats;
pub mod group;
pub mod health;
pub mod manifest;
pub mod pii;
pub mod query;
pub mod record;
pub mod report;
pub mod search;
pub mod tables;
pub mod walk;

#[doc(hidden)]
pub mod synth;

pub use crawl::{CheckMode, CrawlProgress, IndexOptions, index};
pub use export::{ExportDatasetOptions, ExportDatasetReport, export_dataset};
pub use health::{HealthOptions, HealthReport, health};
pub use manifest::{IndexManifest, read_manifest};
pub use search::{SearchOutput, SearchRequest, search};

/// Columns of the table called `name` (`experiments`, `files`, `problems`).
pub fn tables_named(name: &str) -> &'static [tables::ColumnDef] {
    match name {
        "experiments" => tables::EXPERIMENTS,
        "files" => tables::FILES,
        "problems" => tables::PROBLEMS,
        _ => &[],
    }
}

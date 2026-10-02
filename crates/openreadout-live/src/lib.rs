//! OpenReadout live: data that is still being acquired.
//!
//! - [`replay`] rewrites a finished file step by step in the order acquisition software
//!   plausibly writes it (each pattern marked documented or assumed), so that reading growing
//!   files can be tested without an instrument.
//! - [`watch`] is a polling directory watcher: one [`watch::WatchEvent`] per new data set,
//!   plane, scan or sweep, per completed or stalled data set, per QC finding and per error.
//! - [`synth`] builds a small synthetic OME-TIFF for tests and demos.
//! - [`qc`] computes live QC metrics (saturation, focus-drift proxy, dropped frames, TIC drop)
//!   and evaluates declarative TOML rules over them.
//!
//! Whether a single file is still being written is decided in
//! [`openreadout_core::live`] from the evidence each reader reports. See `book/src/guides/lab-shares.md`.
//!
//! # Example
//!
//! ```no_run
//! use openreadout_core::Registry;
//! use openreadout_live::watch::{WatchOptions, Watcher};
//!
//! // Register readers: `Registry::new().with(Box::new(openreadout_nd2::Nd2Reader))`.
//! let registry = Registry::new();
//! let mut options = WatchOptions::default();
//! options.roots.push("/data/incoming".into());
//! let mut watcher = Watcher::new(options);
//! loop {
//!     watcher.poll(&registry, &mut |event| {
//!         println!("{}", serde_json::to_string(&event).unwrap());
//!     });
//!     std::thread::sleep(std::time::Duration::from_millis(250));
//! }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod qc;
pub mod replay;
pub mod synth;
pub mod watch;

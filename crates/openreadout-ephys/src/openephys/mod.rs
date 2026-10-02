//! Open Ephys recordings: the binary format (`structure.oebin` + `continuous.dat` + `.npy`) and the
//! legacy one-file-per-channel format (`.continuous`, `.events`, `.spikes`). Notes:
//! `docs/formats/open-ephys.md`.

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub mod binary;
pub mod dataset;
pub mod legacy;
pub mod npy;
#[cfg(test)]
mod tests;

/// Format id of Open Ephys recordings.
pub const OPEN_EPHYS_FORMAT_ID: &str = "open-ephys";

/// The Open Ephys reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenEphysReader;

fn dir_has(input: &Input, depth: usize, pred: &dyn Fn(&str) -> bool) -> bool {
    let fs = input.fs();
    let mut stack = vec![(input.path().to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, k)) = stack.pop() {
        let Ok(rd) = fs.read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(std::result::Result::ok) {
            seen += 1;
            if seen > 20_000 {
                return false;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if e.metadata().is_ok_and(|m| m.is_dir()) {
                if k < depth {
                    stack.push((e.path(), k + 1));
                }
            } else if pred(&name) {
                return true;
            }
        }
    }
    false
}

/// Every entry of the directory is part of an Open Ephys recording: `Record Node …` (any
/// spelling), `experiment…`, `recording…`, `continuous`, `events`, `spikes` folders; recording
/// files (`.continuous`, `.events`, `.spikes`, `.oebin`, `.openephys`, `.xml`, `.npy`,
/// `sync_messages.txt`) and notes (`.txt`, `.md`, `.json`, `.ok`); hidden entries are ignored.
fn only_open_ephys_entries(input: &Input) -> bool {
    let Ok(rd) = input.fs().read_dir(input.path()) else {
        return false;
    };
    let mut any = false;
    for e in rd.filter_map(std::result::Result::ok) {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with('.') {
            continue;
        }
        any = true;
        let ok = if e.metadata().is_ok_and(|m| m.is_dir()) {
            name.starts_with("record")
                || name.starts_with("experiment")
                || matches!(name.as_str(), "continuous" | "events" | "spikes")
        } else {
            name.rsplit_once('.').is_some_and(|(_, ext)| {
                matches!(
                    ext,
                    "continuous"
                        | "events"
                        | "spikes"
                        | "oebin"
                        | "openephys"
                        | "xml"
                        | "npy"
                        | "txt"
                        | "md"
                        | "json"
                        | "ok"
                )
            })
        };
        if !ok {
            return false;
        }
    }
    any
}

impl FormatReader for OpenEphysReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::OPEN_EPHYS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OPEN_EPHYS_FORMAT_ID.into(),
            name: "Open Ephys recording (binary or Open Ephys format)".into(),
            vendor: "Open Ephys GUI".into(),
            extensions: vec!["oebin".into(), "continuous".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::OPEN_EPHYS.confidence,
            known_gaps: vec![
                "Input is a recording directory (or its structure.oebin / a .continuous file); NWB-format Open Ephys recordings are read by the NWB reader".into(),
                "Binary format: spike groups and binary (tracking) event streams are listed, not decoded; synchronized timestamps are reported per sweep, samples are not resampled".into(),
                "Channel units come from structure.oebin as recorded; when empty, µV for neural and V for ADC channels (Open Ephys docs)".into(),
                "Legacy format: records that are not full 1024-sample records on a regular clock are refused per channel; gaps and recording numbers split sweeps (Neo zero-fills gaps)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if input.is_dir() {
            // Only a level of the Open Ephys hierarchy itself is claimed (not a share that holds
            // recordings somewhere below): every entry must be one of its folders or files.
            if only_open_ephys_entries(input)
                && (dir_has(input, 4, &|n| n == "structure.oebin")
                    || dir_has(input, 1, &|n| n.ends_with(".continuous")))
            {
                return Some(Detection {
                    format_id: OPEN_EPHYS_FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: Some("Open Ephys recording directory (read as one session)".into()),
                });
            }
            return None;
        }
        if path.file_name().is_some_and(|n| n == "structure.oebin") {
            return Some(Detection {
                format_id: OPEN_EPHYS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("reads the recording directory that holds it".into()),
            });
        }
        if has_extension(path, &["continuous"]) && legacy::looks_like_legacy(head) {
            return Some(Detection {
                format_id: OPEN_EPHYS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("reads every channel file in the same directory".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::OpenEphysDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(dataset::OpenEphysDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

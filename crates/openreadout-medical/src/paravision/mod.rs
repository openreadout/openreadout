//! Bruker ParaVision reader: reconstructed MRI data (`pdata/<procno>/2dseq`) described by
//! `visu_pars`, with the acquisition's `method` and `acqp`, in an experiment folder
//! (`<expno>/`) or a whole study folder.
//!
//! Format notes and vocabulary: `docs/formats/bruker-paravision.md`. Provenance:
//! `docs/provenance/bruker-paravision.md`.

mod dataset;
mod params;

use std::path::{Path, PathBuf};

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader};
use openreadout_core::source::{Fs, Input};

use dataset::ParavisionDataset;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "bruker-paravision";

/// The Bruker ParaVision reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ParavisionReader;

/// A reconstruction: an experiment number, a processing number and their folders.
#[derive(Debug, Clone)]
pub(crate) struct Reco {
    pub(crate) expno: String,
    pub(crate) procno: String,
    pub(crate) exp_dir: PathBuf,
    pub(crate) proc_dir: PathBuf,
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn sorted_subdirs(fs: &Fs, dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir()))
        .map(|e| e.path())
        .collect();
    // numeric names in numeric order
    v.sort_by_key(|p| {
        let n = name_of(p);
        (n.parse::<u64>().unwrap_or(u64::MAX), n)
    });
    v
}

/// The reconstructions of an experiment folder (`<expno>/pdata/<procno>/2dseq`).
fn recos_of_exp(fs: &Fs, exp: &Path) -> Vec<Reco> {
    let pdata = exp.join("pdata");
    sorted_subdirs(fs, &pdata)
        .into_iter()
        .filter(|p| fs.is_file(&p.join("2dseq")))
        .map(|p| Reco {
            expno: name_of(exp),
            procno: name_of(&p),
            exp_dir: exp.to_path_buf(),
            proc_dir: p,
        })
        .collect()
}

/// What `path` is: a `2dseq` file, a `pdata/<procno>` folder, an experiment folder or a study
/// folder, and the reconstructions it holds.
pub(crate) fn recos(fs: &Fs, path: &Path) -> Option<Vec<Reco>> {
    if fs.is_file(path) {
        if !name_of(path).eq_ignore_ascii_case("2dseq") {
            return None;
        }
        let proc = path.parent()?;
        let exp = proc.parent()?.parent()?;
        return Some(vec![Reco {
            expno: name_of(exp),
            procno: name_of(proc),
            exp_dir: exp.to_path_buf(),
            proc_dir: proc.to_path_buf(),
        }]);
    }
    if fs.is_file(&path.join("2dseq")) && fs.is_file(&path.join("visu_pars")) {
        let exp = path.parent()?.parent()?;
        return Some(vec![Reco {
            expno: name_of(exp),
            procno: name_of(path),
            exp_dir: exp.to_path_buf(),
            proc_dir: path.to_path_buf(),
        }]);
    }
    if fs.is_file(&path.join("acqp")) || fs.is_file(&path.join("method")) {
        let r = recos_of_exp(fs, path);
        return (!r.is_empty()).then_some(r);
    }
    // a study: experiment folders with acqp or method
    if fs.is_file(&path.join("subject")) || fs.is_file(&path.join("ResultState")) {
        let r: Vec<Reco> = sorted_subdirs(fs, path)
            .iter()
            .filter(|e| fs.is_file(&e.join("acqp")) || fs.is_file(&e.join("method")))
            .flat_map(|e| recos_of_exp(fs, e))
            .collect();
        return (!r.is_empty()).then_some(r);
    }
    None
}

impl FormatReader for ParavisionReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::PARAVISION)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Bruker ParaVision (2dseq)".into(),
            vendor: "Bruker BioSpin preclinical MRI".into(),
            extensions: Vec::new(),
            family: "medical-imaging".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::PARAVISION.confidence,
            known_gaps: vec![
                "Raw k-space (fid, rawdata.job*) is not reconstructed; only pdata/<procno>/2dseq is read".into(),
                "Frames stored transposed (VisuCoreTransposition) are returned as stored".into(),
                "ParaVision 5 data without visu_pars are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        let r = recos(input.fs(), input.path())?;
        let any_visu = r
            .iter()
            .any(|r| input.fs().is_file(&r.proc_dir.join("visu_pars")));
        any_visu.then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ParavisionDataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

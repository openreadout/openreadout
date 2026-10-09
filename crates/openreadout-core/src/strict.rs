//! `--strict`: a dataset wrapper that refuses outputs its file's assurance does not validate.
//!
//! [`crate::Registry::strict`] wraps every dataset it opens in a [`StrictDataset`]. The wrapper
//! assesses the file once ([`crate::assurance::assess_dataset`]) and then answers each request by
//! scope: image planes need `pixels`, spectra and scan headers `spectra`, sweeps `traces`, table
//! rows `tables`, and `info`, the vendor tree and per-frame records `metadata`. A refused scope
//! fails with exit 6 (`unsupported_feature`) and a hint. Integrity checks (`check`), the structural
//! listing (`info --view structure`), provenance and attachments are never refused: they report on
//! the file rather than returning its measured values.

use std::path::PathBuf;

use crate::assurance::{Assurance, Observations, Scope};
use crate::error::{Error, Result};
use crate::model::{AttachmentInfo, CheckReport, FileInfo, LsEntry, Spectrum, Table, Trace};
use crate::pixel::Plane;
use crate::provenance::ProvenanceMap;
use crate::reader::{Dataset, PlaneIndex, SpectrumView};

/// A dataset that refuses unvalidated outputs (see the module docs).
pub struct StrictDataset {
    inner: Box<dyn Dataset>,
    format_id: String,
    /// `None` when `info` itself failed: then every measured output is refused with that error.
    assurance: Option<Assurance>,
    info_error: Option<String>,
}

impl std::fmt::Debug for StrictDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrictDataset")
            .field("format_id", &self.format_id)
            .field("assurance", &self.assurance.as_ref().map(|a| a.level))
            .finish_non_exhaustive()
    }
}

impl StrictDataset {
    /// Assess `inner` and wrap it.
    pub fn wrap(inner: Box<dyn Dataset>) -> Result<Box<dyn Dataset>> {
        let (format_id, assurance, info_error) = match inner.info() {
            Ok(info) => {
                let a = crate::assurance::assess_dataset(inner.as_ref(), &info);
                (info.format.id.clone(), Some(a), None)
            }
            Err(e) => (String::from("unknown"), None, Some(e.to_string())),
        };
        Ok(Box::new(StrictDataset {
            inner,
            format_id,
            assurance,
            info_error,
        }))
    }

    /// The file's assurance, when `info` could be read.
    pub fn assurance(&self) -> Option<&Assurance> {
        self.assurance.as_ref()
    }

    fn gate(&self, scope: Scope) -> Result<()> {
        match (&self.assurance, &self.info_error) {
            (Some(a), _) => match a.strict_error(&self.format_id, scope) {
                Some(e) => Err(e),
                None => Ok(()),
            },
            (None, Some(msg)) => Err(Error::Unsupported {
                format: "strict",
                feature: format!(
                    "returning {} from a file whose header could not be read ({msg})",
                    scope.as_str()
                ),
                hint: Some(
                    "--strict needs the file's header to assess it; run `openreadout check FILE` to see what is wrong."
                        .into(),
                ),
            }),
            (None, None) => Ok(()),
        }
    }
}

impl Dataset for StrictDataset {
    fn info(&self) -> Result<FileInfo> {
        self.gate(Scope::Metadata)?;
        self.inner.info()
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        self.gate(Scope::Metadata)?;
        self.inner.vendor_metadata()
    }
    fn provenance(&self) -> ProvenanceMap {
        self.inner.provenance()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.inner.entries()
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.gate(Scope::Pixels)?;
        self.inner.read_plane(image, index)
    }
    fn check(&mut self) -> Result<CheckReport> {
        self.inner.check()
    }
    fn check_headers(&mut self) -> Result<CheckReport> {
        self.inner.check_headers()
    }
    fn member_files(&self) -> Vec<PathBuf> {
        self.inner.member_files()
    }
    fn write_state(&self) -> Option<crate::live::WriteState> {
        self.inner.write_state()
    }
    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        self.gate(Scope::Tables)?;
        self.inner.read_table(index, first_row, max_rows)
    }
    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        self.gate(Scope::Traces)?;
        self.inner
            .read_trace(index, sweep, first_sample, max_samples)
    }
    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.gate(Scope::Spectra)?;
        self.inner.read_spectrum(index, spectrum)
    }
    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        self.gate(Scope::Spectra)?;
        self.inner.spectrum_ms_levels(run)
    }
    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        self.gate(Scope::Spectra)?;
        self.inner.find_spectrum(run, scan_number)
    }
    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(crate::scans::ScanHeader) -> bool,
    ) -> Result<bool> {
        self.gate(Scope::Spectra)?;
        self.inner.visit_scan_headers(run, first, visit)
    }
    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        self.gate(Scope::Spectra)?;
        self.inner.read_spectrum_view(index, spectrum, view)
    }
    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        self.gate(Scope::Pixels)?;
        self.inner.read_plane_level(image, index, level)
    }
    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: crate::region::Region,
    ) -> Result<Plane> {
        self.gate(Scope::Pixels)?;
        self.inner.read_region(image, index, level, region)
    }
    fn reads_strips(&self, image: u32, level: u32) -> bool {
        self.inner.reads_strips(image, level)
    }
    fn read_strips(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        strips: &[crate::region::Region],
        sink: &mut dyn FnMut(Plane) -> Result<()>,
    ) -> Result<()> {
        self.gate(Scope::Pixels)?;
        self.inner.read_strips(image, index, level, strips, sink)
    }
    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        self.inner.attachments()
    }
    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        self.inner.read_attachment(index)
    }
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<serde_json::Value>)> {
        self.gate(Scope::Metadata)?;
        self.inner.frames(image, limit)
    }
    fn experiment(&self) -> Option<crate::experiment::Experiment> {
        self.inner.experiment()
    }
    fn plate(&self) -> Option<crate::plate::PlateSummary> {
        self.inner.plate()
    }
    fn assurance_observations(&self) -> Observations {
        self.inner.assurance_observations()
    }
    fn file_assurance(&self) -> Option<Assurance> {
        self.assurance.clone()
    }
    fn is_strict(&self) -> bool {
        true
    }
    fn exclude_flagged_peaks(&mut self, exclude: bool) -> bool {
        self.inner.exclude_flagged_peaks(exclude)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assurance::{
        AssuranceLevel, AssuranceProfile, Basis, FeatureKind, Validated, register_profile, row,
    };
    use crate::model::FormatDescriptor;
    use crate::pixel::PixelType;
    use crate::provenance::Confidence;

    const ROWS: &[Validated] = &[row(FeatureKind::FormatVersion, "2", 3, 2, 3)];

    fn observe(info: &FileInfo) -> Observations {
        let mut o = Observations::default();
        if let Some(v) = &info.format_version {
            o.feature(FeatureKind::FormatVersion, v, &[Scope::Pixels]);
        }
        o
    }

    static PROFILE: AssuranceProfile = AssuranceProfile {
        format_id: "strict-test",
        observe,
        validated: ROWS,
        confidence: Confidence::Medium,
        basis: Basis::ReverseEngineered,
    };

    /// A one-image file whose format version decides whether its pixels are validated.
    struct Fake(&'static str);
    impl Dataset for Fake {
        fn info(&self) -> Result<FileInfo> {
            Ok(FileInfo {
                path: "x".into(),
                size_bytes: 1,
                format: FormatDescriptor {
                    id: "strict-test".into(),
                    name: "Strict test".into(),
                    vendor: String::new(),
                    extensions: vec![],
                    family: "microscopy".into(),
                    can_read: true,
                    can_write: false,
                    confidence: Confidence::Medium,
                    known_gaps: vec![],
                },
                format_version: Some(self.0.into()),
                images: vec![crate::model::ImageInfo::new(0, 1, 1, PixelType::Uint8)],
                tables: vec![],
                spectra: vec![],
                traces: vec![],
                plane_count: 1,
                notes: vec![],
            })
        }
        fn vendor_metadata(&self) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> ProvenanceMap {
            ProvenanceMap::new()
        }
        fn entries(&self) -> Result<Vec<LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
            Ok(Plane {
                width: 1,
                height: 1,
                pixel_type: PixelType::Uint8,
                samples_per_pixel: 1,
                data: vec![7],
            })
        }
        fn check(&mut self) -> Result<CheckReport> {
            Ok(CheckReport::new("x", "strict-test"))
        }
    }

    #[test]
    fn a_validated_variant_is_read() {
        register_profile(&PROFILE);
        let mut ds = StrictDataset::wrap(Box::new(Fake("2"))).unwrap();
        assert!(ds.info().is_ok());
        assert_eq!(
            ds.read_plane(0, PlaneIndex::default()).unwrap().data,
            vec![7]
        );
        assert_eq!(
            ds.file_assurance().unwrap().level,
            AssuranceLevel::Validated
        );
    }

    #[test]
    fn an_unvalidated_variant_is_refused_where_it_matters() {
        register_profile(&PROFILE);
        let mut ds = StrictDataset::wrap(Box::new(Fake("3"))).unwrap();
        // The format version affects pixels only in this profile: metadata is still served.
        assert!(ds.info().is_ok());
        let e = ds.read_plane(0, PlaneIndex::default()).unwrap_err();
        assert_eq!(e.exit_code(), 6);
        assert!(e.to_string().contains("pixels"), "{e}");
        assert!(e.hint().unwrap().contains("--strict"));
        // Integrity checks and the assessment itself are never refused.
        assert!(ds.check().unwrap().ok);
        assert_eq!(
            ds.file_assurance().unwrap().strict_refuses,
            vec![Scope::Pixels]
        );
    }
}

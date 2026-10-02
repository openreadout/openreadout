//! Progress reporting for long exports without touching the writers: a [`Dataset`] wrapper that
//! counts plane and spectrum reads and reports `(done, total)` to a callback.

use openreadout_core::Result;
use openreadout_core::model::{
    AttachmentInfo, CheckReport, FileInfo, LsEntry, Spectrum, Table, Trace,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, PlaneIndex, SpectrumView};

/// Called with `(units done, total units)`.
pub type ProgressFn = Box<dyn FnMut(u64, u64) + Send>;

/// Forwards every call to `inner`; each plane or spectrum read counts one unit.
pub struct ProgressDataset<'a> {
    inner: &'a mut dyn Dataset,
    total: u64,
    done: u64,
    report: ProgressFn,
}

impl<'a> ProgressDataset<'a> {
    pub fn new(inner: &'a mut dyn Dataset, total: u64, report: ProgressFn) -> Self {
        ProgressDataset {
            inner,
            total,
            done: 0,
            report,
        }
    }

    fn tick(&mut self) {
        self.done += 1;
        let d = self.done.min(self.total);
        (self.report)(d, self.total);
    }
}

impl Dataset for ProgressDataset<'_> {
    fn info(&self) -> Result<FileInfo> {
        self.inner.info()
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        self.inner.vendor_metadata()
    }
    fn provenance(&self) -> ProvenanceMap {
        self.inner.provenance()
    }
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        self.inner.assurance_observations()
    }
    fn file_assurance(&self) -> Option<openreadout_core::assurance::Assurance> {
        self.inner.file_assurance()
    }
    fn is_strict(&self) -> bool {
        self.inner.is_strict()
    }
    fn exclude_flagged_peaks(&mut self, exclude: bool) -> bool {
        self.inner.exclude_flagged_peaks(exclude)
    }
    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        self.inner.experiment()
    }
    fn check_headers(&mut self) -> Result<CheckReport> {
        self.inner.check_headers()
    }
    fn member_files(&self) -> Vec<std::path::PathBuf> {
        self.inner.member_files()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.inner.entries()
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let p = self.inner.read_plane(image, index);
        self.tick();
        p
    }
    fn check(&mut self) -> Result<CheckReport> {
        self.inner.check()
    }
    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        self.inner.read_table(index, first_row, max_rows)
    }
    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        self.inner
            .read_trace(index, sweep, first_sample, max_samples)
    }
    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        let s = self.inner.read_spectrum(index, spectrum);
        self.tick();
        s
    }
    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        self.inner.find_spectrum(run, scan_number)
    }
    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        self.inner.spectrum_ms_levels(run)
    }
    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        self.inner.visit_scan_headers(run, first, visit)
    }
    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        let s = self.inner.read_spectrum_view(index, spectrum, view);
        self.tick();
        s
    }
    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        let p = self.inner.read_plane_level(image, index, level);
        self.tick();
        p
    }
    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: openreadout_core::Region,
    ) -> Result<Plane> {
        let p = self.inner.read_region(image, index, level, region);
        self.tick();
        p
    }
    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        self.inner.attachments()
    }
    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        self.inner.read_attachment(index)
    }
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<serde_json::Value>)> {
        self.inner.frames(image, limit)
    }
}

/// Planes an image export will read: every selected (c, z, t) of every selected image.
pub fn planes_to_export(info: &FileInfo, image: Option<u32>, select: &[String]) -> u64 {
    let Ok(sel) = openreadout_core::select::Selection::parse(select) else {
        return 0;
    };
    let count = |axis: &[u32], size: u32| -> u64 {
        if axis.is_empty() {
            u64::from(size)
        } else {
            axis.iter().filter(|&&i| i < size).count() as u64
        }
    };
    info.images
        .iter()
        .filter(|im| image.is_none_or(|i| i == im.index))
        .map(|im| count(&sel.c, im.size_c) * count(&sel.z, im.size_z) * count(&sel.t, im.size_t))
        .sum()
}

/// Wrap a callback so it fires only when the integer percentage changes.
pub fn throttled(mut f: impl FnMut(u64, u64) + Send + 'static) -> ProgressFn {
    let mut last = u64::MAX;
    Box::new(move |done, total| {
        let pct = done.saturating_mul(100).checked_div(total).unwrap_or(100);
        if pct != last {
            last = pct;
            f(done, total);
        }
    })
}

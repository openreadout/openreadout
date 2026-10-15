//! Progress reporting. [`Notifier`] turns `(done, total)` counts into MCP progress
//! notifications with a message a host can show as is ("Reading 1,200 of 53,141 spectra"),
//! sent only when the client gave a `progressToken`. [`ProgressDataset`] reports exports
//! without touching the writers: it wraps a [`Dataset`] and counts plane and spectrum reads
//! (or, for table and trace exports, rows and samples).

use openreadout_core::Result;
use openreadout_core::model::{
    AttachmentInfo, CheckReport, FileInfo, LsEntry, Spectrum, Table, Trace,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, PlaneIndex, SpectrumView};

/// `12345` as `12,345`.
pub fn group(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::with_capacity(d.len() + d.len() / 3);
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A progress message: `Reading 1,200 of 53,141 spectra`, or `Indexing: 1,200 data sets read`
/// without a total.
pub fn message(what: &str, done: u64, total: u64, unit: &str) -> String {
    if total > 0 {
        format!("{what} {} of {} {unit}", group(done), group(total))
    } else {
        format!("{what}: {} {unit}", group(done))
    }
}

/// Sends progress notifications for one tool call, when the client asked for them.
#[derive(Clone)]
pub struct Notifier {
    tx: tokio::sync::mpsc::UnboundedSender<(u64, u64, String)>,
}

/// The task forwarding a call's notifications to the client; await it before answering.
pub struct Forwarder(Option<tokio::task::JoinHandle<()>>);

impl Forwarder {
    /// Wait until every notification is sent.
    pub async fn finish(self) {
        if let Some(t) = self.0 {
            let _ = t.await;
        }
    }
}

impl Notifier {
    /// A notifier for the call `ctx`, or `None` when the client gave no `progressToken`.
    pub fn for_call(
        ctx: &rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> (Option<Notifier>, Forwarder) {
        let Some(tok) = ctx.meta.get_progress_token() else {
            return (None, Forwarder(None));
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64, String)>();
        let peer = ctx.peer.clone();
        let task = tokio::spawn(async move {
            while let Some((done, total, msg)) = rx.recv().await {
                let mut p = rmcp::model::ProgressNotificationParam::new(tok.clone(), done as f64)
                    .with_message(msg);
                if total > 0 {
                    p = p.with_total(total as f64);
                }
                let _ = peer.notify_progress(p).await;
            }
        });
        (Some(Notifier { tx }), Forwarder(Some(task)))
    }

    /// Send one notification now.
    pub fn send(&self, done: u64, total: u64, msg: String) {
        let _ = self.tx.send((done, total, msg));
    }

    /// A counter for a writer or reader loop: each call sends `what done of total unit`, at
    /// most once per percent.
    pub fn counter(&self, what: impl Into<String>, unit: &'static str) -> ProgressFn {
        let n = self.clone();
        let what = what.into();
        throttled(move |done, total| n.send(done, total, message(&what, done, total, unit)))
    }

    /// The same as a shared callback for [`openreadout_core::parallel::ReadContext`].
    pub fn shared(
        &self,
        what: impl Into<String>,
        unit: &'static str,
    ) -> Box<dyn Fn(u64, u64) + Send + Sync> {
        let n = self.clone();
        let what = what.into();
        let last = std::sync::Mutex::new(u64::MAX);
        Box::new(move |done, total| {
            let pct = done.saturating_mul(100).checked_div(total).unwrap_or(100);
            if let Ok(mut l) = last.lock()
                && *l != pct
            {
                *l = pct;
                n.send(done, total, message(&what, done, total, unit));
            }
        })
    }
}

/// Called with `(units done, total units)`.
pub type ProgressFn = Box<dyn FnMut(u64, u64) + Send>;

/// Forwards every call to `inner`; each plane or spectrum read counts one unit, and with
/// [`ProgressDataset::rows`] each table row and trace sample read counts one unit too.
pub struct ProgressDataset<'a> {
    inner: &'a mut dyn Dataset,
    total: u64,
    done: u64,
    report: ProgressFn,
    rows: bool,
}

impl<'a> ProgressDataset<'a> {
    pub fn new(inner: &'a mut dyn Dataset, total: u64, report: ProgressFn) -> Self {
        ProgressDataset {
            inner,
            total,
            done: 0,
            report,
            rows: false,
        }
    }

    /// Also count the rows of table reads and the samples of trace reads.
    #[cfg_attr(not(feature = "parquet"), allow(dead_code))]
    pub fn rows(inner: &'a mut dyn Dataset, total: u64, report: ProgressFn) -> Self {
        ProgressDataset {
            rows: true,
            ..Self::new(inner, total, report)
        }
    }

    fn tick(&mut self) {
        self.tick_by(1);
    }

    fn tick_by(&mut self, n: u64) {
        self.done = self.done.saturating_add(n);
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
    fn set_lock_mass(&mut self, reference_mz: Option<f64>) -> openreadout_core::Result<bool> {
        self.inner.set_lock_mass(reference_mz)
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
        let t = self.inner.read_table(index, first_row, max_rows);
        if self.rows
            && let Ok(t) = &t
        {
            self.tick_by(t.columns.first().map_or(0, |c| c.len() as u64));
        }
        t
    }
    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let t = self
            .inner
            .read_trace(index, sweep, first_sample, max_samples);
        if self.rows
            && let Ok(t) = &t
        {
            self.tick_by(t.channels.first().map_or(0, |c| c.len() as u64));
        }
        t
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
    fn stored_areas(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Option<openreadout_core::Region>,
    ) -> Result<Option<Vec<openreadout_core::Region>>> {
        self.inner.stored_areas(image, index, level, region)
    }
    fn reads_strips(&self, image: u32, level: u32) -> bool {
        self.inner.reads_strips(image, level)
    }
    fn read_strips(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        strips: &[openreadout_core::Region],
        sink: &mut dyn FnMut(Plane) -> Result<()>,
    ) -> Result<()> {
        let r = self.inner.read_strips(image, index, level, strips, sink);
        self.tick();
        r
    }
    fn pixels_refused(&self, image: u32) -> Option<String> {
        self.inner.pixels_refused(image)
    }
    fn z_planes_per_chunk(&self, image: u32, level: u32) -> u32 {
        self.inner.z_planes_per_chunk(image, level)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_read_naturally() {
        assert_eq!(group(53141), "53,141");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000_000), "1,000,000");
        assert_eq!(
            message("Reading", 1200, 53141, "spectra"),
            "Reading 1,200 of 53,141 spectra"
        );
        assert_eq!(
            message("Indexing", 12, 0, "data sets read"),
            "Indexing: 12 data sets read"
        );
    }
}

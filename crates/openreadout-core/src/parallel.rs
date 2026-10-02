//! Parallel plane decoding with ordered delivery.
//!
//! [`Dataset::read_plane`] takes `&mut self`, so one dataset handle decodes one plane at a
//! time. To decode several planes at once, workers take a handle from a small pool (the
//! caller's own dataset first, then extra handles opened on the same file through an
//! [`Opener`]), decode on the global rayon pool (sized by `--threads`), and the results are
//! handed to the caller's `sink` strictly in request order. Writers stay sequential, so the
//! bytes they produce do not depend on the number of threads.
//!
//! Memory: at most two windows of decoded planes are alive at once (the one being decoded
//! and the one being consumed). A window holds at most one plane per thread and at most
//! [`IN_FLIGHT_BYTES`] of estimated plane data, and never fewer than one plane.

use std::sync::{Mutex, PoisonError, mpsc};

use rayon::prelude::*;

use crate::reader::{Dataset, PlaneIndex};
use crate::{Plane, Result};

/// Opens another, independent handle on the file being read (for worker threads).
pub type Opener<'a> = dyn Fn() -> Result<Box<dyn Dataset>> + Sync + 'a;

/// Progress callback: `(done, total)`, called on the caller's thread after each item.
pub type Progress<'a> = dyn Fn(u64, u64) + Sync + 'a;

/// Upper bound on the estimated decoded bytes of one window of planes.
pub const IN_FLIGHT_BYTES: u64 = 1 << 30;

/// Optional helpers for long-running plane loops (export, `check --planes`, `stats`).
#[derive(Clone, Copy, Default)]
pub struct ReadContext<'a> {
    /// Opens extra handles for parallel decoding. `None` decodes sequentially.
    pub opener: Option<&'a Opener<'a>>,
    /// Called after each plane has been consumed.
    pub progress: Option<&'a Progress<'a>>,
}

impl std::fmt::Debug for ReadContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadContext")
            .field("opener", &self.opener.is_some())
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

impl ReadContext<'_> {
    /// Report progress, if a callback is set.
    pub fn report(&self, done: u64, total: u64) {
        if let Some(p) = self.progress {
            p(done, total);
        }
    }
}

/// One plane to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneRequest {
    /// Image index.
    pub image: u32,
    /// Which plane (c, z, t).
    pub index: PlaneIndex,
    /// Pyramid level (0 = full resolution).
    pub level: u32,
    /// Only this rectangle of the plane (in the level's pixel coordinates); `None` = all.
    pub region: Option<crate::region::Region>,
}

impl PlaneRequest {
    /// The whole plane `index` of `image` at `level`.
    pub fn plane(image: u32, index: PlaneIndex, level: u32) -> Self {
        PlaneRequest {
            image,
            index,
            level,
            region: None,
        }
    }

    /// Read this request from `ds`: the whole plane, or its region.
    pub fn read(&self, ds: &mut dyn Dataset) -> Result<Plane> {
        match self.region {
            None => ds.read_plane_level(self.image, self.index, self.level),
            Some(r) => ds.read_region(self.image, self.index, self.level, r),
        }
    }
}

enum Handle<'d> {
    Borrowed(&'d mut dyn Dataset),
    Owned(Box<dyn Dataset>),
}

impl Handle<'_> {
    fn get(&mut self) -> &mut dyn Dataset {
        match self {
            Handle::Borrowed(d) => &mut **d,
            Handle::Owned(d) => d.as_mut(),
        }
    }
}

/// Number of planes decoded concurrently for planes of about `plane_bytes` bytes.
pub fn window(plane_bytes: u64) -> usize {
    let threads = rayon::current_num_threads().max(1);
    let by_memory = (IN_FLIGHT_BYTES / plane_bytes.max(1)).max(1);
    threads.min(usize::try_from(by_memory).unwrap_or(usize::MAX))
}

/// Read `requests`, transform each plane with `map` (on a worker thread) and hand the results
/// to `sink` in request order. Errors are reported in order too: the first failing request
/// (by position) is the error returned, after every earlier result has been consumed.
///
/// `plane_bytes` is an estimate of one decoded plane (it sizes the window). Decoding is
/// sequential on `primary` when no opener is given, the pool has one thread, or the window
/// is a single plane.
pub fn read_in_order<T: Send>(
    primary: &mut dyn Dataset,
    ctx: &ReadContext<'_>,
    requests: &[PlaneRequest],
    plane_bytes: u64,
    map: &(dyn Fn(&PlaneRequest, Plane) -> Result<T> + Sync),
    sink: &mut dyn FnMut(usize, T) -> Result<()>,
) -> Result<()> {
    let window = window(plane_bytes);
    let opener = match ctx.opener {
        Some(o) if window > 1 && requests.len() > 1 => o,
        _ => {
            for (i, r) in requests.iter().enumerate() {
                let p = r.read(primary)?;
                sink(i, map(r, p)?)?;
            }
            return Ok(());
        }
    };
    let pool: Mutex<Vec<Handle<'_>>> = Mutex::new(vec![Handle::Borrowed(primary)]);
    let take = || -> Result<Handle<'_>> {
        let h = pool.lock().unwrap_or_else(PoisonError::into_inner).pop();
        match h {
            Some(h) => Ok(h),
            None => Ok(Handle::Owned(opener()?)),
        }
    };
    let decode = |r: &PlaneRequest| -> Result<T> {
        let mut h = take()?;
        let res = r.read(h.get()).and_then(|p| map(r, p));
        pool.lock().unwrap_or_else(PoisonError::into_inner).push(h);
        res
    };
    std::thread::scope(|s| {
        // Rendezvous channel: the producer decodes window k+1 while the caller consumes k.
        let (tx, rx) = mpsc::sync_channel::<Vec<Result<T>>>(0);
        s.spawn(move || {
            for chunk in requests.chunks(window) {
                let out: Vec<Result<T>> = chunk.par_iter().map(&decode).collect();
                let failed = out.iter().any(Result::is_err);
                if tx.send(out).is_err() || failed {
                    break;
                }
            }
        });
        let mut i = 0usize;
        for chunk in rx {
            for r in chunk {
                sink(i, r?)?;
                i += 1;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CheckReport, FileInfo, LsEntry};
    use crate::{Error, PixelType, ProvenanceMap};

    /// Plane (c) is filled with `c`; plane 5 fails.
    struct Fake;
    impl Dataset for Fake {
        fn info(&self) -> Result<FileInfo> {
            Err(Error::Other("unused".into()))
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
        fn read_plane(&mut self, _: u32, i: PlaneIndex) -> Result<Plane> {
            if i.c == 5 {
                return Err(Error::corrupt("fake", "plane 5"));
            }
            Ok(Plane {
                width: 2,
                height: 1,
                pixel_type: PixelType::Uint8,
                samples_per_pixel: 1,
                data: vec![i.c as u8; 2],
            })
        }
        fn check(&mut self) -> Result<CheckReport> {
            Ok(CheckReport::new("fake", "fake"))
        }
    }

    fn reqs(n: u32) -> Vec<PlaneRequest> {
        (0..n)
            .map(|c| PlaneRequest::plane(0, PlaneIndex { c, z: 0, t: 0 }, 0))
            .collect()
    }

    #[test]
    fn delivers_in_order_and_stops_at_first_error() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        pool.install(|| {
            let opener = || -> Result<Box<dyn Dataset>> { Ok(Box::new(Fake)) };
            let ctx = ReadContext {
                opener: Some(&opener),
                progress: None,
            };
            let mut seen = Vec::new();
            let mut ds = Fake;
            read_in_order(
                &mut ds,
                &ctx,
                &reqs(5),
                2,
                &|_, p| Ok(p.data[0]),
                &mut |i, v| {
                    seen.push((i, v));
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(seen, (0..5).map(|i| (i, i as u8)).collect::<Vec<_>>());
            let mut seen = Vec::new();
            let e = read_in_order(
                &mut ds,
                &ctx,
                &reqs(9),
                2,
                &|_, p| Ok(p.data[0]),
                &mut |i, _| {
                    seen.push(i);
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(e.to_string().contains("plane 5"));
            assert_eq!(seen, vec![0, 1, 2, 3, 4]);
        });
    }
}

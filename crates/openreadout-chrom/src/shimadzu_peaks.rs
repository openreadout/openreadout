//! LabSolutions' own peak tables (`docs/formats/shimadzu.md` § Vendor peaks; provenance
//! 2026-09-26).
//!
//! - Older layout: `LC Data Processing/Peak Table-N` (N = chromatogram channel): u32 count,
//!   u32 0, then 280-byte records.
//! - Newer layout: `LSS Data Processing/PT-<data set id>[.<channel>]`: `VER1`, u32 count, 12
//!   bytes, then records of (length − 20) / count bytes whose first 64 bytes have the older
//!   record's layout.
//!
//! Record fields read: u32 flags (+0), u32 retention time in ms (+4), f64 area (+8), f64 height
//! (+24), f64 baseline value at the start (+40) and end (+48), u32 start and end in ms (+56,
//! +60); older layout only: f64 k′ (+208), plate number (+216), plate height (+224), tailing
//! (+232), resolution (+240).

use openreadout_core::bytes::{le_f64, le_u32};

/// Bytes of an older-layout peak record.
pub const PEAK_RECORD_OLD: usize = 280;
/// Most peaks accepted in one table.
pub const MAX_PEAKS: u32 = 100_000;

/// One peak of a vendor peak table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VendorPeak {
    /// Trace (signal) name the table belongs to.
    pub signal: String,
    /// Peak number within its table (1-based).
    pub number: u32,
    pub flags: u32,
    pub rt_min: f64,
    pub start_min: f64,
    pub end_min: f64,
    /// Area in the signal's unit × s (µV·s, µAU·s).
    pub area: f64,
    /// Height in the signal's stored unit (µV, µAU).
    pub height: f64,
    pub baseline_start: f64,
    pub baseline_end: f64,
    /// Older layout only (validated against a vendor export).
    pub capacity_factor: Option<f64>,
    pub plates: Option<f64>,
    pub plate_height: Option<f64>,
    pub tailing: Option<f64>,
    pub resolution: Option<f64>,
}

fn record(r: &[u8], number: u32, older: bool) -> Option<VendorPeak> {
    let ms = |at: usize| le_u32(r, at).map(|v| f64::from(v) / 60_000.0);
    let fin = |v: Option<f64>| v.filter(|x| x.is_finite());
    let p = VendorPeak {
        signal: String::new(),
        number,
        flags: le_u32(r, 0)?,
        rt_min: ms(4)?,
        area: fin(le_f64(r, 8))?,
        height: fin(le_f64(r, 24))?,
        baseline_start: fin(le_f64(r, 40))?,
        baseline_end: fin(le_f64(r, 48))?,
        start_min: ms(56)?,
        end_min: ms(60)?,
        capacity_factor: older.then(|| fin(le_f64(r, 208))).flatten(),
        plates: older.then(|| fin(le_f64(r, 216))).flatten(),
        plate_height: older.then(|| fin(le_f64(r, 224))).flatten(),
        tailing: older.then(|| fin(le_f64(r, 232))).flatten(),
        resolution: older.then(|| fin(le_f64(r, 240))).flatten(),
    };
    // a peak lies inside its integration window
    (p.start_min <= p.rt_min && p.rt_min <= p.end_min).then_some(p)
}

/// The peaks of an older-layout `Peak Table-N` stream.
pub fn peak_table_old(b: &[u8]) -> Result<Vec<VendorPeak>, String> {
    let n = le_u32(b, 0).ok_or("peak table shorter than its header")?;
    if n > MAX_PEAKS {
        return Err(format!("peak table declares {n} peaks"));
    }
    let need = 8 + n as usize * PEAK_RECORD_OLD;
    if b.len() < need {
        return Err(format!(
            "peak table of {n} peaks needs {need} bytes, the stream has {}",
            b.len()
        ));
    }
    (0..n)
        .map(|k| {
            let at = 8 + k as usize * PEAK_RECORD_OLD;
            record(&b[at..at + PEAK_RECORD_OLD], k + 1, true)
                .ok_or_else(|| format!("peak {} is not a valid record", k + 1))
        })
        .collect()
}

/// The peaks of a newer-layout `PT-…` stream.
pub fn peak_table_new(b: &[u8]) -> Result<Vec<VendorPeak>, String> {
    if !b.starts_with(b"VER1") {
        return Err("peak table does not start with VER1".into());
    }
    let n = le_u32(b, 4).ok_or("peak table shorter than its header")?;
    if n == 0 {
        return Ok(Vec::new());
    }
    if n > MAX_PEAKS {
        return Err(format!("peak table declares {n} peaks"));
    }
    let body = b
        .len()
        .checked_sub(20)
        .ok_or("peak table shorter than its header")?;
    let size = body / n as usize;
    if size < 64 || size * n as usize != body {
        return Err(format!(
            "peak table of {n} peaks in {body} bytes has no whole record size"
        ));
    }
    (0..n)
        .map(|k| {
            let at = 20 + k as usize * size;
            record(&b[at..at + size], k + 1, false)
                .ok_or_else(|| format!("peak {} is not a valid record", k + 1))
        })
        .collect()
}

/// PDA channel wavelengths (nm) of `Multi Chromato Table`, channel 1 first: u32 count, then
/// 32-byte records from byte 24 with the wavelength in 1/100 nm at +24.
pub fn multi_chromato_wavelengths(b: &[u8]) -> Vec<Option<f64>> {
    let n = le_u32(b, 0).unwrap_or(0).min(256) as usize;
    (0..n)
        .map(|k| {
            le_u32(b, 24 + 32 * k + 24)
                .filter(|&w| (10_000..=120_000).contains(&w))
                .map(|w| f64::from(w) / 100.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(size: usize, rt: u32, s: u32, e: u32, area: f64) -> Vec<u8> {
        let mut r = vec![0u8; size];
        r[0..4].copy_from_slice(&16u32.to_le_bytes());
        r[4..8].copy_from_slice(&rt.to_le_bytes());
        r[8..16].copy_from_slice(&area.to_le_bytes());
        r[24..32].copy_from_slice(&12.5f64.to_le_bytes());
        r[56..60].copy_from_slice(&s.to_le_bytes());
        r[60..64].copy_from_slice(&e.to_le_bytes());
        r
    }

    #[test]
    fn both_layouts() {
        let mut old = vec![1, 0, 0, 0, 0, 0, 0, 0];
        old.extend(rec(PEAK_RECORD_OLD, 49_982, 34_500, 166_500, 327_205.75));
        let p = peak_table_old(&old).unwrap();
        assert!((p[0].rt_min - 0.833_033).abs() < 1e-5);
        assert!((p[0].area - 327_205.75).abs() < 1e-9);
        assert_eq!(p[0].capacity_factor, Some(0.0));
        let mut new = b"VER1".to_vec();
        new.extend(2u32.to_le_bytes());
        new.extend([0u8; 12]);
        new.extend(rec(792, 60_000, 50_000, 70_000, 1.0));
        new.extend(rec(792, 90_000, 80_000, 95_000, 2.0));
        let p = peak_table_new(&new).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].number, 2);
        assert_eq!(p[1].capacity_factor, None);
        // a peak outside its window, a short table, a ragged record size are errors
        let mut bad = new.clone();
        bad[20 + 4..20 + 8].copy_from_slice(&1u32.to_le_bytes());
        assert!(peak_table_new(&bad).is_err());
        assert!(peak_table_old(&old[..100]).is_err());
        assert!(peak_table_new(&new[..new.len() - 1]).is_err());
        for cut in 0..new.len() {
            let _ = peak_table_new(&new[..cut]);
            let _ = peak_table_old(&old[..cut.min(old.len())]);
        }
    }

    #[test]
    fn pda_channels() {
        let mut b = vec![0u8; 24 + 32 * 3];
        b[0] = 3;
        for (k, w) in [25_400u32, 20_500, 0].iter().enumerate() {
            b[24 + 32 * k + 24..24 + 32 * k + 28].copy_from_slice(&w.to_le_bytes());
        }
        assert_eq!(
            multi_chromato_wavelengths(&b),
            vec![Some(254.0), Some(205.0), None]
        );
    }
}

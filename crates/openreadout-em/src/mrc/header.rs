//! The 1024-byte MRC2014 main header (CCP-EM specification; see `docs/formats/mrc.md`).

use crate::util::{Endian, byte_order_name, text};

/// Size of the main header in bytes.
pub const HEADER_LEN: usize = 1024;

/// The IMOD stamp word (bytes 153–156) marking files whose flag word (157–160) is meaningful.
pub const IMOD_STAMP: i32 = 1_146_047_817;

/// MODE: how one sample is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 0: 8-bit integer (signed per MRC2014; unsigned in IMOD files without the signed flag).
    Int8,
    /// 1: 16-bit signed integer.
    Int16,
    /// 2: 32-bit IEEE float.
    Float32,
    /// 3: complex, two 16-bit integers (transforms).
    ComplexInt16,
    /// 4: complex, two 32-bit floats (transforms).
    ComplexFloat32,
    /// 6: 16-bit unsigned integer.
    Uint16,
    /// 12: 16-bit IEEE half float.
    Float16,
    /// 16: three unsigned bytes per pixel (RGB; IMOD, non-standard).
    Rgb8,
    /// 101: 4-bit values, two per byte (IMOD, non-standard).
    Packed4Bit,
    /// Any other value.
    Unknown(i32),
}

impl Mode {
    pub fn from_code(code: i32) -> Mode {
        match code {
            0 => Mode::Int8,
            1 => Mode::Int16,
            2 => Mode::Float32,
            3 => Mode::ComplexInt16,
            4 => Mode::ComplexFloat32,
            6 => Mode::Uint16,
            12 => Mode::Float16,
            16 => Mode::Rgb8,
            101 => Mode::Packed4Bit,
            other => Mode::Unknown(other),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Mode::Int8 => 0,
            Mode::Int16 => 1,
            Mode::Float32 => 2,
            Mode::ComplexInt16 => 3,
            Mode::ComplexFloat32 => 4,
            Mode::Uint16 => 6,
            Mode::Float16 => 12,
            Mode::Rgb8 => 16,
            Mode::Packed4Bit => 101,
            Mode::Unknown(c) => c,
        }
    }

    /// Our name for the mode, used in `extra.mode_name`.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Int8 => "int8",
            Mode::Int16 => "int16",
            Mode::Float32 => "float32",
            Mode::ComplexInt16 => "complex-int16",
            Mode::ComplexFloat32 => "complex-float32",
            Mode::Uint16 => "uint16",
            Mode::Float16 => "float16",
            Mode::Rgb8 => "rgb8",
            Mode::Packed4Bit => "packed-4-bit",
            Mode::Unknown(_) => "unknown",
        }
    }

    /// Stored bytes of one sample (per component for RGB, per complex value for complex modes);
    /// `None` for the packed 4-bit mode and unknown modes.
    pub fn stored_bytes(self) -> Option<u64> {
        Some(match self {
            Mode::Int8 | Mode::Rgb8 => 1,
            Mode::Int16 | Mode::Uint16 | Mode::Float16 => 2,
            Mode::Float32 | Mode::ComplexInt16 => 4,
            Mode::ComplexFloat32 => 8,
            Mode::Packed4Bit | Mode::Unknown(_) => return None,
        })
    }

    /// True for the modes the MRC2014 specification lists (0, 1, 2, 3, 4, 6, 12, 101).
    pub fn in_spec(self) -> bool {
        !matches!(self, Mode::Rgb8 | Mode::Unknown(_))
    }
}

/// How the NZ sections are organised (ISPG, specification note 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// ISPG 0: a single image or a stack of images (sections → T).
    ImageStack,
    /// ISPG 1–230: one volume (sections → Z).
    Volume,
    /// ISPG 401–630: NZ / MZ volumes of MZ sections each (sections → Z within T).
    VolumeStack,
}

impl Layout {
    pub fn label(self) -> &'static str {
        match self {
            Layout::ImageStack => "image-stack",
            Layout::Volume => "volume",
            Layout::VolumeStack => "volume-stack",
        }
    }
}

/// The decoded main header. Field names follow the specification's variable names in lower case.
#[derive(Debug, Clone, PartialEq)]
pub struct MrcHeader {
    pub nx: i32,
    pub ny: i32,
    pub nz: i32,
    pub mode: Mode,
    pub nxstart: i32,
    pub nystart: i32,
    pub nzstart: i32,
    pub mx: i32,
    pub my: i32,
    pub mz: i32,
    /// CELLA: cell dimensions in ångström.
    pub cell_a: [f32; 3],
    /// CELLB: cell angles in degrees.
    pub cell_b: [f32; 3],
    pub mapc: i32,
    pub mapr: i32,
    pub maps: i32,
    pub dmin: f32,
    pub dmax: f32,
    pub dmean: f32,
    pub ispg: i32,
    pub nsymbt: i32,
    /// EXTTYP as text (trailing NULs and spaces removed; empty when unset).
    pub exttyp: String,
    pub nversion: i32,
    /// ORIGIN (ångström for real-space data).
    pub origin: [f32; 3],
    /// Bytes 209–211 are `MAP`.
    pub has_map_id: bool,
    pub machine_stamp: [u8; 4],
    /// True when values are little-endian (after the machine-stamp checks below).
    pub little_endian: bool,
    /// Why the byte order is not simply the machine stamp's, when it is not.
    pub stamp_problem: Option<String>,
    pub rms: f32,
    pub nlabl: i32,
    /// All ten labels, trimmed (empty strings for unused labels).
    pub labels: Vec<String>,
    /// IMOD stamp present (bytes 153–156).
    pub imod_stamp: bool,
    /// IMOD flag word (bytes 157–160), meaningful only with `imod_stamp`.
    pub imod_flags: i32,
    /// SerialEM/Agard `nint` (bytes 129–130) and `nreal` (131–132) as signed shorts.
    pub nint: i16,
    pub nreal: i16,
}

fn stamp_order(stamp: [u8; 4]) -> Option<Endian> {
    match (stamp[0], stamp[1]) {
        (0x44, 0x44 | 0x41) => Some(Endian::Little),
        (0x11, 0x11) => Some(Endian::Big),
        _ => None,
    }
}

fn opposite(e: Endian) -> Endian {
    match e {
        Endian::Little => Endian::Big,
        Endian::Big => Endian::Little,
    }
}

fn plausible(b: &[u8], e: Endian) -> bool {
    let mode = e.i32(b, 12).map(Mode::from_code);
    let dims = [0usize, 4, 8].map(|o| e.i32(b, o).unwrap_or(-1));
    matches!(mode, Some(m) if !matches!(m, Mode::Unknown(_)))
        && dims.iter().all(|d| (1..=1 << 24).contains(d))
}

impl MrcHeader {
    /// Decode a header from at least 1024 bytes. Returns `None` for shorter input.
    pub fn parse(b: &[u8]) -> Option<MrcHeader> {
        if b.len() < HEADER_LEN {
            return None;
        }
        let stamp: [u8; 4] = b[212..216].try_into().ok()?;
        let (endian, stamp_problem) = match stamp_order(stamp) {
            Some(e) if plausible(b, e) => (e, None),
            Some(e) if plausible(b, opposite(e)) => (
                opposite(e),
                Some(format!(
                    "machine stamp says {} but the header only makes sense as {}",
                    byte_order_name(e),
                    byte_order_name(opposite(e))
                )),
            ),
            Some(e) => (e, None),
            None => {
                let e = if plausible(b, Endian::Little) || !plausible(b, Endian::Big) {
                    Endian::Little
                } else {
                    Endian::Big
                };
                (
                    e,
                    Some(format!(
                        "unrecognised machine stamp {:02x} {:02x} {:02x} {:02x}; header read as {}",
                        stamp[0],
                        stamp[1],
                        stamp[2],
                        stamp[3],
                        byte_order_name(e)
                    )),
                )
            }
        };
        let i = |o: usize| endian.i32(b, o).unwrap_or(0);
        let f = |o: usize| endian.f32(b, o).unwrap_or(0.0);
        let i16v = |o: usize| endian.i16(b, o).unwrap_or(0);
        let labels = (0..10)
            .map(|k| text(&b[224 + 80 * k..224 + 80 * (k + 1)]))
            .collect();
        Some(MrcHeader {
            nx: i(0),
            ny: i(4),
            nz: i(8),
            mode: Mode::from_code(i(12)),
            nxstart: i(16),
            nystart: i(20),
            nzstart: i(24),
            mx: i(28),
            my: i(32),
            mz: i(36),
            cell_a: [f(40), f(44), f(48)],
            cell_b: [f(52), f(56), f(60)],
            mapc: i(64),
            mapr: i(68),
            maps: i(72),
            dmin: f(76),
            dmax: f(80),
            dmean: f(84),
            ispg: i(88),
            nsymbt: i(92),
            exttyp: text(&b[104..108]),
            nversion: i(108),
            origin: [f(196), f(200), f(204)],
            has_map_id: &b[208..211] == b"MAP",
            machine_stamp: stamp,
            little_endian: endian == Endian::Little,
            stamp_problem,
            rms: f(216),
            nlabl: i(220),
            labels,
            imod_stamp: i(152) == IMOD_STAMP,
            imod_flags: i(156),
            nint: i16v(128),
            nreal: i16v(130),
        })
    }

    pub(crate) fn endian(&self) -> Endian {
        if self.little_endian {
            Endian::Little
        } else {
            Endian::Big
        }
    }

    /// Section organisation from ISPG (spec note 6) and, for ISPG 0 files named `.map`/`.rec`,
    /// the extension convention (those are volumes).
    pub fn layout(&self, extension: &str) -> Layout {
        if (401..=630).contains(&self.ispg) && self.mz > 0 && self.nz % self.mz == 0 {
            Layout::VolumeStack
        } else if self.ispg == 0 && !matches!(extension, "map" | "rec" | "ccp4") {
            Layout::ImageStack
        } else {
            Layout::Volume
        }
    }

    /// True when mode-0 bytes are unsigned: IMOD stamp present without the "signed" flag (bit 1).
    pub fn bytes_unsigned(&self) -> bool {
        self.mode == Mode::Int8 && self.imod_stamp && self.imod_flags & 1 == 0
    }

    /// Stored bytes of one section (NX × NY samples).
    pub fn section_bytes(&self) -> Option<u64> {
        let nx = u64::try_from(self.nx).ok()?;
        let ny = u64::try_from(self.ny).ok()?;
        let row = match self.mode {
            Mode::Packed4Bit => nx.div_ceil(2),
            Mode::Rgb8 => nx.checked_mul(3)?,
            m => nx.checked_mul(m.stored_bytes()?)?,
        };
        row.checked_mul(ny)
    }

    /// First byte of the data block.
    pub fn data_offset(&self) -> u64 {
        HEADER_LEN as u64 + u64::try_from(self.nsymbt.max(0)).unwrap_or(0)
    }

    /// MAPC/MAPR/MAPS as zero-based cell axes, when they are a permutation of 1, 2, 3.
    pub fn axis_map(&self) -> Option<[usize; 3]> {
        let m = [self.mapc, self.mapr, self.maps];
        let mut seen = [false; 3];
        let mut out = [0usize; 3];
        for (k, v) in m.iter().enumerate() {
            let a = usize::try_from(*v).ok()?.checked_sub(1)?;
            if a > 2 || seen[a] {
                return None;
            }
            seen[a] = true;
            out[k] = a;
        }
        Some(out)
    }

    /// Sampling of cell axis `a` (0 = X, 1 = Y, 2 = Z) in ångström: CELLA / (MX, MY, MZ).
    pub fn cell_step_angstrom(&self, a: usize) -> Option<f64> {
        let m = [self.mx, self.my, self.mz][a];
        let c = f64::from(self.cell_a[a]);
        (m > 0 && c.is_finite() && c > 0.0).then(|| c / f64::from(m))
    }

    /// Sampling in ångström along the file's columns, rows and sections (MAPC/MAPR/MAPS applied;
    /// identity when they are not a permutation).
    pub fn file_axis_steps_angstrom(&self) -> [Option<f64>; 3] {
        let map = self.axis_map().unwrap_or([0, 1, 2]);
        map.map(|a| self.cell_step_angstrom(a))
    }

    /// Density statistics are flagged undetermined (spec note 5).
    pub fn stats_undetermined(&self) -> (bool, bool, bool) {
        let range = self.dmax < self.dmin;
        let mean = self.dmean < self.dmin.min(self.dmax);
        let rms = self.rms < 0.0;
        (range, mean, rms)
    }
}

/// Header bytes that look like MRC: plausible dimensions and a known mode in some byte order.
pub fn looks_like_mrc(b: &[u8]) -> bool {
    b.len() >= HEADER_LEN && (plausible(b, Endian::Little) || plausible(b, Endian::Big))
}

/// Header with the `MAP` identifier at byte 209.
pub fn has_map_id(b: &[u8]) -> bool {
    b.get(208..211) == Some(b"MAP")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn header(nx: i32, ny: i32, nz: i32, mode: i32, big: bool) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_LEN];
        let put = |b: &mut Vec<u8>, o: usize, v: i32| {
            let bytes = if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            };
            b[o..o + 4].copy_from_slice(&bytes);
        };
        put(&mut b, 0, nx);
        put(&mut b, 4, ny);
        put(&mut b, 8, nz);
        put(&mut b, 12, mode);
        put(&mut b, 28, nx);
        put(&mut b, 32, ny);
        put(&mut b, 36, nz);
        put(&mut b, 64, 1);
        put(&mut b, 68, 2);
        put(&mut b, 72, 3);
        b[208..212].copy_from_slice(b"MAP ");
        b[212..216].copy_from_slice(if big {
            &[0x11, 0x11, 0, 0]
        } else {
            &[0x44, 0x44, 0, 0]
        });
        b
    }

    #[test]
    fn parses_both_byte_orders() {
        for big in [false, true] {
            let h = MrcHeader::parse(&header(10, 20, 3, 2, big)).unwrap();
            assert_eq!((h.nx, h.ny, h.nz, h.mode), (10, 20, 3, Mode::Float32));
            assert_eq!(h.little_endian, !big);
            assert!(h.stamp_problem.is_none());
            assert!(h.has_map_id);
            assert_eq!(h.axis_map(), Some([0, 1, 2]));
            assert_eq!(h.section_bytes(), Some(800));
        }
    }

    #[test]
    fn wrong_stamp_is_detected_and_corrected() {
        let mut b = header(10, 20, 3, 1, true);
        b[212..216].copy_from_slice(&[0x44, 0x44, 0, 0]);
        let h = MrcHeader::parse(&b).unwrap();
        assert!(!h.little_endian);
        assert!(h.stamp_problem.is_some());
        let mut b = header(10, 20, 3, 1, false);
        b[212..216].copy_from_slice(&[0, 0, 0, 0]);
        let h = MrcHeader::parse(&b).unwrap();
        assert!(h.little_endian);
        assert!(h.stamp_problem.unwrap().contains("unrecognised"));
    }

    #[test]
    fn packed_rows_are_padded() {
        let h = MrcHeader::parse(&header(5, 4, 1, 101, false)).unwrap();
        assert_eq!(h.mode, Mode::Packed4Bit);
        assert_eq!(h.section_bytes(), Some(12));
    }

    #[test]
    fn layouts() {
        let mut h = MrcHeader::parse(&header(4, 4, 6, 2, false)).unwrap();
        assert_eq!(h.layout("mrc"), Layout::ImageStack);
        assert_eq!(h.layout("map"), Layout::Volume);
        h.ispg = 1;
        assert_eq!(h.layout("mrcs"), Layout::Volume);
        h.ispg = 401;
        h.mz = 3;
        assert_eq!(h.layout("mrc"), Layout::VolumeStack);
        h.mz = 4;
        assert_eq!(h.layout("mrc"), Layout::Volume);
    }

    #[test]
    fn axis_permutation_steps() {
        let mut h = MrcHeader::parse(&header(4, 4, 4, 2, false)).unwrap();
        h.cell_a = [40.0, 12.0, 72.0];
        h.mx = 40;
        h.my = 12;
        h.mz = 36;
        h.mapc = 3;
        h.mapr = 1;
        h.maps = 2;
        assert_eq!(
            h.file_axis_steps_angstrom(),
            [Some(2.0), Some(1.0), Some(1.0)]
        );
        h.maps = 1;
        assert_eq!(h.axis_map(), None);
    }
}

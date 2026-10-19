//! DeltaVision header (1024 bytes, MRC-like, with the DeltaVision identifier at byte 96) and
//! extended header (integers and floats per section). Layout and vocabulary:
//! `docs/formats/deltavision.md`.

use openreadout_core::bytes::Endian;
use openreadout_core::pixel::PixelType;

/// Bytes of the main header.
pub const HEADER_LEN: usize = 1024;
/// The DeltaVision identifier at byte 96 (−16224 as a 16-bit integer).
pub const DV_ID: i16 = -16224;
/// Byte offset of the identifier.
pub const DV_ID_OFFSET: usize = 96;
/// Named floats at the start of each section's extended-header floats.
pub const SECTION_FLOATS: usize = 14;

/// Order of the sections in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionOrder {
    /// Z fastest, then time, then wavelength (`ZTW`, code 0).
    Ztw,
    /// Wavelength fastest, then Z, then time (`WZT`, code 1).
    Wzt,
    /// Z fastest, then wavelength, then time (`ZWT`, code 2).
    Zwt,
}

impl SectionOrder {
    /// The order of code `v`; `None` for other codes.
    pub fn from_code(v: i16) -> Option<SectionOrder> {
        match v {
            0 => Some(SectionOrder::Ztw),
            1 => Some(SectionOrder::Wzt),
            2 => Some(SectionOrder::Zwt),
            _ => None,
        }
    }
    /// Our name of the order.
    pub fn name(self) -> &'static str {
        match self {
            SectionOrder::Ztw => "ZTW",
            SectionOrder::Wzt => "WZT",
            SectionOrder::Zwt => "ZWT",
        }
    }
}

/// The parsed main header.
#[derive(Debug, Clone, PartialEq)]
pub struct DvHeader {
    /// Byte order of every value.
    pub endian: Endian,
    /// Columns.
    pub width: i32,
    /// Rows.
    pub height: i32,
    /// Sections (planes) in the file.
    pub sections: i32,
    /// Pixel mode.
    pub mode: i32,
    /// Start indices (x, y, z).
    pub start: [i32; 3],
    /// Sampling (mx, my, mz).
    pub sampling: [i32; 3],
    /// Pixel spacing in µm (x, y, z).
    pub spacing_um: [f32; 3],
    /// Minimum, maximum and mean of the first wavelength.
    pub intensity: [f32; 3],
    /// Bytes of the extended header.
    pub ext_len: i32,
    /// Integers per section in the extended header.
    pub ext_ints: i16,
    /// Floats per section in the extended header.
    pub ext_floats: i16,
    /// Image type code (0 normal, 1 tilt series, …).
    pub image_type: i16,
    /// Lens (objective) id.
    pub lens_id: i16,
    /// Time points.
    pub time_points: i16,
    /// Section order code.
    pub sequence: i16,
    /// Wavelengths (channels).
    pub wavelengths: i16,
    /// The five wavelength fields, nm (0 when unused).
    pub wavelength_nm: [i16; 5],
    /// Minimum and maximum of wavelengths 2–5.
    pub other_ranges: [[f32; 2]; 4],
    /// Origin (x, y, z), µm.
    pub origin_um: [f32; 3],
    /// Tilt angles (x, y, z).
    pub tilt: [f32; 3],
    /// Title lines (10 × 80 characters), blank lines dropped.
    pub titles: Vec<String>,
}

/// True when `head` carries the DeltaVision identifier at byte 96 in either byte order.
pub fn looks_like_dv(head: &[u8]) -> bool {
    dv_endian(head).is_some()
}

/// Byte order from the identifier.
pub fn dv_endian(head: &[u8]) -> Option<Endian> {
    match head.get(DV_ID_OFFSET..DV_ID_OFFSET + 2)? {
        [0xa0, 0xc0] => Some(Endian::Little),
        [0xc0, 0xa0] => Some(Endian::Big),
        _ => None,
    }
}

/// Bytes per sample of pixel mode `mode`, and its pixel type (`None` for unknown modes and
/// complex 16-bit integers).
pub fn mode_type(mode: i32) -> Option<PixelType> {
    Some(match mode {
        0 => PixelType::Uint8,
        1 | 5 => PixelType::Int16,
        2 => PixelType::Float,
        4 => PixelType::ComplexFloat,
        6 => PixelType::Uint16,
        7 => PixelType::Int32,
        _ => return None,
    })
}

impl DvHeader {
    /// Parse the first 1024 bytes; `None` without the identifier.
    pub fn parse(b: &[u8]) -> Option<DvHeader> {
        let e = dv_endian(b)?;
        if b.len() < HEADER_LEN {
            return None;
        }
        let i32_ = |o: usize| e.i32(b, o).unwrap_or(0);
        let i16_ = |o: usize| e.i16(b, o).unwrap_or(0);
        let f32_ = |o: usize| e.f32(b, o).unwrap_or(f32::NAN);
        let titles = b[224..HEADER_LEN]
            .chunks(80)
            .map(|t| {
                openreadout_core::bytes::latin1(openreadout_core::bytes::until_nul(t))
                    .trim()
                    .to_string()
            })
            .filter(|t| !t.is_empty())
            .collect();
        Some(DvHeader {
            endian: e,
            width: i32_(0),
            height: i32_(4),
            sections: i32_(8),
            mode: i32_(12),
            start: [i32_(16), i32_(20), i32_(24)],
            sampling: [i32_(28), i32_(32), i32_(36)],
            spacing_um: [f32_(40), f32_(44), f32_(48)],
            intensity: [f32_(76), f32_(80), f32_(84)],
            ext_len: i32_(92),
            ext_ints: i16_(128),
            ext_floats: i16_(130),
            image_type: i16_(160),
            lens_id: i16_(162),
            time_points: i16_(180),
            sequence: i16_(182),
            wavelengths: i16_(196),
            wavelength_nm: [i16_(198), i16_(200), i16_(202), i16_(204), i16_(206)],
            other_ranges: [
                [f32_(136), f32_(140)],
                [f32_(144), f32_(148)],
                [f32_(152), f32_(156)],
                [f32_(172), f32_(176)],
            ],
            origin_um: [f32_(212), f32_(216), f32_(208)],
            tilt: [f32_(184), f32_(188), f32_(192)],
            titles,
        })
    }

    /// Wavelengths as a channel count (at least 1).
    pub fn channel_count(&self) -> u32 {
        u32::try_from(self.wavelengths).unwrap_or(0).max(1)
    }

    /// Time points (at least 1).
    pub fn time_count(&self) -> u32 {
        u32::try_from(self.time_points).unwrap_or(0).max(1)
    }

    /// Z planes: sections ÷ (wavelengths × time points), when that divides.
    pub fn z_count(&self) -> Option<u32> {
        let s = u32::try_from(self.sections).ok()?;
        let per = self.channel_count().checked_mul(self.time_count())?;
        (per > 0 && s % per == 0).then_some(s / per)
    }

    /// Index of the section holding plane (c, z, t).
    pub fn section_of(&self, c: u32, z: u32, t: u32) -> Option<u64> {
        let (nc, nz, nt) = (
            u64::from(self.channel_count()),
            u64::from(self.z_count()?),
            u64::from(self.time_count()),
        );
        let (c, z, t) = (u64::from(c), u64::from(z), u64::from(t));
        if c >= nc || z >= nz || t >= nt {
            return None;
        }
        Some(match SectionOrder::from_code(self.sequence)? {
            SectionOrder::Ztw => z + nz * (t + nt * c),
            SectionOrder::Wzt => c + nc * (z + nz * t),
            SectionOrder::Zwt => z + nz * (c + nc * t),
        })
    }
}

/// The named floats of one section of the extended header.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SectionInfo {
    /// Photosensor reading.
    pub photosensor: f32,
    /// Time stamp, s from the start.
    pub time_s: f32,
    /// Stage position x, y, z (µm).
    pub stage_um: [f32; 3],
    /// Minimum, maximum and mean intensity of the section.
    pub intensity: [f32; 3],
    /// Exposure time, s.
    pub exposure_s: f32,
    /// Neutral-density filter.
    pub nd_filter: f32,
    /// Excitation wavelength, nm.
    pub excitation_nm: f32,
    /// Emission wavelength, nm.
    pub emission_nm: f32,
    /// Intensity scaling.
    pub intensity_scaling: f32,
    /// Energy conversion factor.
    pub energy_conversion: f32,
}

/// The extended-header floats of `section`, when the extended header holds them.
pub fn section_info(ext: &[u8], h: &DvHeader, section: u64) -> Option<SectionInfo> {
    let ints = usize::try_from(h.ext_ints).ok()?;
    let floats = usize::try_from(h.ext_floats).ok()?;
    if floats < SECTION_FLOATS {
        return None;
    }
    let stride = (ints + floats).checked_mul(4)?;
    let at = usize::try_from(section)
        .ok()?
        .checked_mul(stride)?
        .checked_add(ints * 4)?;
    let f = |k: usize| h.endian.f32(ext, at + 4 * k);
    Some(SectionInfo {
        photosensor: f(0)?,
        time_s: f(1)?,
        stage_um: [f(2)?, f(3)?, f(4)?],
        intensity: [f(5)?, f(6)?, f(7)?],
        exposure_s: f(8)?,
        nd_filter: f(9)?,
        excitation_nm: f(10)?,
        emission_nm: f(11)?,
        intensity_scaling: f(12)?,
        energy_conversion: f(13)?,
    })
}

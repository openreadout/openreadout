//! The NIfTI-1 (348 bytes), NIfTI-2 (540 bytes) and Analyze 7.5 (348 bytes) headers, as the
//! NIfTI Data Format Working Group's public `nifti1.h` and `nifti2.h` lay them out, and the
//! Mayo Clinic Analyze 7.5 header they extend.

/// Which header a file has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// NIfTI-1 single file (`n+1`) or pair (`ni1`).
    Nifti1,
    /// NIfTI-2 single file (`n+2`) or pair (`ni2`).
    Nifti2,
    /// Analyze 7.5 (no magic; always a `.hdr`/`.img` pair).
    Analyze,
}

/// The fields we read, NIfTI-2 widths for both versions.
#[derive(Debug, Clone)]
pub(crate) struct Header {
    pub(crate) kind: Kind,
    pub(crate) little: bool,
    /// Single file (`n+1` / `n+2`): data in the same file at `vox_offset`.
    pub(crate) single: bool,
    pub(crate) dim: [i64; 8],
    pub(crate) datatype: i16,
    pub(crate) bitpix: i16,
    pub(crate) pixdim: [f64; 8],
    pub(crate) vox_offset: f64,
    pub(crate) scl_slope: f64,
    pub(crate) scl_inter: f64,
    pub(crate) cal_max: f64,
    pub(crate) cal_min: f64,
    pub(crate) slice_duration: f64,
    pub(crate) toffset: f64,
    pub(crate) slice_start: i64,
    pub(crate) slice_end: i64,
    pub(crate) slice_code: i32,
    pub(crate) xyzt_units: i32,
    pub(crate) intent_code: i32,
    pub(crate) intent_p: [f64; 3],
    pub(crate) intent_name: String,
    pub(crate) dim_info: u8,
    pub(crate) descrip: String,
    pub(crate) aux_file: String,
    pub(crate) qform_code: i32,
    pub(crate) sform_code: i32,
    /// quatern_b, c, d, qoffset_x, y, z.
    pub(crate) quatern: [f64; 6],
    pub(crate) srow: [[f64; 4]; 3],
    pub(crate) magic: String,
    pub(crate) header_len: usize,
    /// Analyze 7.5 only: `patient_id` holds text (it is withheld).
    pub(crate) analyze_patient_id: bool,
    /// Analyze 7.5 / SPM: the scale factor in `funused1` (SPM writes it there).
    pub(crate) spm_scale: f64,
    /// Analyze 7.5: `orient` byte.
    pub(crate) orient: u8,
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim().to_string()
}

struct R<'a> {
    b: &'a [u8],
    little: bool,
}

impl R<'_> {
    fn i16(&self, o: usize) -> i16 {
        let a = [self.b[o], self.b[o + 1]];
        if self.little {
            i16::from_le_bytes(a)
        } else {
            i16::from_be_bytes(a)
        }
    }
    fn i32(&self, o: usize) -> i32 {
        let a: [u8; 4] = self.b[o..o + 4].try_into().unwrap_or([0; 4]);
        if self.little {
            i32::from_le_bytes(a)
        } else {
            i32::from_be_bytes(a)
        }
    }
    fn i64(&self, o: usize) -> i64 {
        let a: [u8; 8] = self.b[o..o + 8].try_into().unwrap_or([0; 8]);
        if self.little {
            i64::from_le_bytes(a)
        } else {
            i64::from_be_bytes(a)
        }
    }
    fn f32(&self, o: usize) -> f64 {
        let a: [u8; 4] = self.b[o..o + 4].try_into().unwrap_or([0; 4]);
        f64::from(if self.little {
            f32::from_le_bytes(a)
        } else {
            f32::from_be_bytes(a)
        })
    }
    fn f64(&self, o: usize) -> f64 {
        let a: [u8; 8] = self.b[o..o + 8].try_into().unwrap_or([0; 8]);
        if self.little {
            f64::from_le_bytes(a)
        } else {
            f64::from_be_bytes(a)
        }
    }
}

/// The header kind and byte order from `sizeof_hdr` (348 or 540, either byte order), when
/// `head` starts like one.
pub(crate) fn probe(head: &[u8]) -> Option<(Kind, bool)> {
    if head.len() < 4 {
        return None;
    }
    let le = i32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let be = i32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    let (len, little) = match (le, be) {
        (348 | 540, _) => (le, true),
        (_, 348 | 540) => (be, false),
        _ => return None,
    };
    if len == 540 {
        let m = head.get(4..8)?;
        return (m == b"n+2\0" || m == b"ni2\0").then_some((Kind::Nifti2, little));
    }
    let m = head.get(344..348);
    match m {
        Some(b"n+1\0" | b"ni1\0") => Some((Kind::Nifti1, little)),
        Some(_) if head.len() >= 348 => {
            // Analyze 7.5: dim[0] between 1 and 7 is the only check the format allows
            let r = R { b: head, little };
            let n = r.i16(40);
            (1..=7).contains(&n).then_some((Kind::Analyze, little))
        }
        _ => None,
    }
}

/// Parse a header (at least 348 or 540 bytes).
pub(crate) fn parse(b: &[u8]) -> Option<Header> {
    let (kind, little) = probe(b)?;
    let r = R { b, little };
    if kind == Kind::Nifti2 {
        if b.len() < 540 {
            return None;
        }
        let mut dim = [0i64; 8];
        let mut pixdim = [0f64; 8];
        for i in 0..8 {
            dim[i] = r.i64(16 + 8 * i);
            pixdim[i] = r.f64(104 + 8 * i);
        }
        let mut srow = [[0f64; 4]; 3];
        for (k, row) in srow.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = r.f64(400 + 32 * k + 8 * j);
            }
        }
        let magic = text(&b[4..8]);
        return Some(Header {
            kind,
            little,
            single: magic == "n+2",
            dim,
            datatype: r.i16(12),
            bitpix: r.i16(14),
            pixdim,
            vox_offset: r.i64(168) as f64,
            scl_slope: r.f64(176),
            scl_inter: r.f64(184),
            cal_max: r.f64(192),
            cal_min: r.f64(200),
            slice_duration: r.f64(208),
            toffset: r.f64(216),
            slice_start: r.i64(224),
            slice_end: r.i64(232),
            slice_code: r.i32(496),
            xyzt_units: r.i32(500),
            intent_code: r.i32(504),
            intent_p: [r.f64(80), r.f64(88), r.f64(96)],
            intent_name: text(&b[508..524]),
            dim_info: b[524],
            descrip: text(&b[240..320]),
            aux_file: text(&b[320..344]),
            qform_code: r.i32(344),
            sform_code: r.i32(348),
            quatern: [
                r.f64(352),
                r.f64(360),
                r.f64(368),
                r.f64(376),
                r.f64(384),
                r.f64(392),
            ],
            srow,
            magic,
            header_len: 540,
            analyze_patient_id: false,
            spm_scale: 0.0,
            orient: 0,
        });
    }
    if b.len() < 348 {
        return None;
    }
    let mut dim = [0i64; 8];
    let mut pixdim = [0f64; 8];
    for i in 0..8 {
        dim[i] = i64::from(r.i16(40 + 2 * i));
        pixdim[i] = r.f32(76 + 4 * i);
    }
    let analyze = kind == Kind::Analyze;
    let mut srow = [[0f64; 4]; 3];
    if !analyze {
        for (k, row) in srow.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = r.f32(280 + 16 * k + 4 * j);
            }
        }
    }
    let magic = if analyze {
        String::new()
    } else {
        text(&b[344..348])
    };
    Some(Header {
        kind,
        little,
        single: magic == "n+1",
        dim,
        datatype: r.i16(70),
        bitpix: r.i16(72),
        pixdim,
        vox_offset: r.f32(108),
        scl_slope: if analyze { 0.0 } else { r.f32(112) },
        scl_inter: if analyze { 0.0 } else { r.f32(116) },
        cal_max: r.f32(124),
        cal_min: r.f32(128),
        slice_duration: if analyze { 0.0 } else { r.f32(132) },
        toffset: if analyze { 0.0 } else { r.f32(136) },
        slice_start: if analyze { 0 } else { i64::from(r.i16(74)) },
        slice_end: if analyze { 0 } else { i64::from(r.i16(120)) },
        slice_code: if analyze { 0 } else { i32::from(b[122]) },
        xyzt_units: if analyze { 0 } else { i32::from(b[123]) },
        intent_code: if analyze { 0 } else { i32::from(r.i16(68)) },
        intent_p: if analyze {
            [0.0; 3]
        } else {
            [r.f32(56), r.f32(60), r.f32(64)]
        },
        intent_name: if analyze {
            String::new()
        } else {
            text(&b[328..344])
        },
        dim_info: if analyze { 0 } else { b[39] },
        descrip: text(&b[148..228]),
        aux_file: text(&b[228..252]),
        qform_code: if analyze { 0 } else { i32::from(r.i16(252)) },
        sform_code: if analyze { 0 } else { i32::from(r.i16(254)) },
        quatern: if analyze {
            [0.0; 6]
        } else {
            [
                r.f32(256),
                r.f32(260),
                r.f32(264),
                r.f32(268),
                r.f32(272),
                r.f32(276),
            ]
        },
        srow,
        magic,
        header_len: 348,
        analyze_patient_id: analyze && b[283..293].iter().any(|c| !matches!(c, 0 | b' ')),
        spm_scale: if analyze { r.f32(112) } else { 0.0 },
        orient: if analyze { b[252] } else { 0 },
    })
}

/// Name, sample bytes and our pixel type of a NIfTI datatype code.
pub(crate) fn datatype(
    code: i16,
) -> Option<(&'static str, usize, openreadout_core::PixelType, u32)> {
    use openreadout_core::PixelType as P;
    Some(match code {
        2 => ("uint8", 1, P::Uint8, 1),
        4 => ("int16", 2, P::Int16, 1),
        8 => ("int32", 4, P::Int32, 1),
        16 => ("float32", 4, P::Float, 1),
        32 => ("complex64", 8, P::ComplexFloat, 1),
        64 => ("float64", 8, P::Double, 1),
        128 => ("rgb24", 3, P::Uint8, 3),
        256 => ("int8", 1, P::Int8, 1),
        512 => ("uint16", 2, P::Uint16, 1),
        768 => ("uint32", 4, P::Uint32, 1),
        1024 => ("int64", 8, P::Int64, 1),
        1280 => ("uint64", 8, P::Uint64, 1),
        1792 => ("complex128", 16, P::ComplexDouble, 1),
        2304 => ("rgba32", 4, P::Uint8, 4),
        _ => return None,
    })
}

/// The name of datatype codes this reader does not decode.
pub(crate) fn unsupported_datatype(code: i16) -> Option<&'static str> {
    match code {
        1 => Some("binary (1 bit)"),
        1536 => Some("float128"),
        2048 => Some("complex256"),
        _ => None,
    }
}

impl Header {
    /// Spatial unit to micrometres and temporal unit to seconds (xyzt_units; NIfTI-1 codes
    /// 1 m, 2 mm, 3 µm; 8 s, 16 ms, 24 µs). Unknown spatial units are taken as mm (the
    /// Analyze convention).
    pub(crate) fn units(&self) -> (f64, Option<f64>, bool) {
        let space = self.xyzt_units & 0x07;
        let time = self.xyzt_units & 0x38;
        let (um, assumed) = match space {
            1 => (1e6, false),
            2 => (1e3, false),
            3 => (1.0, false),
            _ => (1e3, true),
        };
        let s = match time {
            8 => Some(1.0),
            16 => Some(1e-3),
            24 => Some(1e-6),
            _ => None,
        };
        (um, s, assumed)
    }

    /// The voxel-to-world affine (RAS+, mm): the sform when its code is set, else the qform
    /// (quaternion), else pixdim scaling (method 1 of `nifti1.h`).
    #[allow(clippy::many_single_char_names)]
    pub(crate) fn affine(&self) -> ([[f64; 4]; 3], &'static str) {
        if self.sform_code > 0 {
            return (self.srow, "sform");
        }
        let (dx, dy, dz) = (self.pixdim[1], self.pixdim[2], self.pixdim[3]);
        if self.qform_code > 0 {
            let [b, c, d, qx, qy, qz] = self.quatern;
            let a = (1.0 - (b * b + c * c + d * d)).max(0.0).sqrt();
            let qfac = if self.pixdim[0] < 0.0 { -1.0 } else { 1.0 };
            let r = [
                [
                    a * a + b * b - c * c - d * d,
                    2.0 * (b * c - a * d),
                    2.0 * (b * d + a * c),
                ],
                [
                    2.0 * (b * c + a * d),
                    a * a + c * c - b * b - d * d,
                    2.0 * (c * d - a * b),
                ],
                [
                    2.0 * (b * d - a * c),
                    2.0 * (c * d + a * b),
                    a * a + d * d - c * c - b * b,
                ],
            ];
            let s = [dx, dy, dz * qfac];
            let mut m = [[0.0; 4]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] = r[i][j] * s[j];
                }
            }
            m[0][3] = qx;
            m[1][3] = qy;
            m[2][3] = qz;
            return (m, "qform");
        }
        (
            [
                [dx, 0.0, 0.0, 0.0],
                [0.0, dy, 0.0, 0.0],
                [0.0, 0.0, dz, 0.0],
            ],
            "pixdim",
        )
    }
}

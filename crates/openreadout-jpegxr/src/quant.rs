//! Quantizers (T.832 clause 9.4; jxrlib `strPredQuant.c` `remapQP`, `strcodec.c`
//! `formatQuantizer`).

const SHIFTZERO: i32 = 1;
const QPFRACBITS: i32 = 2;

/// One quantizer: the coded index and the step it maps to.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Quantizer {
    pub(crate) index: u8,
    pub(crate) qp: i32,
}

/// jxrlib `remapQP`.
fn remap(q: &mut Quantizer, shift: i32, scaled: bool) {
    let idx = i32::from(q.index);
    if idx == 0 {
        q.qp = 1;
        return;
    }
    let (man, exp) = if !scaled {
        let ci_shift = SHIFTZERO - (SHIFTZERO + QPFRACBITS);
        if idx < 32 {
            ((idx + 3) >> 2, ci_shift + 2)
        } else if idx < 48 {
            ((16 + (idx & 0xf) + 1) >> 1, ((idx >> 4) - 1) + 1 + ci_shift)
        } else {
            (16 + (idx & 0xf), ((idx >> 4) - 1) + ci_shift)
        }
    } else if idx < 16 {
        (idx, shift)
    } else {
        (16 + (idx & 0xf), ((idx >> 4) - 1) + shift)
    };
    q.qp = man.wrapping_shl(exp as u32);
}

/// Quantizers of one band for all channels: `q[channel][qp_index]`.
pub(crate) type BandQuant = Vec<Vec<Quantizer>>;

pub(crate) fn alloc(channels: usize, count: usize) -> BandQuant {
    vec![vec![Quantizer::default(); count]; channels]
}

/// jxrlib `formatQuantizer`: spread the channel mode and derive the steps.
pub(crate) fn format(q: &mut BandQuant, mode: u32, pos: usize, shifted_uv: bool, scaled: bool) {
    for ch in 0..q.len() {
        if ch > 0 {
            if mode == 0 {
                q[ch][pos] = q[0][pos];
            } else if mode == 1 {
                q[ch][pos] = q[1][pos];
            }
        }
        let shift = if ch > 0 && shifted_uv {
            SHIFTZERO - 1
        } else {
            SHIFTZERO
        };
        remap(&mut q[ch][pos], shift, scaled);
    }
}

/// Number of bits of a macroblock's QP index for `count` quantizers (jxrlib `dquantBits`).
pub(crate) fn dquant_bits(count: usize) -> u32 {
    match count {
        0 | 1 => 0,
        2 | 3 => 1,
        4 | 5 => 2,
        6..=9 => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_matches_the_reference_tables() {
        let qp = |index, shift, scaled| {
            let mut q = Quantizer { index, qp: 0 };
            remap(&mut q, shift, scaled);
            q.qp
        };
        assert_eq!(qp(0, 1, true), 1);
        assert_eq!(qp(1, 1, false), 1);
        assert_eq!(qp(8, 1, false), 2);
        assert_eq!(qp(40, 1, false), 12);
        assert_eq!(qp(80, 1, false), 64);
        assert_eq!(qp(10, 1, true), 20);
        assert_eq!(qp(10, 0, true), 10);
        assert_eq!(qp(255, 1, true), 31 << 15);
    }
}

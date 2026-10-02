//! Two's-complement `i32` arithmetic, as jxrlib's C code gets on every platform it runs on.
//!
//! Valid codestreams never overflow; malformed ones can, and must neither panic (debug builds,
//! fuzzing) nor differ from wrapping semantics. Shifts mask their count like the hardware does.

use std::ops::{Add, AddAssign, BitAnd, BitOr, BitXor, Mul, Neg, Shl, Shr, Sub, SubAssign};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) struct W(pub(crate) i32);

impl Add for W {
    type Output = W;
    #[inline]
    fn add(self, o: W) -> W {
        W(self.0.wrapping_add(o.0))
    }
}
impl Add<i32> for W {
    type Output = W;
    #[inline]
    fn add(self, o: i32) -> W {
        W(self.0.wrapping_add(o))
    }
}
impl Sub for W {
    type Output = W;
    #[inline]
    fn sub(self, o: W) -> W {
        W(self.0.wrapping_sub(o.0))
    }
}
impl Sub<i32> for W {
    type Output = W;
    #[inline]
    fn sub(self, o: i32) -> W {
        W(self.0.wrapping_sub(o))
    }
}
impl Mul<i32> for W {
    type Output = W;
    #[inline]
    fn mul(self, o: i32) -> W {
        W(self.0.wrapping_mul(o))
    }
}
impl Mul for W {
    type Output = W;
    #[inline]
    fn mul(self, o: W) -> W {
        W(self.0.wrapping_mul(o.0))
    }
}
impl Shr<i32> for W {
    type Output = W;
    #[inline]
    fn shr(self, n: i32) -> W {
        W(self.0.wrapping_shr(n as u32))
    }
}
impl Shl<i32> for W {
    type Output = W;
    #[inline]
    fn shl(self, n: i32) -> W {
        W(self.0.wrapping_shl(n as u32))
    }
}
impl Neg for W {
    type Output = W;
    #[inline]
    fn neg(self) -> W {
        W(self.0.wrapping_neg())
    }
}
impl BitAnd<i32> for W {
    type Output = W;
    #[inline]
    fn bitand(self, o: i32) -> W {
        W(self.0 & o)
    }
}
impl BitOr for W {
    type Output = W;
    #[inline]
    fn bitor(self, o: W) -> W {
        W(self.0 | o.0)
    }
}
impl BitXor for W {
    type Output = W;
    #[inline]
    fn bitxor(self, o: W) -> W {
        W(self.0 ^ o.0)
    }
}
impl AddAssign for W {
    #[inline]
    fn add_assign(&mut self, o: W) {
        self.0 = self.0.wrapping_add(o.0);
    }
}
impl AddAssign<i32> for W {
    #[inline]
    fn add_assign(&mut self, o: i32) {
        self.0 = self.0.wrapping_add(o);
    }
}
impl SubAssign for W {
    #[inline]
    fn sub_assign(&mut self, o: W) {
        self.0 = self.0.wrapping_sub(o.0);
    }
}
impl SubAssign<i32> for W {
    #[inline]
    fn sub_assign(&mut self, o: i32) {
        self.0 = self.0.wrapping_sub(o);
    }
}

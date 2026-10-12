//! The optional data-set CRC (FCS 3.1 §3.5): 16-bit CCITT polynomial (x^16 + x^12 + x^5 + 1).
//! The standard describes each input byte bit-reversed with initial value 0 (the bit-reflected
//! form, least-significant bit first). Every writer seen that stores a CRC (CytoFLEX, FACSVerse
//! and others) computes it most-significant bit first from 0x1D0F instead ("AUG-CCITT"), so both
//! are computed. Stored as ASCII in the 8 bytes after the last segment; `00000000` means the
//! writer did not compute one.

/// Incremental CRC state, both forms at once.
#[derive(Debug, Clone, Copy)]
pub struct Crc16 {
    value: u16,
    aug: u16,
}

impl Default for Crc16 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc16 {
    pub fn new() -> Self {
        Crc16 {
            value: 0,
            aug: 0x1D0F,
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.value;
        let mut aug = self.aug;
        for &b in bytes {
            crc ^= u16::from(b);
            aug ^= u16::from(b) << 8;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x8408
                } else {
                    crc >> 1
                };
                aug = if aug & 0x8000 != 0 {
                    (aug << 1) ^ 0x1021
                } else {
                    aug << 1
                };
            }
        }
        self.value = crc;
        self.aug = aug;
    }

    /// The bit-reflected form the standard describes.
    pub fn finish(self) -> u16 {
        self.value
    }

    /// The form writers store: most-significant bit first from 0x1D0F.
    pub fn finish_aug(self) -> u16 {
        self.aug
    }
}

/// What the 8 bytes after the last segment hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrcField {
    /// Fewer than 8 bytes remain in the file.
    Absent,
    /// `00000000` (or all zeros after trimming): the writer did not compute a CRC.
    NotComputed,
    /// ASCII digits: the stored value (read as decimal).
    Value(u32),
    /// Anything else (NUL bytes, spaces, the next data set's HEADER, ...).
    Other,
}

/// Interpret the 8 bytes after the last segment.
pub fn read_crc_field(bytes: &[u8]) -> CrcField {
    if bytes.len() < 8 {
        return CrcField::Absent;
    }
    let s = String::from_utf8_lossy(&bytes[..8]);
    let t = s.trim();
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) {
        return CrcField::Other;
    }
    match t.parse::<u32>() {
        Ok(0) => CrcField::NotComputed,
        Ok(v) => CrcField::Value(v),
        Err(_) => CrcField::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflected_ccitt_check_value() {
        let mut c = Crc16::new();
        c.update(b"123456789");
        assert_eq!(c.finish(), 0x2189);
        assert_eq!(c.finish_aug(), 0xE5CC);
    }

    #[test]
    fn incremental_equals_one_shot() {
        let mut a = Crc16::new();
        a.update(b"FCS3.1    ");
        a.update(b"rest of data set");
        let mut b = Crc16::new();
        b.update(b"FCS3.1    rest of data set");
        assert_eq!(a.finish(), b.finish());
        assert_eq!(a.finish_aug(), b.finish_aug());
    }

    #[test]
    fn fields() {
        assert_eq!(read_crc_field(b"00000000"), CrcField::NotComputed);
        assert_eq!(read_crc_field(b"0000000"), CrcField::Absent);
        assert_eq!(read_crc_field(b"   12345"), CrcField::Value(12345));
        assert_eq!(read_crc_field(b"\0\0\0\0\0\0\0\0"), CrcField::Other);
        assert_eq!(read_crc_field(b"FCS3.0  "), CrcField::Other);
    }
}

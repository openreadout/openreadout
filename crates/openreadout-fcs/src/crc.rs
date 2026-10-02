//! The optional data-set CRC (FCS 3.1 §3.5): 16-bit CCITT polynomial (x^16 + x^12 + x^5 + 1),
//! each input byte bit-reversed, initial value 0 — i.e. the bit-reflected form processed
//! least-significant bit first. Stored as ASCII in the 8 bytes after the last segment;
//! `00000000` means the writer did not compute one.

/// Incremental CRC state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Crc16 {
    value: u16,
}

impl Crc16 {
    pub fn new() -> Self {
        Crc16 { value: 0 }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.value;
        for &b in bytes {
            crc ^= u16::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x8408
                } else {
                    crc >> 1
                };
            }
        }
        self.value = crc;
    }

    pub fn finish(self) -> u16 {
        self.value
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
    }

    #[test]
    fn incremental_equals_one_shot() {
        let mut a = Crc16::new();
        a.update(b"FCS3.1    ");
        a.update(b"rest of data set");
        let mut b = Crc16::new();
        b.update(b"FCS3.1    rest of data set");
        assert_eq!(a.finish(), b.finish());
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

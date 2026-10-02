//! The two records of the .NET Remoting Binary Format ([MS-NRBF], a public Microsoft Open
//! Specification) that UNICORN 6/7 exports use: a string (the XML of a `…Data` member) and an
//! array of 32-bit floats (the volumes and amplitudes of a curve). Each stream is a
//! serialization header record, one record and a message-end record.

use openreadout_core::bytes::le_i32;

/// Record type of the serialization header (it is 17 bytes long).
const HEADER: u8 = 0;
/// Record type of a string object.
const OBJECT_STRING: u8 = 6;
/// Record type of the message end.
const MESSAGE_END: u8 = 11;
/// Record type of an array of primitives.
const ARRAY_OF_PRIMITIVE: u8 = 15;
/// Primitive type code of a 32-bit float.
const SINGLE: u8 = 11;
/// Bytes of the header record.
const HEADER_LEN: usize = 17;

/// Why a stream could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NrbfError(pub(crate) String);

fn header(b: &[u8]) -> Result<(), NrbfError> {
    if b.len() < HEADER_LEN || b[0] != HEADER {
        return Err(NrbfError(
            "no serialization header (the stream does not start with record type 0)".into(),
        ));
    }
    Ok(())
}

/// The number of floats a stream holding one float array declares, and where they start
/// (without reading them): `(count, offset of the first float)`.
pub(crate) fn single_array_shape(b: &[u8]) -> Result<(usize, usize), NrbfError> {
    header(b)?;
    let rec = b.get(HEADER_LEN).copied();
    if rec != Some(ARRAY_OF_PRIMITIVE) {
        return Err(NrbfError(format!(
            "record {rec:?} after the header, expected an array of primitives (15)"
        )));
    }
    let n = le_i32(b, HEADER_LEN + 5).ok_or_else(|| NrbfError("array header truncated".into()))?;
    let kind = b.get(HEADER_LEN + 9).copied();
    if kind != Some(SINGLE) {
        return Err(NrbfError(format!(
            "array of primitive type {kind:?}, expected 32-bit floats (11)"
        )));
    }
    let n = usize::try_from(n).map_err(|_| NrbfError(format!("negative array length {n}")))?;
    let start = HEADER_LEN + 10;
    let end = n
        .checked_mul(4)
        .and_then(|x| x.checked_add(start))
        .ok_or_else(|| NrbfError("array length overflows".into()))?;
    if end > b.len() {
        return Err(NrbfError(format!(
            "array of {n} floats runs past the end of the {}-byte stream",
            b.len()
        )));
    }
    if b.get(end).copied() != Some(MESSAGE_END) {
        return Err(NrbfError(
            "no message-end record after the array (the stream is truncated or holds more)".into(),
        ));
    }
    Ok((n, start))
}

/// The floats of a stream holding one float array, widened to f64.
pub(crate) fn single_array(b: &[u8]) -> Result<Vec<f64>, NrbfError> {
    let (n, start) = single_array_shape(b)?;
    let data = &b[start..start + n * 4];
    Ok(data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f64::from(f32::from_le_bytes(*c)))
        .collect())
}

/// The text of a stream holding one string object.
pub(crate) fn string(b: &[u8]) -> Result<String, NrbfError> {
    header(b)?;
    let rec = b.get(HEADER_LEN).copied();
    if rec == Some(MESSAGE_END) {
        return Ok(String::new());
    }
    if rec != Some(OBJECT_STRING) {
        return Err(NrbfError(format!(
            "record {rec:?} after the header, expected a string object (6)"
        )));
    }
    // object id (4 bytes), then the length as 7-bit groups, low first (at most 5 bytes)
    let mut at = HEADER_LEN + 5;
    let mut len: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *b
            .get(at)
            .ok_or_else(|| NrbfError("string length truncated".into()))?;
        at += 1;
        len |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return Err(NrbfError("string length longer than 5 bytes".into()));
        }
    }
    let len = usize::try_from(len).map_err(|_| NrbfError("string too long".into()))?;
    let end = at
        .checked_add(len)
        .filter(|e| *e <= b.len())
        .ok_or_else(|| {
            NrbfError(format!(
                "a {len}-byte string runs past the end of the {}-byte stream",
                b.len()
            ))
        })?;
    Ok(String::from_utf8_lossy(&b[at..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> Vec<u8> {
        let mut v = vec![
            0u8, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(v.len(), HEADER_LEN);
        v.shrink_to_fit();
        v
    }

    #[test]
    fn floats_round_trip() {
        let mut b = head();
        b.push(ARRAY_OF_PRIMITIVE);
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&3i32.to_le_bytes());
        b.push(SINGLE);
        for v in [1.5f32, -2.0, 0.25] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(MESSAGE_END);
        assert_eq!(single_array(&b).unwrap(), vec![1.5, -2.0, 0.25]);
        assert_eq!(single_array_shape(&b).unwrap(), (3, 27));
        // truncated: the count claims more than the stream holds
        let short = &b[..b.len() - 5];
        assert!(single_array(short).is_err());
        // not a float array
        let mut other = b.clone();
        other[26] = 6;
        assert!(single_array(&other).is_err());
    }

    #[test]
    fn strings_with_long_lengths() {
        let text = "x".repeat(300);
        let mut b = head();
        b.push(OBJECT_STRING);
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&[0xac, 0x02]); // 300 = 0b10_0101100
        b.extend_from_slice(text.as_bytes());
        b.push(MESSAGE_END);
        assert_eq!(string(&b).unwrap(), text);
        // an empty member: header then message end
        let mut e = head();
        e.push(MESSAGE_END);
        assert_eq!(string(&e).unwrap(), "");
        // runs past the end
        assert!(string(&b[..40]).is_err());
        // garbage
        assert!(string(&[1, 2, 3]).is_err());
        assert!(string(&[0u8; 17][..]).is_err());
    }
}

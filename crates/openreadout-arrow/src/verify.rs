//! Column digests: what was written is hashed batch by batch, the file is read back through the
//! Parquet or Arrow IPC reader, hashed the same way, and the two must agree column by column.

use std::fs::File;
use std::path::Path;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type, UInt8Type, UInt16Type,
    UInt32Type, UInt64Type,
};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, SchemaRef};
use openreadout_core::{Error, Result};
use rayon::prelude::*;
use xxhash_rust::xxh3::Xxh3;

use crate::ColumnarFormat;

/// One xxh3 digest per column, plus the row count.
#[derive(Default)]
pub(crate) struct Digest {
    columns: Vec<Xxh3>,
    pub rows: u64,
}

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Digest")
            .field("columns", &self.columns.len())
            .field("rows", &self.rows)
            .finish()
    }
}

fn float(h: &mut Vec<u8>, v: f64) {
    h.push(1);
    let bits = if v.is_nan() {
        f64::NAN.to_bits()
    } else {
        v.to_bits()
    };
    h.extend_from_slice(&bits.to_le_bytes());
}

macro_rules! prim {
    ($h:expr, $a:expr, $t:ty, |$v:ident| $body:expr) => {{
        let a = $a.as_primitive::<$t>();
        for i in 0..a.len() {
            if a.is_null(i) {
                $h.push(0);
            } else {
                let $v = a.value(i);
                $body;
            }
        }
    }};
}

/// Hash the logical values of `a` (dictionary arrays by their resolved strings, so a reader that
/// re-encodes the dictionary still matches).
///
/// The bytes are collected first and hashed with one call: the digest is the same, and one
/// call per value was most of the time of a Parquet export.
fn hash_array(hasher: &mut Xxh3, a: &dyn Array) -> Result<()> {
    let mut buf = Vec::with_capacity(a.len().saturating_mul(9));
    hash_values(&mut buf, a)?;
    hasher.update(&buf);
    Ok(())
}

#[allow(clippy::many_single_char_names)]
fn hash_values(h: &mut Vec<u8>, a: &dyn Array) -> Result<()> {
    match a.data_type() {
        DataType::Float64 => prim!(h, a, Float64Type, |v| float(h, v)),
        DataType::Float32 => prim!(h, a, Float32Type, |v| float(h, f64::from(v))),
        DataType::Int8 => prim!(h, a, Int8Type, |v| float(h, f64::from(v))),
        DataType::Int16 => prim!(h, a, Int16Type, |v| float(h, f64::from(v))),
        DataType::Int32 => prim!(h, a, Int32Type, |v| float(h, f64::from(v))),
        DataType::Int64 => prim!(h, a, Int64Type, |v| {
            h.push(2);
            h.extend_from_slice(&v.to_le_bytes());
        }),
        DataType::UInt8 => prim!(h, a, UInt8Type, |v| float(h, f64::from(v))),
        DataType::UInt16 => prim!(h, a, UInt16Type, |v| float(h, f64::from(v))),
        DataType::UInt32 => prim!(h, a, UInt32Type, |v| float(h, f64::from(v))),
        DataType::UInt64 => prim!(h, a, UInt64Type, |v| {
            h.push(3);
            h.extend_from_slice(&v.to_le_bytes());
        }),
        DataType::Boolean => {
            let b = a.as_boolean();
            for i in 0..b.len() {
                h.extend_from_slice(if b.is_null(i) {
                    &[0]
                } else if b.value(i) {
                    &[4, 1]
                } else {
                    &[4, 0]
                });
            }
        }
        DataType::Utf8 => {
            let s = a.as_string::<i32>();
            for i in 0..s.len() {
                if s.is_null(i) {
                    h.push(0);
                } else {
                    let v = s.value(i).as_bytes();
                    h.push(5);
                    h.extend_from_slice(&(v.len() as u64).to_le_bytes());
                    h.extend_from_slice(v);
                }
            }
        }
        DataType::Dictionary(k, v)
            if k.as_ref() == &DataType::Int32 && v.as_ref() == &DataType::Utf8 =>
        {
            let d = a.as_dictionary::<Int32Type>();
            let values = d.values().as_string::<i32>();
            let keys = d.keys();
            for i in 0..keys.len() {
                if keys.is_null(i) {
                    h.push(0);
                } else {
                    let k = usize::try_from(keys.value(i))
                        .map_err(|_| Error::Other("read-back dictionary key is negative".into()))?;
                    if k >= values.len() {
                        return Err(Error::Other(
                            "read-back dictionary key is out of range".into(),
                        ));
                    }
                    let v = values.value(k).as_bytes();
                    h.push(5);
                    h.extend_from_slice(&(v.len() as u64).to_le_bytes());
                    h.extend_from_slice(v);
                }
            }
        }
        other => {
            return Err(Error::Other(format!(
                "no digest for Arrow type {other} (internal)"
            )));
        }
    }
    Ok(())
}

impl Digest {
    pub(crate) fn update(&mut self, batch: &RecordBatch) -> Result<()> {
        if self.columns.is_empty() {
            self.columns = (0..batch.num_columns()).map(|_| Xxh3::new()).collect();
        }
        if batch.num_columns() != self.columns.len() {
            return Err(Error::Other(format!(
                "read-back batch has {} columns, expected {}",
                batch.num_columns(),
                self.columns.len()
            )));
        }
        self.columns
            .par_iter_mut()
            .zip(batch.columns().par_iter())
            .map(|(h, c)| hash_array(h, c.as_ref()))
            .collect::<Result<Vec<()>>>()?;
        self.rows += batch.num_rows() as u64;
        Ok(())
    }

    fn digests(&self) -> Vec<u128> {
        self.columns.iter().map(Xxh3::digest128).collect()
    }
}

/// Read `path` back and compare it with what was written: the schema (names, types and
/// metadata) and every column's digest.
pub(crate) fn read_back(
    path: &Path,
    format: ColumnarFormat,
    schema: &SchemaRef,
    written: &Digest,
) -> Result<()> {
    let open = || File::open(path).map_err(|e| Error::io(path, e));
    let mut back = Digest::default();
    let read_schema = match format {
        ColumnarFormat::Parquet => {
            let builder =
                parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(open()?)
                    .map_err(|e| Error::Other(format!("Parquet read-back: {e}")))?;
            let s = builder.schema().clone();
            let reader = builder
                .with_batch_size(65_536)
                .build()
                .map_err(|e| Error::Other(format!("Parquet read-back: {e}")))?;
            for b in reader {
                back.update(&b.map_err(|e| Error::Other(format!("Parquet read-back: {e}")))?)?;
            }
            s
        }
        ColumnarFormat::ArrowIpc => {
            let reader = arrow_ipc::reader::FileReader::try_new_buffered(open()?, None)
                .map_err(|e| Error::Other(format!("Arrow IPC read-back: {e}")))?;
            let s = reader.schema();
            for b in reader {
                back.update(&b.map_err(|e| Error::Other(format!("Arrow IPC read-back: {e}")))?)?;
            }
            s
        }
    };
    if read_schema.fields().len() != schema.fields().len() {
        return Err(Error::Other(format!(
            "read-back schema has {} columns, expected {}",
            read_schema.fields().len(),
            schema.fields().len()
        )));
    }
    for (a, b) in read_schema.fields().iter().zip(schema.fields()) {
        if a.name() != b.name() || a.data_type() != b.data_type() || a.metadata() != b.metadata() {
            return Err(Error::Other(format!(
                "read-back column {:?} ({}) differs from the written column {:?} ({})",
                a.name(),
                a.data_type(),
                b.name(),
                b.data_type()
            )));
        }
    }
    for (k, v) in schema.metadata() {
        if read_schema.metadata().get(k) != Some(v) {
            return Err(Error::Other(format!(
                "read-back file metadata {k:?} differs from what was written"
            )));
        }
    }
    if back.rows != written.rows {
        return Err(Error::Other(format!(
            "read back {} rows, wrote {}",
            back.rows, written.rows
        )));
    }
    if written.rows > 0 && back.digests() != written.digests() {
        let bad: Vec<&str> = back
            .digests()
            .iter()
            .zip(written.digests())
            .zip(schema.fields())
            .filter(|((a, b), _)| **a != *b)
            .map(|(_, f)| f.name().as_str())
            .collect();
        return Err(Error::Other(format!(
            "read-back values differ from the written values in column(s) {}",
            bad.join(", ")
        )));
    }
    Ok(())
}

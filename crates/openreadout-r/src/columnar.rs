//! Arrow record batches (from `openreadout_arrow::read_columnar`) to R column vectors.
//!
//! | Arrow type | R vector |
//! | --- | --- |
//! | Int8, Int16, Int32, UInt8, UInt16 | integer |
//! | UInt32, Int64, UInt64, Float32, Float64 | double (exact up to 2^53) |
//! | Boolean | logical |
//! | Utf8, LargeUtf8 | character |
//! | Dictionary of strings (plate `well`) | factor, levels in order of first appearance |
//!
//! Nulls become `NA`.

use std::collections::HashMap;

use extendr_api::prelude::*;
use openreadout_arrow::arrow_array::cast::AsArray;
use openreadout_arrow::arrow_array::types::{
    Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type, UInt8Type, UInt16Type,
    UInt32Type, UInt64Type,
};
use openreadout_arrow::arrow_array::{Array, ArrayRef, RecordBatch};
use openreadout_arrow::arrow_schema::{DataType, SchemaRef};
use openreadout_core::{Error, Result};

enum Col {
    Int(Vec<Rint>),
    Dbl(Vec<Rfloat>),
    Lgl(Vec<Rbool>),
    Str(Vec<Option<String>>),
    /// Factor codes (1-based, NA for null) and levels.
    Factor(Vec<Rint>, Vec<String>, HashMap<String, i32>),
}

fn unsupported(name: &str, t: &DataType) -> Error {
    Error::Other(format!(
        "column {name}: Arrow type {t} has no R conversion here"
    ))
}

fn push_int<T>(v: &mut Vec<Rint>, a: &ArrayRef, f: impl Fn(T::Native) -> i32)
where
    T: openreadout_arrow::arrow_array::types::ArrowPrimitiveType,
{
    let p = a.as_primitive::<T>();
    v.extend((0..p.len()).map(|i| {
        if p.is_null(i) {
            Rint::na()
        } else {
            Rint::from(f(p.value(i)))
        }
    }));
}

fn push_dbl<T>(v: &mut Vec<Rfloat>, a: &ArrayRef, f: impl Fn(T::Native) -> f64)
where
    T: openreadout_arrow::arrow_array::types::ArrowPrimitiveType,
{
    let p = a.as_primitive::<T>();
    v.extend((0..p.len()).map(|i| {
        if p.is_null(i) {
            Rfloat::na()
        } else {
            Rfloat::from(f(p.value(i)))
        }
    }));
}

/// The string values of a Utf8/LargeUtf8 array, or of a dictionary's values array.
fn strings(a: &dyn Array) -> Option<Vec<Option<String>>> {
    match a.data_type() {
        DataType::Utf8 => {
            let s = a.as_string::<i32>();
            Some(
                (0..s.len())
                    .map(|i| (!s.is_null(i)).then(|| s.value(i).to_string()))
                    .collect(),
            )
        }
        DataType::LargeUtf8 => {
            let s = a.as_string::<i64>();
            Some(
                (0..s.len())
                    .map(|i| (!s.is_null(i)).then(|| s.value(i).to_string()))
                    .collect(),
            )
        }
        _ => None,
    }
}

impl Col {
    fn new(name: &str, t: &DataType) -> Result<Col> {
        Ok(match t {
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::UInt8
            | DataType::UInt16 => Col::Int(Vec::new()),
            DataType::UInt32
            | DataType::Int64
            | DataType::UInt64
            | DataType::Float32
            | DataType::Float64 => Col::Dbl(Vec::new()),
            DataType::Boolean => Col::Lgl(Vec::new()),
            DataType::Utf8 | DataType::LargeUtf8 => Col::Str(Vec::new()),
            DataType::Dictionary(_, v) if matches!(**v, DataType::Utf8 | DataType::LargeUtf8) => {
                Col::Factor(Vec::new(), Vec::new(), HashMap::new())
            }
            other => return Err(unsupported(name, other)),
        })
    }

    fn push(&mut self, name: &str, a: &ArrayRef) -> Result<()> {
        match self {
            Col::Int(v) => match a.data_type() {
                DataType::Int8 => push_int::<Int8Type>(v, a, i32::from),
                DataType::Int16 => push_int::<Int16Type>(v, a, i32::from),
                DataType::Int32 => push_int::<Int32Type>(v, a, |x| x),
                DataType::UInt8 => push_int::<UInt8Type>(v, a, i32::from),
                DataType::UInt16 => push_int::<UInt16Type>(v, a, i32::from),
                other => return Err(unsupported(name, other)),
            },
            Col::Dbl(v) => match a.data_type() {
                DataType::UInt32 => push_dbl::<UInt32Type>(v, a, f64::from),
                DataType::Int64 => push_dbl::<Int64Type>(v, a, |x| x as f64),
                DataType::UInt64 => push_dbl::<UInt64Type>(v, a, |x| x as f64),
                DataType::Float32 => push_dbl::<Float32Type>(v, a, f64::from),
                DataType::Float64 => push_dbl::<Float64Type>(v, a, |x| x),
                other => return Err(unsupported(name, other)),
            },
            Col::Lgl(v) => {
                let b = a
                    .as_boolean_opt()
                    .ok_or_else(|| unsupported(name, a.data_type()))?;
                v.extend((0..b.len()).map(|i| {
                    if b.is_null(i) {
                        Rbool::na()
                    } else {
                        Rbool::from(b.value(i))
                    }
                }));
            }
            Col::Str(v) => {
                v.extend(strings(a.as_ref()).ok_or_else(|| unsupported(name, a.data_type()))?);
            }
            Col::Factor(codes, levels, seen) => {
                let d = a
                    .as_any_dictionary_opt()
                    .ok_or_else(|| unsupported(name, a.data_type()))?;
                let values =
                    strings(d.values().as_ref()).ok_or_else(|| unsupported(name, a.data_type()))?;
                let keys = d.normalized_keys();
                for i in 0..a.len() {
                    let v = if a.is_null(i) {
                        None
                    } else {
                        keys.get(i)
                            .and_then(|&k| values.get(k))
                            .and_then(Clone::clone)
                    };
                    codes.push(match v {
                        None => Rint::na(),
                        Some(s) => {
                            let next = levels.len() as i32 + 1;
                            let code = *seen.entry(s.clone()).or_insert_with(|| {
                                levels.push(s);
                                next
                            });
                            Rint::from(code)
                        }
                    });
                }
            }
        }
        Ok(())
    }

    fn into_robj(self) -> Result<Robj> {
        Ok(match self {
            Col::Int(v) => Integers::from_values(v).into(),
            Col::Dbl(v) => Doubles::from_values(v).into(),
            Col::Lgl(v) => Logicals::from_values(v).into(),
            Col::Str(v) => Strings::from_values(v.into_iter().map(|s| match s {
                Some(s) => Rstr::from(s),
                None => Rstr::na(),
            }))
            .into(),
            Col::Factor(codes, levels, _) => {
                let mut f: Robj = Integers::from_values(codes).into();
                f.set_attrib("levels", Strings::from_values(levels))
                    .and_then(|f| f.set_class(["factor"]))
                    .map_err(|e| Error::Other(e.to_string()))?;
                f
            }
        })
    }
}

/// `list(columns = <named list>, meta = <JSON>)`: the columns of the batches, and for each
/// column its unit, label and stored dtype, plus the schema metadata.
pub fn to_r(schema: &SchemaRef, batches: &[RecordBatch]) -> Result<Robj> {
    let fields = schema.fields();
    let mut cols = fields
        .iter()
        .map(|f| Col::new(f.name(), f.data_type()))
        .collect::<Result<Vec<_>>>()?;
    for b in batches {
        for (i, col) in cols.iter_mut().enumerate() {
            col.push(fields[i].name(), b.column(i))?;
        }
    }
    let names: Vec<String> = fields.iter().map(|f| f.name().clone()).collect();
    let values = cols
        .into_iter()
        .map(Col::into_robj)
        .collect::<Result<Vec<_>>>()?;
    let mut list = List::from_values(values);
    list.set_names(names)
        .map_err(|e| Error::Other(e.to_string()))?;
    let meta = serde_json::json!({
        "columns": fields.iter().map(|f| {
            let m = f.metadata();
            serde_json::json!({
                "name": f.name(),
                "unit": m.get(openreadout_arrow::meta::KEY_UNIT),
                "label": m.get(openreadout_arrow::meta::KEY_LABEL),
                "dtype": m.get(openreadout_arrow::meta::KEY_DTYPE),
            })
        }).collect::<Vec<_>>(),
        "schema": schema
            .metadata()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<std::collections::BTreeMap<String, String>>(),
    });
    let meta = serde_json::to_string(&meta)
        .map_err(|e| Error::Other(format!("JSON serialization failed: {e}")))?;
    Ok(list!(columns = list, meta = meta).into())
}

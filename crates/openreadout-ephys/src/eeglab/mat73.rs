//! MATLAB 7.3 MAT-files: HDF5 files after a 512-byte userblock. Each variable is a dataset or
//! a group carrying a `MATLAB_class` attribute; HDF5 shapes are MATLAB's dimensions reversed,
//! so the bytes are in MATLAB's column-major order; characters are UTF-16 code units; an empty
//! array is a dataset of its dimensions with `MATLAB_empty`; struct arrays and cells hold object
//! references into `#refs#`. Parsed into the same [`MatValue`] tree as level-5 files. Notes:
//! `docs/formats/eeglab.md`.

use std::path::Path;

use hdf5_pure::{DType, Dataset, Group, Object};
use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};
use openreadout_hdf5::h5util::{attr_text, big_endian, open_h5_in};

use super::EEGLAB_FORMAT_ID;
use super::mat::{
    MAX_MAT_DEPTH, MAX_MAT_INLINE, MAX_MAT_NODES, MAX_MAT_TEXT, MI_DOUBLE, MI_INT8, MI_INT16,
    MI_INT32, MI_INT64, MI_SINGLE, MI_UINT8, MI_UINT16, MI_UINT32, MI_UINT64, MatFile, MatNumbers,
    MatValue, MatVariable, MiType, decode_values,
};

/// The level-5 type a dataset's elements correspond to, and their byte order.
fn stored_type(ds: &Dataset) -> Option<(MiType, bool)> {
    let t = match ds.dtype().ok()? {
        DType::F64 => MI_DOUBLE,
        DType::F32 => MI_SINGLE,
        DType::I8 => MI_INT8,
        DType::U8 => MI_UINT8,
        DType::I16 => MI_INT16,
        DType::U16 => MI_UINT16,
        DType::I32 => MI_INT32,
        DType::U32 => MI_UINT32,
        DType::I64 => MI_INT64,
        DType::U64 => MI_UINT64,
        _ => return None,
    };
    let be = ds.datatype().is_ok_and(|d| big_endian(&d));
    Some((t, be))
}

fn h5err(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::corrupt(
        EEGLAB_FORMAT_ID,
        format!(
            "{}: the MAT 7.3 (HDF5) structure cannot be read: {e}",
            path.display()
        ),
    )
}

fn class_code(name: &str) -> u8 {
    match name {
        "cell" => 1,
        "struct" => 2,
        "char" => 4,
        "double" => 6,
        "single" => 7,
        "int8" => 8,
        "uint8" | "logical" => 9,
        "int16" => 10,
        "uint16" => 11,
        "int32" => 12,
        "uint32" => 13,
        "int64" => 14,
        "uint64" => 15,
        _ => 0,
    }
}

struct Walker<'a> {
    path: &'a Path,
    nodes: usize,
}

impl Walker<'_> {
    fn count(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        if self.nodes > MAX_MAT_NODES || depth > MAX_MAT_DEPTH {
            return Err(Error::unsupported(
                EEGLAB_FORMAT_ID,
                "MAT-files nested this deeply or with this many arrays",
                "The file may be damaged; the limits are in docs/formats/eeglab.md.",
            ));
        }
        Ok(())
    }

    fn dataset(&mut self, ds: &Dataset, depth: usize) -> Result<MatValue> {
        self.count(depth)?;
        let attrs = ds.attrs().map_err(|e| h5err(self.path, e))?;
        let class = attrs
            .get("MATLAB_class")
            .and_then(attr_text)
            .unwrap_or_default();
        let shape = ds.shape().map_err(|e| h5err(self.path, e))?;
        let empty = attrs.contains_key("MATLAB_empty");
        let dims: Vec<u64> = if empty {
            vec![0, 0]
        } else {
            shape.iter().rev().copied().collect()
        };
        let count = dims
            .iter()
            .try_fold(1u64, |a, &d| a.checked_mul(d))
            .filter(|n| *n <= super::mat::MAX_MAT_ELEMENTS)
            .ok_or_else(|| h5err(self.path, "a dataset's shape is too large"))?;
        if ds.dtype().map_err(|e| h5err(self.path, e))? == DType::ObjectReference {
            if count > MAX_MAT_NODES as u64 {
                return Err(h5err(
                    self.path,
                    "a cell or struct array declares more elements than are read",
                ));
            }
            let objs = ds.dereference().map_err(|e| h5err(self.path, e))?;
            let mut items = Vec::with_capacity(objs.len());
            for o in &objs {
                items.push(self.object(o, depth + 1)?);
            }
            return Ok(MatValue::Cell { dims, items });
        }
        if empty {
            return Ok(if class == "char" {
                MatValue::Char {
                    dims,
                    text: String::new(),
                }
            } else {
                MatValue::Numeric {
                    class: class_code(&class).max(6),
                    dims,
                    data: MatNumbers {
                        stored: MI_DOUBLE,
                        count: 0,
                        values: Some(Vec::new()),
                        at: 0,
                    },
                }
            });
        }
        let Some((stored, be)) = stored_type(ds) else {
            return Ok(MatValue::Other {
                class: class_code(&class),
            });
        };
        let size = super::mat::mi_size(stored);
        if class == "char" {
            if count.saturating_mul(2) > MAX_MAT_TEXT {
                return Err(h5err(self.path, "a character array is too large"));
            }
            let raw = ds.read_raw().map_err(|e| h5err(self.path, e))?;
            let units: Vec<u16> = decode_values(be, MI_UINT16, &raw)
                .into_iter()
                .map(|v| v as u16)
                .collect();
            let chars: Vec<char> = char::decode_utf16(units.iter().copied())
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect();
            // rows only as many as the characters stored allow (a declared row count is not trusted)
            let rows = usize::try_from(dims.first().copied().unwrap_or(1))
                .unwrap_or(1)
                .clamp(1, chars.len().max(1));
            let text = if rows == 1 {
                chars.into_iter().collect()
            } else {
                let cols = chars.len() / rows;
                (0..rows)
                    .map(|r| {
                        (0..cols)
                            .filter_map(|c| chars.get(r + rows * c))
                            .collect::<String>()
                            .trim_end()
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            return Ok(MatValue::Char { dims, text });
        }
        let values = if count.saturating_mul(size) <= MAX_MAT_INLINE {
            let raw = ds.read_raw().map_err(|e| h5err(self.path, e))?;
            Some(decode_values(be, stored, &raw))
        } else {
            None
        };
        Ok(MatValue::Numeric {
            class: class_code(&class).max(6),
            dims,
            data: MatNumbers {
                stored,
                count,
                values,
                at: 0,
            },
        })
    }

    fn object(&mut self, o: &Object, depth: usize) -> Result<MatValue> {
        match o {
            Object::Dataset(ds) => self.dataset(ds, depth),
            Object::Group(g) => self.group(g, depth),
            _ => Ok(MatValue::Other { class: 0 }),
        }
    }

    fn group(&mut self, g: &Group, depth: usize) -> Result<MatValue> {
        self.count(depth)?;
        let attrs = g.attrs().map_err(|e| h5err(self.path, e))?;
        let class = attrs
            .get("MATLAB_class")
            .and_then(attr_text)
            .unwrap_or_default();
        if !class.is_empty() && class != "struct" {
            return Ok(MatValue::Other {
                class: class_code(&class),
            });
        }
        let mut names: Vec<(String, bool)> = Vec::new();
        for n in g.datasets().map_err(|e| h5err(self.path, e))? {
            names.push((n, false));
        }
        for n in g.groups().map_err(|e| h5err(self.path, e))? {
            names.push((n, true));
        }
        names.retain(|(n, _)| !n.starts_with('#'));
        names.sort();
        // a struct array: every field a dataset of references without a MATLAB class
        let mut array: Option<(Vec<u64>, Vec<Vec<Object>>)> = None;
        if !names.is_empty() && names.iter().all(|(_, grp)| !grp) {
            let mut fields = Vec::new();
            let mut dims = None;
            for (n, _) in &names {
                let ds = g.dataset(n).map_err(|e| h5err(self.path, e))?;
                let a = ds.attrs().map_err(|e| h5err(self.path, e))?;
                let refs = ds.dtype().map_err(|e| h5err(self.path, e))? == DType::ObjectReference
                    && !a.contains_key("MATLAB_class");
                if !refs {
                    fields.clear();
                    break;
                }
                let shape = ds.shape().map_err(|e| h5err(self.path, e))?;
                dims.get_or_insert_with(|| shape.iter().rev().copied().collect::<Vec<u64>>());
                fields.push(ds.dereference().map_err(|e| h5err(self.path, e))?);
            }
            if fields.len() == names.len() {
                array = dims.map(|d| (d, fields));
            }
        }
        let field_names: Vec<String> = names.iter().map(|(n, _)| n.clone()).collect();
        if let Some((dims, fields)) = array {
            let n = fields.first().map_or(0, Vec::len);
            let mut values = Vec::with_capacity(n * fields.len());
            for e in 0..n {
                for f in &fields {
                    let v = match f.get(e) {
                        Some(o) => self.object(o, depth + 1)?,
                        None => MatValue::Other { class: 0 },
                    };
                    values.push(v);
                }
            }
            return Ok(MatValue::Struct {
                dims,
                fields: field_names,
                values,
                class_name: None,
            });
        }
        let mut values = Vec::with_capacity(names.len());
        for (n, grp) in &names {
            let v = if *grp {
                let sub = g.group(n).map_err(|e| h5err(self.path, e))?;
                self.group(&sub, depth + 1)?
            } else {
                let ds = g.dataset(n).map_err(|e| h5err(self.path, e))?;
                self.dataset(&ds, depth + 1)?
            };
            values.push(v);
        }
        Ok(MatValue::Struct {
            dims: vec![1, 1],
            fields: field_names,
            values,
            class_name: None,
        })
    }
}

/// Parse a MAT 7.3 file: every top-level variable (large numeric arrays only described).
pub fn parse_mat73(fs: &Fs, path: &Path, header_text: String) -> Result<MatFile> {
    let file = open_h5_in(fs, path, EEGLAB_FORMAT_ID)?;
    let root = file.root();
    let mut w = Walker { path, nodes: 0 };
    let mut variables = Vec::new();
    let mut names: Vec<(String, bool)> = Vec::new();
    for n in root.datasets().map_err(|e| h5err(path, e))? {
        names.push((n, false));
    }
    for n in root.groups().map_err(|e| h5err(path, e))? {
        names.push((n, true));
    }
    names.retain(|(n, _)| !n.starts_with('#'));
    names.sort();
    for (n, grp) in names {
        let value = if grp {
            w.group(&root.group(&n).map_err(|e| h5err(path, e))?, 0)?
        } else {
            w.dataset(&root.dataset(&n).map_err(|e| h5err(path, e))?, 0)?
        };
        variables.push(MatVariable {
            name: n,
            value,
            compressed: None,
        });
    }
    Ok(MatFile {
        header_text,
        big_endian: false,
        variables,
        hdf5: true,
    })
}

/// Raw bytes of rows `[first, first + n)` of the dataset at `name` (rows along HDF5's first
/// dimension), and their stored type and byte order.
pub fn read_rows(
    fs: &Fs,
    path: &Path,
    name: &str,
    first: u64,
    n: u64,
) -> Result<(Vec<u8>, MiType, bool)> {
    let file = open_h5_in(fs, path, EEGLAB_FORMAT_ID)?;
    let ds = file.dataset(name).map_err(|e| h5err(path, e))?;
    let (t, be) = stored_type(&ds).ok_or_else(|| {
        Error::unsupported(
            EEGLAB_FORMAT_ID,
            "a data array of this HDF5 type",
            "Resave the dataset in EEGLAB; the samples should be single or double.",
        )
    })?;
    let raw = ds.read_raw_rows(first, n).map_err(|e| h5err(path, e))?;
    Ok((raw, t, be))
}

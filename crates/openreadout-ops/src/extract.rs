//! `extract`: write one embedded attachment's raw bytes to a new file, verified by
//! read-back. Shared by the CLI and the MCP server.

use std::path::{Path, PathBuf};

use openreadout_core::error::{Error, Result};
use openreadout_core::model::{AttachmentInfo, ExtractOutput};
use openreadout_core::reader::Dataset;

/// Pick an attachment by name (exact, then case-insensitive), by `#<index>`, or by a bare index.
pub fn resolve_attachment(list: &[AttachmentInfo], selector: &str) -> Result<AttachmentInfo> {
    let names = || {
        list.iter()
            .map(|a| format!("#{} {} ({})", a.index, a.name, a.content_type))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if list.is_empty() {
        return Err(Error::Usage("this file has no attachments".into()));
    }
    if let Some(a) = list.iter().find(|a| a.name == selector) {
        return Ok(a.clone());
    }
    if let Some(a) = list.iter().find(|a| a.name.eq_ignore_ascii_case(selector)) {
        return Ok(a.clone());
    }
    let idx = selector.strip_prefix('#').unwrap_or(selector);
    if let Ok(i) = idx.parse::<u32>()
        && let Some(a) = list.iter().find(|a| a.index == i)
    {
        return Ok(a.clone());
    }
    Err(Error::Usage(format!(
        "no attachment named '{selector}'; available: {}",
        names()
    )))
}

/// Default output path: `<input stem>.<attachment name>.<extension>` next to the input.
pub fn default_output(input: &Path, a: &AttachmentInfo) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "file".into(), |s| s.to_string_lossy().to_string());
    let name: String = a
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = if name.is_empty() {
        format!("attachment{}", a.index)
    } else {
        name
    };
    input.with_file_name(format!("{stem}.{name}.{}", a.extension))
}

/// Extract attachment `selector` of `ds` (opened from `input`) to `output`.
pub fn extract_attachment(
    ds: &mut dyn Dataset,
    input: &Path,
    format: &str,
    selector: &str,
    output: Option<&Path>,
    overwrite: bool,
) -> Result<ExtractOutput> {
    let list = ds.attachments()?;
    let a = resolve_attachment(&list, selector)?;
    let output = output.map_or_else(|| default_output(input, &a), Path::to_path_buf);
    if output == input
        || (output.exists()
            && std::fs::canonicalize(&output).ok() == std::fs::canonicalize(input).ok())
    {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    if output.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let bytes = ds.read_attachment(a.index)?;
    let hash = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
    let tmp = output.with_file_name(format!(
        ".{}.partial-{}",
        output
            .file_name()
            .map_or_else(|| "extract".into(), |s| s.to_string_lossy().to_string()),
        std::process::id()
    ));
    std::fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
    // Hash the written file as a stream: no second copy of a large attachment in memory.
    let back = hash_file(&tmp).map_err(|e| Error::io(&tmp, e));
    let verified = back.as_ref().is_ok_and(|b| *b == hash);
    if !verified {
        let _ = std::fs::remove_file(&tmp);
        back?;
        return Err(Error::Other(
            "read-back verification failed; output discarded".into(),
        ));
    }
    std::fs::rename(&tmp, &output).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(&output, e)
    })?;
    Ok(ExtractOutput {
        path: input.display().to_string(),
        format: format.into(),
        attachment: a,
        output: output.display().to_string(),
        bytes_written: bytes.len() as u64,
        xxh3: hash,
        verified,
    })
}

/// xxh3-128 of a file's bytes as 32 hex digits, read in 1 MiB pieces.
fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:032x}", h.digest128()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(index: u32, name: &str) -> AttachmentInfo {
        AttachmentInfo {
            index,
            name: name.into(),
            content_type: "JPG".into(),
            extension: "jpg".into(),
            ..AttachmentInfo::default()
        }
    }

    #[test]
    fn resolves_by_name_case_and_index() {
        let l = [att(0, "Thumbnail"), att(1, "Label")];
        assert_eq!(resolve_attachment(&l, "Label").unwrap().index, 1);
        assert_eq!(resolve_attachment(&l, "thumbnail").unwrap().index, 0);
        assert_eq!(resolve_attachment(&l, "#1").unwrap().index, 1);
        assert_eq!(resolve_attachment(&l, "0").unwrap().index, 0);
        let e = resolve_attachment(&l, "nope").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("Thumbnail"));
        assert!(resolve_attachment(&[], "x").is_err());
    }

    #[test]
    fn default_output_is_next_to_input() {
        let p = default_output(Path::new("/d/slide.czi"), &att(0, "Slide Preview"));
        assert_eq!(p, Path::new("/d/slide.Slide_Preview.jpg"));
    }
}

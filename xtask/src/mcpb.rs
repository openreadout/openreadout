//! `cargo xtask mcpb pack`: a .mcpb bundle (a zip of `manifest.json` and the binary) for
//! MCP clients that install servers from bundles.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::{root, sha256_file};

/// The MCP-bundle platform name (`compatibility.platforms`) of a Rust target triple.
fn platform(target: &str) -> Result<&'static str> {
    if target.contains("apple-darwin") {
        Ok("darwin")
    } else if target.contains("windows") {
        Ok("win32")
    } else if target.contains("linux") {
        Ok("linux")
    } else {
        bail!("no MCP-bundle platform for target {target}")
    }
}

/// The bundle manifest for one target: `mcpb/manifest.json` with `compatibility.platforms`
/// narrowed to the platform the bundled binary runs on (a bundle carries one binary).
fn manifest(manifest_src: &str, target: &str) -> Result<String> {
    let mut manifest: serde_json::Value =
        serde_json::from_str(manifest_src).context("parse mcpb/manifest.json")?;
    let platform = platform(target)?;
    manifest
        .get_mut("compatibility")
        .and_then(serde_json::Value::as_object_mut)
        .context("mcpb/manifest.json has no `compatibility` object")?
        .insert("platforms".into(), serde_json::json!([platform]));
    Ok(serde_json::to_string_pretty(&manifest)? + "\n")
}

/// A .mcpb is a zip: `manifest.json`, `icon.png` and `server/<binary>`, written with a minimal zip writer.
pub(crate) fn pack(binary: &Path, target: &str, out: &PathBuf) -> Result<()> {
    let manifest = manifest(
        &fs::read_to_string(root().join("mcpb/manifest.json"))?,
        target,
    )?;
    let icon = fs::read(root().join("assets/icon.png")).context("read assets/icon.png")?;
    let bin = fs::read(binary).with_context(|| format!("read {}", binary.display()))?;
    let is_win = target.contains("windows");
    let bin_name = if is_win {
        "server/openreadout.exe"
    } else {
        "server/openreadout"
    };
    fs::create_dir_all(out)?;
    let zip_path = out.join(format!("openreadout-mcp-{target}.mcpb"));
    let mut z = ZipWriter::default();
    z.add("manifest.json", manifest.as_bytes(), false)?;
    z.add("icon.png", &icon, false)?;
    z.add(bin_name, &bin, true)?;
    fs::write(&zip_path, z.finish())?;
    let (sha, n) = sha256_file(&zip_path)?;
    println!("wrote {} ({} bytes)\nsha256 {sha}", zip_path.display(), n);
    Ok(())
}

/// Minimal ZIP writer (deflate when it saves space, else stored; no zip64) — enough for .mcpb
/// bundles. Timestamps are zero, so the output depends only on the inputs.
#[derive(Default)]
struct ZipWriter {
    data: Vec<u8>,
    central: Vec<u8>,
    entries: u16,
}

impl ZipWriter {
    fn add(&mut self, name: &str, bytes: &[u8], executable: bool) -> Result<()> {
        let crc = {
            let mut c = flate2::Crc::new();
            c.update(bytes);
            c.sum()
        };
        let offset = u32::try_from(self.data.len()).context("bundle exceeds 4 GiB")?;
        let size = u32::try_from(bytes.len()).context("entry exceeds 4 GiB")?;
        let nb = name.as_bytes();
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(bytes)?;
        let deflated = enc.finish()?;
        let (method, payload): (u8, &[u8]) = if deflated.len() < bytes.len() {
            (8, &deflated)
        } else {
            (0, bytes)
        };
        let stored_size = u32::try_from(payload.len()).context("entry exceeds 4 GiB")?;
        // local file header: signature, version needed 2.0, flags 0, method, time and date 0
        self.data
            .extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, method, 0, 0, 0, 0, 0]);
        self.data.extend_from_slice(&crc.to_le_bytes());
        self.data.extend_from_slice(&stored_size.to_le_bytes());
        self.data.extend_from_slice(&size.to_le_bytes());
        self.data
            .extend_from_slice(&(nb.len() as u16).to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data.extend_from_slice(nb);
        self.data.extend_from_slice(payload);
        // central directory entry (version made by 3 = unix, external attrs carry the mode)
        let mode: u32 = if executable { 0o100_755 } else { 0o100_644 };
        self.central.extend_from_slice(&[
            0x50, 0x4b, 0x01, 0x02, 20, 3, 20, 0, 0, 0, method, 0, 0, 0, 0, 0,
        ]);
        self.central.extend_from_slice(&crc.to_le_bytes());
        self.central.extend_from_slice(&stored_size.to_le_bytes());
        self.central.extend_from_slice(&size.to_le_bytes());
        self.central
            .extend_from_slice(&(nb.len() as u16).to_le_bytes());
        self.central.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        self.central.extend_from_slice(&(mode << 16).to_le_bytes());
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(nb);
        self.entries += 1;
        Ok(())
    }
    fn finish(mut self) -> Vec<u8> {
        let cd_off = self.data.len() as u32;
        let cd_len = self.central.len() as u32;
        self.data.extend_from_slice(&self.central);
        self.data
            .extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        self.data.extend_from_slice(&self.entries.to_le_bytes());
        self.data.extend_from_slice(&self.entries.to_le_bytes());
        self.data.extend_from_slice(&cd_len.to_le_bytes());
        self.data.extend_from_slice(&cd_off.to_le_bytes());
        self.data.extend_from_slice(&0u16.to_le_bytes());
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::extract_bundle;

    #[test]
    fn mcpb_bundle_round_trips_and_names_one_platform() {
        let dir = std::env::temp_dir().join(format!("xtask-mcpb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("openreadout");
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&bin, &payload).unwrap();
        pack(&bin, "x86_64-unknown-linux-musl", &dir).unwrap();
        let bundle = dir.join("openreadout-mcp-x86_64-unknown-linux-musl.mcpb");
        // deflated, not stored
        assert!(fs::metadata(&bundle).unwrap().len() < payload.len() as u64);
        let unpacked = dir.join("unpacked");
        assert_eq!(extract_bundle(&bundle, &unpacked, 0).unwrap(), 3);
        assert!(unpacked.join("icon.png").is_file());
        assert_eq!(
            fs::read(unpacked.join("server/openreadout")).unwrap(),
            payload
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(unpacked.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["icon"], "icon.png");
        assert_eq!(
            manifest["compatibility"]["platforms"],
            serde_json::json!(["linux"])
        );
        assert_eq!(platform("aarch64-apple-darwin").unwrap(), "darwin");
        assert_eq!(platform("x86_64-pc-windows-msvc").unwrap(), "win32");
        fs::remove_dir_all(&dir).unwrap();
    }
}

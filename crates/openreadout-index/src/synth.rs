//! Small synthetic instrument files for tests and benchmarks (no corpus needed): the same two
//! fixtures as `openreadout self doctor` (an uncompressed two-page TIFF and an FCS 3.1 file),
//! with FCS keywords of the caller's choosing. Every name and value is invented.

use std::path::Path;

/// A classic little-endian TIFF with `pages` uncompressed `w`×`h` uint16 pages whose samples
/// start at `seed`.
pub fn tiff(w: u32, h: u32, pages: u32, seed: u16) -> Vec<u8> {
    const ENTRIES: u16 = 10;
    let ifd_len = 2 + 12 * u32::from(ENTRIES) + 4;
    let page_len = w * h * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    for p in 0..pages {
        let ifd = 8 + p * (ifd_len + page_len);
        let data = ifd + ifd_len;
        let next = if p + 1 < pages { data + page_len } else { 0 };
        out.extend_from_slice(&ENTRIES.to_le_bytes());
        let mut entry = |tag: u16, typ: u16, value: u32| {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            if typ == 3 {
                out.extend_from_slice(&(value as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            } else {
                out.extend_from_slice(&value.to_le_bytes());
            }
        };
        entry(256, 3, w);
        entry(257, 3, h);
        entry(258, 3, 16);
        entry(259, 3, 1);
        entry(262, 3, 1);
        entry(273, 4, data);
        entry(277, 3, 1);
        entry(278, 3, h);
        entry(279, 4, page_len);
        entry(339, 3, 1);
        out.extend_from_slice(&next.to_le_bytes());
        for i in 0..(w * h) {
            out.extend_from_slice(&(seed.wrapping_add((p * 1000 + i) as u16)).to_le_bytes());
        }
    }
    out
}

/// An FCS 3.1 list-mode file with `events` × 3 float32 parameters (`FSC-A`, `SSC-A`, `Time`)
/// and extra TEXT keywords (`$OP`, `$SMNO`, `$COM`, `$DATE`, `$BTIM`, vendor keywords, ...).
pub fn fcs(events: usize, extra: &[(&str, &str)]) -> Vec<u8> {
    const PARAMS: [&str; 3] = ["FSC-A", "SSC-A", "Time"];
    let data: Vec<u8> = (0..events)
        .flat_map(|e| (0..PARAMS.len()).map(move |p| (e * 10 + p) as f32 + 0.5))
        .flat_map(f32::to_le_bytes)
        .collect();
    let text_start = 58usize;
    let build_text = |data_start: usize, data_end: usize| {
        let mut kv: Vec<(String, String)> = vec![
            ("$BEGINANALYSIS".into(), "0".into()),
            ("$ENDANALYSIS".into(), "0".into()),
            ("$BEGINSTEXT".into(), "0".into()),
            ("$ENDSTEXT".into(), "0".into()),
            ("$BEGINDATA".into(), format!("{data_start:08}")),
            ("$ENDDATA".into(), format!("{data_end:08}")),
            ("$BYTEORD".into(), "1,2,3,4".into()),
            ("$DATATYPE".into(), "F".into()),
            ("$MODE".into(), "L".into()),
            ("$NEXTDATA".into(), "0".into()),
            ("$PAR".into(), PARAMS.len().to_string()),
            ("$TOT".into(), events.to_string()),
            ("$CYT".into(), "Synthetic Cytometer".into()),
        ];
        for (i, n) in PARAMS.iter().enumerate() {
            let k = i + 1;
            kv.push((format!("$P{k}N"), (*n).to_string()));
            kv.push((format!("$P{k}B"), "32".into()));
            kv.push((format!("$P{k}E"), "0,0".into()));
            kv.push((format!("$P{k}R"), "1024".into()));
        }
        for (k, v) in extra {
            kv.push(((*k).to_string(), (*v).replace('|', "||")));
        }
        let mut t = String::from("|");
        for (k, v) in kv {
            t.push_str(&k);
            t.push('|');
            t.push_str(&v);
            t.push('|');
        }
        t.into_bytes()
    };
    let len = build_text(0, 0).len();
    let data_start = text_start + len;
    let data_end = data_start + data.len().max(1) - 1;
    let text = build_text(data_start, data_end);
    let mut out = format!(
        "FCS3.1    {:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
        text_start,
        text_start + text.len() - 1,
        data_start,
        data_end,
        0,
        0
    )
    .into_bytes();
    out.extend_from_slice(&text);
    out.extend_from_slice(&data);
    out
}

/// A small synthetic lab share under `dir`: TIFFs, FCS files with operators, samples and
/// comments (all invented), a byte-identical copy, a truncated FCS, README files and other
/// unrecognised files, nested directories and a hidden file. Returns the number of files
/// written.
pub fn share(dir: &Path) -> std::io::Result<usize> {
    let mut n = 0;
    let mut put = |rel: &str, bytes: &[u8]| -> std::io::Result<()> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, bytes)?;
        n += 1;
        Ok(())
    };
    put("imaging/2019/cells_a.tif", &tiff(16, 8, 2, 0))?;
    put("imaging/2019/cells_b.tif", &tiff(32, 16, 3, 7))?;
    put("imaging/2019/cells_b_copy.tif", &tiff(32, 16, 3, 7))?;
    put(
        "imaging/2019/notes.txt",
        b"plates imaged on the widefield\n",
    )?;
    put(
        "flow/run1/tube_A12.fcs",
        &fcs(
            20,
            &[
                ("$OP", "Jane Roe"),
                ("$SMNO", "A12"),
                ("$DATE", "03-MAY-2019"),
                ("$BTIM", "10:11:12"),
                ("$ETIM", "10:21:12"),
                (
                    "$COM",
                    "second tube after the clog, contact jroe@example.org",
                ),
            ],
        ),
    )?;
    put(
        "flow/run1/tube_B03.fcs",
        &fcs(
            30,
            &[
                ("$OP", "Jane Roe"),
                ("$SMNO", "roe_B03"),
                ("$DATE", "04-MAY-2021"),
                ("$BTIM", "09:00:00"),
            ],
        ),
    )?;
    let good = fcs(
        40,
        &[
            ("$SMNO", "C07"),
            ("$DATE", "05-MAY-2021"),
            ("$BTIM", "08:00:00"),
        ],
    );
    put("flow/run2/tube_C07.fcs", &good)?;
    put(
        "flow/run2/tube_C07_truncated.fcs",
        &good[..good.len() - 200],
    )?;
    put("flow/run2/README.md", b"# run 2\n")?;
    put("misc/data.bin", &[0u8; 300])?;
    put("misc/.DS_Store", b"hidden")?;
    put("misc/empty", b"")?;
    Ok(n)
}

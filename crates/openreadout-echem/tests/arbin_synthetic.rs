//! Synthetic Arbin `.res` databases built from `docs/formats/arbin-res.md` (a minimal Jet 4 file:
//! catalog, test table, data points out of order, an auxiliary temperature): the trace, facts
//! and tables read back; damage ends in clean errors.
#![allow(clippy::float_cmp, clippy::cast_possible_truncation)] // exact synthetic values

use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};
use openreadout_echem::{ARBIN_FORMAT_ID, ArbinReader};

const PAGE: usize = 4096;

#[derive(Clone)]
enum V {
    Long(i32),
    Int(i16),
    Double(f64),
    Single(f32),
    Text(&'static str),
    Null,
}

struct Col {
    name: &'static str,
    kind: u8,
    fixed: bool,
    offset: u16,
    len: u16,
    var: u16,
}

fn col(name: &'static str, kind: u8, fixed_at: Option<u16>, var: u16) -> Col {
    let len = match kind {
        2 => 1,
        3 => 2,
        4 | 6 => 4,
        7 | 8 => 8,
        _ => 510,
    };
    Col {
        name,
        kind,
        fixed: fixed_at.is_some(),
        offset: fixed_at.unwrap_or(0),
        len,
        var,
    }
}

fn tdef(cols: &[Col], rows: u32) -> Vec<u8> {
    let mut p = vec![0u8; PAGE];
    p[0] = 2;
    p[1] = 1;
    p[16..20].copy_from_slice(&rows.to_le_bytes());
    p[45..47].copy_from_slice(&(cols.len() as u16).to_le_bytes());
    let mut at = 63;
    for (i, c) in cols.iter().enumerate() {
        let e = &mut p[at..at + 25];
        e[0] = c.kind;
        e[5..7].copy_from_slice(&(i as u16).to_le_bytes());
        e[7..9].copy_from_slice(&c.var.to_le_bytes());
        e[9..11].copy_from_slice(&(i as u16).to_le_bytes());
        e[15] = if c.fixed { 0x03 } else { 0x02 };
        e[21..23].copy_from_slice(&c.offset.to_le_bytes());
        e[23..25].copy_from_slice(&c.len.to_le_bytes());
        at += 25;
    }
    for c in cols {
        let u: Vec<u8> = c.name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        p[at..at + 2].copy_from_slice(&(u.len() as u16).to_le_bytes());
        p[at + 2..at + 2 + u.len()].copy_from_slice(&u);
        at += 2 + u.len();
    }
    p
}

fn row(cols: &[Col], vals: &[V]) -> Vec<u8> {
    let fixed_len = cols
        .iter()
        .filter(|c| c.fixed)
        .map(|c| usize::from(c.offset + c.len))
        .max()
        .unwrap_or(0);
    let mut r = vec![0u8; 2 + fixed_len];
    r[0..2].copy_from_slice(&(cols.len() as u16).to_le_bytes());
    let mut mask = vec![0u8; cols.len().div_ceil(8)];
    let nvar = cols.iter().filter(|c| !c.fixed).count();
    let mut var_off = vec![0u16; nvar + 1];
    for (i, (c, v)) in cols.iter().zip(vals).enumerate() {
        if !matches!(v, V::Null) {
            mask[i / 8] |= 1 << (i % 8);
        }
        let bytes: Vec<u8> = match v {
            V::Long(x) => x.to_le_bytes().to_vec(),
            V::Int(x) => x.to_le_bytes().to_vec(),
            V::Double(x) => x.to_le_bytes().to_vec(),
            V::Single(x) => x.to_le_bytes().to_vec(),
            V::Text(s) => {
                // compressed text: FF FE then one byte per character
                let mut b = vec![0xFF, 0xFE];
                b.extend(s.bytes());
                b
            }
            V::Null => Vec::new(),
        };
        if c.fixed {
            let s = 2 + usize::from(c.offset);
            r[s..s + bytes.len()].copy_from_slice(&bytes);
        } else {
            var_off[usize::from(c.var)] = r.len() as u16;
            r.extend(&bytes);
        }
    }
    if nvar > 0 {
        // slots of null variable columns start where the next begins
        let end = r.len() as u16;
        var_off[nvar] = end;
        for k in (0..nvar).rev() {
            if var_off[k] == 0 {
                var_off[k] = var_off[k + 1];
            }
        }
        for k in (0..=nvar).rev() {
            r.extend(var_off[k].to_le_bytes());
        }
        r.extend((nvar as u16).to_le_bytes());
    }
    r.extend(mask);
    r
}

fn data_page(owner: u32, rows: &[Vec<u8>]) -> Vec<u8> {
    let mut p = vec![0u8; PAGE];
    p[0] = 1;
    p[1] = 1;
    p[4..8].copy_from_slice(&owner.to_le_bytes());
    p[12..14].copy_from_slice(&(rows.len() as u16).to_le_bytes());
    let mut end = PAGE;
    for (i, r) in rows.iter().enumerate() {
        let start = end - r.len();
        p[start..end].copy_from_slice(r);
        p[14 + 2 * i..16 + 2 * i].copy_from_slice(&(start as u16).to_le_bytes());
        end = start;
    }
    p
}

fn database() -> Vec<u8> {
    let mut head = vec![0u8; PAGE];
    head[..4].copy_from_slice(&[0, 1, 0, 0]);
    head[4..19].copy_from_slice(b"Standard Jet DB");
    head[0x14] = 1;
    // page 2: the catalog
    let cat = [
        col("Id", 4, Some(0), 0),
        col("Name", 10, None, 0),
        col("Type", 3, Some(4), 0),
        col("Flags", 4, Some(6), 0),
    ];
    let cat_rows = [
        row(
            &cat,
            &[V::Long(4), V::Text("Global_Table"), V::Int(1), V::Long(0)],
        ),
        row(
            &cat,
            &[
                V::Long(6),
                V::Text("Channel_Normal_Table"),
                V::Int(1),
                V::Long(0),
            ],
        ),
        row(
            &cat,
            &[
                V::Long(8),
                V::Text("Auxiliary_Table"),
                V::Int(1),
                V::Long(0),
            ],
        ),
        row(
            &cat,
            &[
                V::Long(2),
                V::Text("MSysObjects"),
                V::Int(1),
                V::Long(-2_147_483_648),
            ],
        ),
    ];
    let global = [
        col("Test_ID", 4, Some(0), 0),
        col("Test_Name", 10, None, 0),
        col("Start_DateTime", 7, Some(4), 0),
        col("Creator", 10, None, 1),
        col("MASS", 6, Some(12), 0),
    ];
    let g_rows = [row(
        &global,
        &[
            V::Long(1),
            V::Text("cell-07"),
            V::Double(42_587.5),
            V::Null,
            V::Single(0.5),
        ],
    )];
    let normal = [
        col("Test_ID", 4, Some(0), 0),
        col("Data_Point", 4, Some(4), 0),
        col("Test_Time", 7, Some(8), 0),
        col("Current", 6, Some(16), 0),
        col("Voltage", 6, Some(20), 0),
        col("Cycle_Index", 3, Some(24), 0),
    ];
    // stored out of order: points 2, 1, 3
    let pt = |dp: i32, t: f64, i: f32, v: f32| {
        row(
            &normal,
            &[
                V::Long(1),
                V::Long(dp),
                V::Double(t),
                V::Single(i),
                V::Single(v),
                V::Int(1),
            ],
        )
    };
    let n_rows = [
        pt(2, 1.0, 0.1, 3.5),
        pt(1, 0.0, 0.0, 3.0),
        pt(3, 2.0, -0.1, 3.25),
    ];
    let aux = [
        col("Test_ID", 4, Some(0), 0),
        col("Data_Point", 4, Some(4), 0),
        col("Auxiliary_Index", 3, Some(8), 0),
        col("Data_Type", 3, Some(10), 0),
        col("X", 6, Some(12), 0),
    ];
    let a_rows = [
        row(
            &aux,
            &[
                V::Long(1),
                V::Long(1),
                V::Int(0),
                V::Int(1),
                V::Single(25.0),
            ],
        ),
        row(
            &aux,
            &[
                V::Long(1),
                V::Long(3),
                V::Int(0),
                V::Int(1),
                V::Single(26.0),
            ],
        ),
    ];
    let mut b = head;
    b.extend(vec![0u8; PAGE]); // page 1
    b.extend(tdef(&cat, 4)); // 2
    b.extend(data_page(2, &cat_rows)); // 3
    b.extend(tdef(&global, 1)); // 4
    b.extend(data_page(4, &g_rows)); // 5
    b.extend(tdef(&normal, 3)); // 6
    b.extend(data_page(6, &n_rows)); // 7
    b.extend(tdef(&aux, 2)); // 8
    b.extend(data_page(8, &a_rows)); // 9
    b
}

fn open(b: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    ArbinReader.open_input(&Input::from_bytes("t.res", b))
}

#[test]
fn reads_a_test() {
    let b = database();
    let det = ArbinReader
        .sniff(&b[..64], std::path::Path::new("t.res"))
        .unwrap();
    assert_eq!(det.format_id, ARBIN_FORMAT_ID);
    assert!(
        ArbinReader
            .sniff(&b[..64], std::path::Path::new("t.mdb"))
            .is_none()
    );
    let mut ds = open(b).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<&str> = info.traces[0]
        .channels
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "time",
            "record",
            "current",
            "voltage",
            "cycle",
            "aux_temperature_1"
        ]
    );
    let tr = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(tr.channels[0], [0.0, 1.0, 2.0]);
    assert_eq!(tr.channels[1], [1.0, 2.0, 3.0]);
    assert_eq!(tr.channels[3], [3.0, 3.5, 3.25]);
    assert_eq!(tr.channels[5][0], 25.0);
    assert!(tr.channels[5][1].is_nan());
    assert_eq!(tr.channels[5][2], 26.0);
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.unwrap().name.as_deref(), Some("cell-07"));
    let acq = e.acquisition.unwrap();
    assert_eq!(acq.started_at.as_deref(), Some("2016-08-05T12:00:00"));
    assert!(acq.operator.is_none());
}

#[test]
fn damage_is_a_clean_error() {
    let b = database();
    // truncations anywhere: never a panic
    for cut in [
        100,
        PAGE,
        2 * PAGE + 10,
        3 * PAGE,
        7 * PAGE + 5,
        b.len() - 1,
    ] {
        let _ = open(b[..cut].to_vec());
    }
    // a row start pointing past the page
    let mut bad = b.clone();
    bad[7 * PAGE + 14..7 * PAGE + 16].copy_from_slice(&0x1FFFu16.to_le_bytes());
    assert!(matches!(open(bad), Err(Error::Corrupt { .. })));
    // a table definition that is not one
    let mut bad = b.clone();
    bad[6 * PAGE] = 9;
    assert!(open(bad).is_err());
    // Jet 3: refused
    let mut bad = b.clone();
    bad[0x14] = 0;
    assert!(matches!(open(bad), Err(Error::Unsupported { .. })));
    // an encrypted catalog
    let mut bad = b.clone();
    bad[2 * PAGE] = 0x5A;
    assert!(matches!(open(bad), Err(Error::Unsupported { .. })));
    // a Jet database without Arbin's tables
    let mut bad = b;
    let k = (3 * PAGE..4 * PAGE)
        .find(|&i| bad[i..].starts_with(b"Global_Table"))
        .unwrap();
    bad[k..k + 12].copy_from_slice(b"Other__Table");
    assert!(matches!(open(bad), Err(Error::Unsupported { .. })));
    // every byte flipped in turn in the data pages: an answer or an error
    let base = database();
    for off in (2 * PAGE..base.len()).step_by(37) {
        let mut m = base.clone();
        m[off] ^= 0xA5;
        let _ = open(m).and_then(|mut d| {
            let n = d.info()?.traces.len();
            for t in 0..n {
                d.read_trace(t as u32, 0, 0, u64::MAX)?;
            }
            Ok(())
        });
    }
}

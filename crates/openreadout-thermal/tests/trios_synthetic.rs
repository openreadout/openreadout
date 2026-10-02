//! Synthetic TA TRIOS `.tri` files built from the layout in `docs/formats/ta-trios.md`: a
//! header, one procedure step with its name property and three signals (one with a flag
//! array), parallel-plate constants for the moduli; damage ends in clean errors, never a panic.
#![allow(clippy::float_cmp)] // exact synthetic values

use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};
use openreadout_thermal::{TRIOS_FORMAT_ID, TriosReader};

/// A GUID in the stored (.NET) byte order from its text form.
fn guid(s: &str) -> [u8; 16] {
    let h: Vec<u8> = s
        .replace('-', "")
        .as_bytes()
        .chunks(2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect();
    [
        h[3], h[2], h[1], h[0], h[5], h[4], h[7], h[6], h[8], h[9], h[10], h[11], h[12], h[13],
        h[14], h[15],
    ]
}

fn string(s: &str) -> Vec<u8> {
    let mut v = vec![u8::try_from(s.len()).unwrap()];
    v.extend(s.as_bytes());
    v
}

fn object(id: &[u8; 16], payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0x21, 0x06];
    v.extend(u32::try_from(16 + payload.len()).unwrap().to_le_bytes());
    v.extend(id);
    v.extend(payload);
    v
}

fn property(key: &str, typ: u32, value: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(guid(key));
    body.extend([0u8; 32]);
    body.extend([0x01, 0x07, 0x20, 0x01, 0x10, 0, 0, 0, 0, 0, 0, 0]);
    body.extend([0x06, 0x20, 0x01]);
    body.extend(u32::try_from(8 + value.len()).unwrap().to_le_bytes());
    body.extend(typ.to_le_bytes());
    body.extend(1u32.to_le_bytes());
    body.extend(value);
    body.extend([0x01, 0x00, 0x01, 0x00]);
    let mut v = vec![0x20, 0x04];
    v.extend(u32::try_from(16 + body.len()).unwrap().to_le_bytes());
    v.extend(guid(key));
    v.extend(body);
    v
}

fn signal(id: &str, values: &[f32], flags: Option<&[u32]>) -> Vec<u8> {
    let n = u32::try_from(values.len()).unwrap();
    let mut p = Vec::new();
    p.extend(1u32.to_le_bytes());
    p.extend(1u32.to_le_bytes());
    p.extend([0x00, 0xF2, 0x21, 0x01, 4, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x00]);
    p.extend(n.to_le_bytes());
    p.extend([0x01, 0x10, 0x21, 0x01]);
    if let Some(f) = flags {
        p.extend(u32::try_from(4 + 4 * f.len()).unwrap().to_le_bytes());
        p.extend(u32::try_from(f.len()).unwrap().to_le_bytes());
        for x in f {
            p.extend(x.to_le_bytes());
        }
    } else {
        p.extend(4u32.to_le_bytes());
        p.extend(0u32.to_le_bytes());
    }
    p.extend([0x01, 0x00]);
    p.extend(n.to_le_bytes());
    for v in values {
        p.extend(v.to_le_bytes());
    }
    p.extend([0, 0, 0]);
    object(&guid(id), &p)
}

const TIME: &str = "7b1a8875-39e5-4993-ae1d-67210ee57fb7";
const TEMP: &str = "f79c919e-6fa6-4856-92f8-6e5868f792f5";
const TORQUE: &str = "360672f5-f937-414b-bd06-5b9d80d6975e";
const UNKNOWN: &str = "5f7b2d27-8360-4bc6-be74-eff29aa2fe74";

fn file() -> Vec<u8> {
    let mut entries = Vec::new();
    for (k, v) in [
        ("instrumenttype", "Discovery HR-2"),
        ("instrumentserialnumber", "5332-0000"),
        ("operator", "tester"),
        ("samplename", "gel"),
        ("ticks", "638603673566287206"),
        ("proceduresegments", "Time sweep"),
        ("holder", "20mm cone, Peltier plate"),
    ] {
        entries.extend(string(k));
        entries.extend(string(v));
    }
    let mut b = vec![
        0x00, 0x25, 0x0E, 0, 0, 0, 0, 0x08, 0x25, 0x02, 0, 0, 0, 0, 0x14, 0x20, 0x01,
    ];
    b.extend(u32::try_from(entries.len() + 4).unwrap().to_le_bytes());
    b.extend(7u32.to_le_bytes());
    b.extend(entries);
    b.extend([0x01, 0x00, 0x01, 4, 0, 0, 0, 0x89, b'P', b'N', b'G']);
    // the step: 16 zero bytes, the step prefix, its name, three signals
    let mut step = vec![0u8; 16];
    step.extend([
        0xF2, 0x21, 0x01, 0x04, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x00, 0x11, 0x20, 0x02,
    ]);
    step.extend([0u8; 12]);
    let mut name = string("Time sweep - 1");
    name.push(0);
    step.extend(property("d467abca-26c2-4afa-8ce7-23e95909709d", 4, &name));
    step.extend(signal(TEMP, &[4.0, 4.5, 5.0], None));
    step.extend(signal(TIME, &[1.0, 2.0, 3.0], None));
    step.extend(signal(TORQUE, &[1e-6, 2e-6, 3e-6], Some(&[0, 1, 0])));
    step.extend(signal(UNKNOWN, &[0.5, 0.25, 0.125], None));
    step.extend([0u8; 3]);
    b.extend(object(&guid("321daac2-2636-4928-9a48-84f72c21edee"), &step));
    b
}

fn open(bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    TriosReader.open_input(&Input::from_bytes("run.tri", bytes))
}

#[test]
fn reads_a_step() {
    let bytes = file();
    let det = TriosReader
        .sniff(&bytes, std::path::Path::new("x.bin"))
        .expect("sniffed");
    assert_eq!(det.format_id, TRIOS_FORMAT_ID);
    let mut ds = open(bytes).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("Time sweep - 1"));
    let names: Vec<&str> = t.channels.iter().map(|c| c.name.as_str()).collect();
    // the time axis first, then the stored order; a cone is not a parallel plate: no moduli
    assert_eq!(
        names,
        [
            "step_time",
            "temperature",
            "oscillation_torque",
            "signal_5f7b2d27"
        ]
    );
    assert_eq!(t.channels[2].unit.as_deref(), Some("N·m"));
    assert_eq!(t.channels[3].unit, None);
    let tr = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(tr.channels[0], [1.0, 2.0, 3.0]);
    assert_eq!(tr.channels[1], [4.0, 4.5, 5.0]);
    assert_eq!(
        tr.channels[2],
        [
            f64::from(1e-6_f32),
            f64::from(2e-6_f32),
            f64::from(3e-6_f32)
        ]
    );
    let exp = ds.experiment().unwrap();
    let ins = exp.instrument.unwrap();
    assert_eq!(ins.model.as_deref(), Some("Discovery HR-2"));
    assert_eq!(ins.serial.as_deref(), Some("5332-0000"));
    assert_eq!(
        exp.acquisition.unwrap().started_at.as_deref(),
        Some("2024-08-27T14:55:56.628")
    );
}

#[test]
fn damage_is_a_clean_error() {
    let good = file();
    for cut in [30, 60, good.len() / 2, good.len() - 10] {
        let r = open(good[..cut].to_vec());
        match r {
            Ok(mut ds) => {
                let _ = ds.info();
                let _ = ds.read_trace(0, 0, 0, 10);
            }
            Err(e) => assert!(
                matches!(e, Error::Corrupt { .. } | Error::Unsupported { .. }),
                "{e}"
            ),
        }
    }
    // every byte flipped once in the first 400 bytes: never a panic
    for i in 0..good.len().min(400) {
        let mut b = good.clone();
        b[i] ^= 0xA5;
        if let Ok(mut ds) = open(b) {
            let _ = ds.info();
            let _ = ds.read_trace(0, 0, 0, 10);
        }
    }
    // an older generation without signal records is refused
    let mut old = good.clone();
    old[2] = 0x0C;
    let cut = old.len() - 200;
    old.truncate(cut);
    assert!(matches!(
        open(old),
        Err(Error::Unsupported { .. } | Error::Corrupt { .. })
    ));
}

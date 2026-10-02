//! Print what the UNICORN readers see in a file: `cargo run -p openreadout-fplc --example
//! fplc_dump -- FILE [--points]`.

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_fplc::{UnicornResReader, UnicornZipReader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = Path::new(args.first().ok_or("usage: fplc_dump FILE [--points]")?);
    let points = args.iter().any(|a| a == "--points");
    let mut ds = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("res"))
    {
        UnicornResReader.open(path)?
    } else {
        UnicornZipReader.open(path)?
    };
    let info = ds.info()?;
    println!("{}", serde_json::to_string_pretty(&info)?);
    println!("{}", serde_json::to_string_pretty(&ds.experiment())?);
    if points {
        for t in &info.traces {
            let tr = ds.read_trace(t.index, 0, 0, u64::MAX)?;
            let y = &tr.channels[0];
            let v = &tr.channels[1];
            let (imax, ymax) =
                y.iter().enumerate().fold(
                    (0, f64::NEG_INFINITY),
                    |a, (i, &x)| if x > a.1 { (i, x) } else { a },
                );
            println!(
                "{:3} {:32} n={:7} first=({:?},{:?}) last=({:?},{:?}) max {ymax} at vol {:?}",
                t.index,
                t.name.as_deref().unwrap_or(""),
                y.len(),
                v.first(),
                y.first(),
                v.last(),
                y.last(),
                v.get(imax)
            );
        }
        for tb in &info.tables {
            let t = ds.read_table(tb.index, 0, 5)?;
            println!(
                "table {} {:?} rows={} first={:?}",
                tb.index,
                tb.name,
                tb.row_count,
                t.columns.iter().map(|c| c.first()).collect::<Vec<_>>()
            );
        }
        println!("{}", serde_json::to_string_pretty(&ds.check()?)?);
    }
    Ok(())
}

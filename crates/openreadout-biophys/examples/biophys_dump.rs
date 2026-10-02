//! Print what the biophysics readers see: `cargo run -p openreadout-biophys --example biophys_dump -- FILE`.

use std::path::Path;

use openreadout_core::Registry;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: biophys_dump FILE")?;
    let reg = Registry::new()
        .with(Box::new(openreadout_biophys::ItcReader))
        .with(Box::new(openreadout_biophys::BiacoreReader));
    let (_, mut ds) = reg.open(Path::new(&path))?;
    let info = ds.info()?;
    for t in info.traces.iter().take(6) {
        let tr = ds.read_trace(t.index, 0, 0, 3)?;
        println!(
            "trace {} {:?} n={} rate={} start={:?} ch={:?} first={:?}",
            t.index,
            t.name,
            t.sample_count,
            t.sample_rate_hz,
            t.start_s,
            t.channels
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
            tr.channels
                .iter()
                .map(|c| c.first().copied())
                .collect::<Vec<_>>()
        );
    }
    for tb in &info.tables {
        let t = ds.read_table(tb.index, 0, 3)?;
        println!(
            "table {} {:?} rows={} first={:?}",
            tb.index, tb.name, tb.row_count, t.columns
        );
    }
    let e = ds.experiment();
    println!("{}", serde_json::to_string(&e)?);
    let c = ds.check()?;
    println!(
        "check ok={} {:?}",
        c.ok,
        c.findings
            .iter()
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
    );
    Ok(())
}

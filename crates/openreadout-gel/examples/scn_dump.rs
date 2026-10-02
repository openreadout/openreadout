//! Print what the Image Lab reader sees: `cargo run -p openreadout-gel --example scn_dump -- FILE [--full]`.

use std::path::Path;

use openreadout_core::{FormatReader, PlaneIndex};
use openreadout_gel::ImageLabReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = Path::new(args.first().ok_or("usage: scn_dump FILE [--full]")?);
    let mut ds = ImageLabReader.open(path)?;
    let info = ds.info()?;
    if args.iter().any(|a| a == "--full") {
        println!("{}", serde_json::to_string_pretty(&info)?);
        println!("{}", serde_json::to_string_pretty(&ds.experiment())?);
    }
    for im in &info.images {
        let p = ds.read_plane(im.index, PlaneIndex::default())?;
        let v: Vec<u16> = p
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        let (mn, mx) = v
            .iter()
            .fold((u16::MAX, 0), |a, &x| (a.0.min(x), a.1.max(x)));
        let mean = v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64;
        println!(
            "{} {}x{} {:?} px={:?}um ch={:?} exp_ms={:?} em={:?} min={mn} max={mx} mean={mean:.2}",
            path.file_name().unwrap().to_string_lossy(),
            im.size_x,
            im.size_y,
            im.pixel_type,
            (im.physical_size.x, im.physical_size.y),
            im.channels[0].name,
            im.channels[0].exposure_ms,
            im.channels[0].emission_nm
        );
    }
    let c = ds.check()?;
    if !c.ok {
        println!("check: {:?}", c.findings);
    }
    Ok(())
}

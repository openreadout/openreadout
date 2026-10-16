//! Development helper: `dump_plane FILE [PLANE]` writes `<type> <w> <h> <spp>` and a newline,
//! then the raw samples of plane PLANE (z of image 0, or t when the image has one z).
use std::io::Write;
use std::path::Path;

use openreadout_core::PlaneIndex;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: u32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    let reg = openreadout_core::Registry::new()
        .with(Box::new(openreadout_medical::NiftiReader))
        .with(Box::new(openreadout_medical::DicomReader));
    let (_, mut ds) = reg.open(Path::new(&args[0])).expect("opens");
    let info = ds.info().expect("info");
    let im = &info.images[0];
    let idx = if im.size_z > 1 {
        PlaneIndex { c: 0, z: n, t: 0 }
    } else {
        PlaneIndex { c: 0, z: 0, t: n }
    };
    let p = ds.read_plane(0, idx).expect("plane");
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "{} {} {} {}",
        p.pixel_type.ome_name(),
        p.width,
        p.height,
        p.samples_per_pixel
    )
    .unwrap();
    out.write_all(&p.data).unwrap();
}

//! Temporary: print what the readers make of files named in SEQ_FILES.
use openreadout_core::FormatReader;

#[test]
fn print_files() {
    let Ok(files) = std::env::var("SEQ_FILES") else {
        return;
    };
    for f in files.split(',') {
        let p = std::path::Path::new(f);
        let r: Box<dyn FormatReader> = if f.ends_with(".pod5") {
            Box::new(openreadout_seq::Pod5Reader)
        } else if f.ends_with(".fast5") || f.ends_with("single-read") {
            Box::new(openreadout_seq::Fast5Reader)
        } else if f.ends_with(".bam") {
            Box::new(openreadout_seq::BamReader)
        } else if f.contains(".fastq") || f.contains(".fq") {
            Box::new(openreadout_seq::FastqReader)
        } else {
            Box::new(openreadout_seq::IlluminaRunReader)
        };
        let mut ds = r.open(p).unwrap();
        let info = ds.info().unwrap();
        let mut t0 = info.tables[0].clone();
        t0.columns.clear();
        println!("== {f}\n{}", serde_json::to_string_pretty(&t0).unwrap());
        let t = ds.read_table(0, 0, 6).unwrap();
        for (c, col) in info.tables[0].columns.iter().zip(&t.columns) {
            println!("{:>20} {:?}", c.name, col);
        }
        println!("notes {:?} version {:?}", info.notes, info.format_version);
        for tr in &info.traces {
            let mut x = tr.clone();
            x.extra.remove("sweep_sample_counts");
            println!("trace {}", serde_json::to_string(&x).unwrap());
            let s = ds.read_trace(tr.index, 0, 0, 8).unwrap();
            println!("  first samples {:?}", s.channels.iter().map(|c| c.iter().take(8).collect::<Vec<_>>()).collect::<Vec<_>>());
        }
        let e = ds.experiment();
        println!("{}", serde_json::to_string(&e).unwrap());
        let c = ds.check().unwrap();
        println!("check ok={} {:?}", c.ok, c.findings);
    }
}

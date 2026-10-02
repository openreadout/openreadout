//! netCDF-3 (classic and 64-bit offset) header parser of the ANDI reader, then every
//! variable's shape, extent and first values and every attribute as JSON.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let Ok(nc) = openreadout_chrom::NetCdf::parse(data) else {
        return;
    };
    for a in &nc.attributes {
        let _ = a.value.to_json();
    }
    let path = openreadout_fuzz::scratch_file("chrom_netcdf", "cdf", data);
    let Ok(mut f) = openreadout_core::source::SourceFile::open_local(&path) else {
        return;
    };
    for v in &nc.variables {
        let _ = nc.shape(v);
        let n = nc.element_count(v);
        let _ = nc.data_end(v);
        for a in &v.attributes {
            let _ = a.value.to_json();
        }
        let _ = nc.read_f64(&mut f, &path, v, 0, n.min(4096));
        let _ = nc.read_f64(&mut f, &path, v, n.saturating_sub(1), 1);
        let _ = nc.read_strings(&mut f, &path, v);
    }
});

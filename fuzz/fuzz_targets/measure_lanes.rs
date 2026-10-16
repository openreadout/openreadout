//! Lanes and bands of a gel image (`stats --lanes`): the first byte sets the width (8 to 263 pixels), the rest are 8-bit rows; the second byte picks options (lane count, polarity, a ladder on lane 1).
#![no_main]

use libfuzzer_sys::fuzz_target;
use openreadout_core::measure::{LadderRequest, LanesRequest, Polarity, lanes_of_bytes};

fuzz_target!(|data: &[u8]| {
    let [w, opt, rest @ ..] = data else { return };
    let mut req = LanesRequest::default();
    if opt & 1 != 0 {
        req.count = Some(u32::from(opt >> 4));
    }
    req.polarity = match (opt >> 1) & 3 {
        1 => Polarity::Bright,
        2 => Polarity::Dark,
        _ => Polarity::Auto,
    };
    if opt & 8 != 0 {
        req.ladder = Some(LadderRequest {
            lane: 1,
            sizes: vec![250.0, 100.0, 50.0, 25.0],
            unit: None,
        });
    }
    req.profiles = opt & 0x80 != 0;
    let _ = lanes_of_bytes(u32::from(*w) + 8, rest, &req);
});

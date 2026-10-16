//! Size standards (ladders) for fragment sizing: the fragment lengths each product contains, as
//! its maker publishes them on the product page (checked 2026-10-15; see
//! `docs/provenance/abif.md`).

/// A size standard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ladder {
    /// Our short name (`GS500LIZ`).
    pub name: &'static str,
    /// The product name.
    pub product: &'static str,
    /// The dye the fragments carry (`LIZ`, `ROX`).
    pub dye: &'static str,
    /// Fragment lengths, nucleotides, ascending.
    pub sizes: &'static [f64],
}

/// The size standards the reader knows.
pub const LADDERS: &[Ladder] = &[
    Ladder {
        name: "GS500LIZ",
        product: "GeneScan 500 LIZ",
        dye: "LIZ",
        sizes: &[
            35.0, 50.0, 75.0, 100.0, 139.0, 150.0, 160.0, 200.0, 250.0, 300.0, 340.0, 350.0, 400.0,
            450.0, 490.0, 500.0,
        ],
    },
    Ladder {
        name: "GS600LIZ",
        product: "GeneScan 600 LIZ v2.0",
        dye: "LIZ",
        sizes: &[
            20.0, 40.0, 60.0, 80.0, 100.0, 114.0, 120.0, 140.0, 160.0, 180.0, 200.0, 214.0, 220.0,
            240.0, 250.0, 260.0, 280.0, 300.0, 314.0, 320.0, 340.0, 360.0, 380.0, 400.0, 414.0,
            420.0, 440.0, 460.0, 480.0, 500.0, 514.0, 520.0, 540.0, 560.0, 580.0, 600.0,
        ],
    },
    Ladder {
        name: "GS1200LIZ",
        product: "GeneScan 1200 LIZ",
        dye: "LIZ",
        sizes: &[
            20.0, 30.0, 40.0, 60.0, 80.0, 100.0, 114.0, 120.0, 140.0, 160.0, 180.0, 200.0, 214.0,
            220.0, 240.0, 250.0, 260.0, 280.0, 300.0, 314.0, 320.0, 340.0, 360.0, 380.0, 400.0,
            414.0, 420.0, 440.0, 460.0, 480.0, 500.0, 514.0, 520.0, 540.0, 560.0, 580.0, 600.0,
            614.0, 620.0, 640.0, 660.0, 680.0, 700.0, 714.0, 720.0, 740.0, 760.0, 780.0, 800.0,
            820.0, 840.0, 850.0, 860.0, 880.0, 900.0, 920.0, 940.0, 960.0, 980.0, 1000.0, 1020.0,
            1040.0, 1060.0, 1080.0, 1100.0, 1120.0, 1160.0, 1200.0,
        ],
    },
    Ladder {
        name: "GS120LIZ",
        product: "GeneScan 120 LIZ",
        dye: "LIZ",
        sizes: &[15.0, 20.0, 25.0, 35.0, 50.0, 62.0, 80.0, 110.0, 120.0],
    },
    Ladder {
        name: "GS500ROX",
        product: "GeneScan 500 ROX",
        dye: "ROX",
        sizes: &[
            35.0, 50.0, 75.0, 100.0, 139.0, 150.0, 160.0, 200.0, 250.0, 300.0, 340.0, 350.0, 400.0,
            450.0, 490.0, 500.0,
        ],
    },
    Ladder {
        name: "GS400HDROX",
        product: "GeneScan 400HD ROX",
        dye: "ROX",
        sizes: &[
            50.0, 60.0, 90.0, 100.0, 120.0, 150.0, 160.0, 180.0, 190.0, 200.0, 220.0, 240.0, 260.0,
            280.0, 290.0, 300.0, 320.0, 340.0, 360.0, 380.0, 400.0,
        ],
    },
    Ladder {
        name: "GS350ROX",
        product: "GeneScan 350 ROX",
        dye: "ROX",
        sizes: &[
            35.0, 50.0, 75.0, 100.0, 139.0, 150.0, 160.0, 200.0, 250.0, 300.0, 340.0, 350.0,
        ],
    },
];

/// The ladder a size-standard name or file name refers to (`GS500LIZ.xml`, `GS600_LIZ_(60-460)`,
/// `GeneScan 500 ROX`), if any.
pub fn named(text: &str) -> Option<&'static Ladder> {
    let t: String = text
        .to_ascii_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let t = t.replace("GENESCAN", "GS");
    // the longest name that matches, so GS1200 is not read as GS120
    LADDERS
        .iter()
        .filter(|l| {
            let n = l.name.to_ascii_uppercase();
            let (num, dye) = n.split_at(n.len() - 3);
            t.contains(&n) || (t.contains(num) && t.contains(dye))
        })
        .max_by_key(|l| l.name.len())
}

/// The ladders whose dye is `dye` (by name: `LIZ`, `ROX`, `GS LIZ`), or every ladder when the
/// dye names neither.
pub fn for_dye(dye: &str) -> Vec<&'static Ladder> {
    let d = dye.to_ascii_uppercase();
    let hits: Vec<&Ladder> = LADDERS.iter().filter(|l| d.contains(l.dye)).collect();
    if hits.is_empty() {
        LADDERS.iter().collect()
    } else {
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladders_are_ascending_and_named() {
        for l in LADDERS {
            assert!(l.sizes.windows(2).all(|w| w[0] < w[1]), "{}", l.name);
        }
        assert_eq!(named("GS500LIZ.xml").unwrap().name, "GS500LIZ");
        assert_eq!(named("GS1200LIZ").unwrap().name, "GS1200LIZ");
        assert_eq!(named("GeneScan 500 ROX").unwrap().name, "GS500ROX");
        assert_eq!(named("GS600_LIZ_(60-460)").unwrap().name, "GS600LIZ");
        assert!(named("my ladder").is_none());
        assert_eq!(for_dye("ROX").len(), 3);
        assert_eq!(for_dye("FAM").len(), LADDERS.len());
    }
}

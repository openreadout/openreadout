//! Adaptive variable-length codes (jxrlib `adapthuff.c`, T.832 clause 9.3 "adaptive VLC tables").
//!
//! Each adaptive table switches between a few fixed code tables driven by a discriminant that the
//! decoder updates after every symbol. Decode tables hold `(symbol << 3) | length` for codes up to
//! 5 bits; negative entries are nodes of a binary tree for longer codes (index `entry + 32768`).

use crate::bits::BitReader;
use crate::{Error, Result};

const ROOT_BITS: u32 = 5;
const ROOT_BITS_LOG: u32 = 3;

static G4_DEC: [i16; 40] = [
    19, 19, 19, 19, 27, 27, 27, 27, 10, 10, 10, 10, 10, 10, 10, 10, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0,
];

static G5_DEC: [[i16; 42]; 2] = [
    [
        28, 28, 36, 36, 19, 19, 19, 19, 10, 10, 10, 10, 10, 10, 10, 10, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        11, 11, 11, 11, 19, 19, 19, 19, 27, 27, 27, 27, 35, 35, 35, 35, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
];

static G6_DEC: [[i16; 44]; 4] = [
    [
        13, 29, 44, 44, 19, 19, 19, 19, 34, 34, 34, 34, 34, 34, 34, 34, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        12, 12, 28, 28, 43, 43, 43, 43, 2, 2, 2, 2, 2, 2, 2, 2, 18, 18, 18, 18, 18, 18, 18, 18, 34,
        34, 34, 34, 34, 34, 34, 34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        4, 4, 12, 12, 43, 43, 43, 43, 18, 18, 18, 18, 18, 18, 18, 18, 26, 26, 26, 26, 26, 26, 26,
        26, 34, 34, 34, 34, 34, 34, 34, 34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        5, 13, 36, 36, 43, 43, 43, 43, 18, 18, 18, 18, 18, 18, 18, 18, 25, 25, 25, 25, 25, 25, 25,
        25, 25, 25, 25, 25, 25, 25, 25, 25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
];

static G7_DEC: [[i16; 46]; 2] = [
    [
        45, 53, 36, 36, 27, 27, 27, 27, 2, 2, 2, 2, 2, 2, 2, 2, 10, 10, 10, 10, 10, 10, 10, 10, 18,
        18, 18, 18, 18, 18, 18, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        -32736, 37, 28, 28, 19, 19, 19, 19, 10, 10, 10, 10, 10, 10, 10, 10, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, 5, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
];

static G8_DEC: [[i16; 48]; 2] = [
    [
        53, 21, 28, 28, 11, 11, 11, 11, 43, 43, 43, 43, 59, 59, 59, 59, 2, 2, 2, 2, 2, 2, 2, 2, 34,
        34, 34, 34, 34, 34, 34, 34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        52, 52, 20, 20, 3, 3, 3, 3, 11, 11, 11, 11, 27, 27, 27, 27, 35, 35, 35, 35, 43, 43, 43, 43,
        58, 58, 58, 58, 58, 58, 58, 58, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
];

static G9_DEC: [[i16; 50]; 2] = [
    [
        13, 29, 37, 61, 20, 20, 68, 68, 3, 3, 3, 3, 51, 51, 51, 51, 41, 41, 41, 41, 41, 41, 41, 41,
        41, 41, 41, 41, 41, 41, 41, 41, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        -32736, 53, 28, 28, 11, 11, 11, 11, 19, 19, 19, 19, 43, 43, 43, 43, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, -32734, 4, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
];

static G12_DEC: [[i16; 56]; 5] = [
    [
        -32736, 5, 76, 76, 37, 53, 69, 85, 43, 43, 43, 43, 91, 91, 91, 91, 57, 57, 57, 57, 57, 57,
        57, 57, 57, 57, 57, 57, 57, 57, 57, 57, -32734, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        -32736, 85, 13, 53, 4, 4, 36, 36, 43, 43, 43, 43, 67, 67, 67, 67, 75, 75, 75, 75, 91, 91,
        91, 91, 58, 58, 58, 58, 58, 58, 58, 58, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0,
    ],
    [
        -32736, 37, 92, 92, 11, 11, 11, 11, 43, 43, 43, 43, 59, 59, 59, 59, 67, 67, 67, 67, 75, 75,
        75, 75, 2, 2, 2, 2, 2, 2, 2, 2, -32734, -32732, 2, 3, 6, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        -32736, 29, 37, 69, 3, 3, 3, 3, 43, 43, 43, 43, 59, 59, 59, 59, 75, 75, 75, 75, 91, 91, 91,
        91, 10, 10, 10, 10, 10, 10, 10, 10, -32734, 10, 2, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        -32736, 93, 28, 28, 60, 60, 76, 76, 3, 3, 3, 3, 43, 43, 43, 43, 9, 9, 9, 9, 9, 9, 9, 9, 9,
        9, 9, 9, 9, 9, 9, 9, -32734, -32732, -32730, 2, 4, 8, 6, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0,
    ],
];

static G5_DELTA: [i32; 5] = [0, -1, 0, 1, 1];
static G6_DELTA: [i32; 18] = [-1, 1, 1, 1, 0, 1, -2, 0, 0, 2, 0, 0, -1, -1, 0, 1, -2, 0];
static G7_DELTA: [i32; 7] = [1, 0, -1, -1, -1, -1, -1];
static G9_DELTA: [i32; 9] = [2, 2, 1, 1, -1, -2, -2, -2, -3];
static G12_DELTA: [i32; 48] = [
    1, 1, 1, 1, 1, 0, 0, -1, 2, 1, 0, 0, 2, 2, -1, -1, -1, 0, -2, -1, 0, 0, -2, -1, -1, 1, 0, 2, 0,
    0, 0, 0, -2, 0, 1, 1, 0, 1, 0, 1, -2, 0, -1, -1, -2, -1, -2, -2,
];

const THRESHOLD: i32 = 8;
const MEMORY: i32 = 8;
/// Code tables per alphabet size (jxrlib `gMaxTables`), indexed by symbol count.
const MAX_TABLES: [i32; 13] = [0, 0, 0, 0, 1, 2, 4, 2, 2, 2, 0, 0, 5];
/// Alphabets whose adaptation uses a second discriminant (jxrlib `gSecondDisc`).
const SECOND_DISC: [bool; 13] = [
    false, false, false, false, false, false, true, false, false, false, false, false, true,
];

/// One adaptive VLC table (jxrlib `CAdaptiveHuffman`).
#[derive(Debug, Clone)]
pub(crate) struct AdaptiveHuffman {
    symbols: usize,
    table_index: i32,
    dec: &'static [i16],
    delta: &'static [i32],
    delta1: &'static [i32],
    initialized: bool,
    pub(crate) disc: i32,
    pub(crate) disc1: i32,
    upper: i32,
    lower: i32,
}

impl AdaptiveHuffman {
    pub(crate) fn new(symbols: usize) -> Self {
        AdaptiveHuffman {
            symbols,
            table_index: 0,
            dec: &G4_DEC,
            delta: &[],
            delta1: &[],
            initialized: false,
            disc: 0,
            disc1: 0,
            upper: 0,
            lower: 0,
        }
    }

    /// Forget the adaptation state; the next `adapt` starts from the initial table.
    pub(crate) fn reset(&mut self) {
        self.initialized = false;
    }

    /// jxrlib `AdaptDiscriminant`: move to a neighbouring code table when the discriminant
    /// crossed a bound.
    pub(crate) fn adapt(&mut self) {
        let sym = self.symbols;
        if !self.initialized {
            self.initialized = true;
            self.disc = 0;
            self.disc1 = 0;
            self.table_index = i32::from(SECOND_DISC[sym]);
        }
        let d_low = self.disc;
        let d_high = if SECOND_DISC[sym] {
            self.disc1
        } else {
            self.disc
        };
        let mut change = false;
        if d_low < self.lower {
            self.table_index -= 1;
            change = true;
        } else if d_high > self.upper {
            self.table_index += 1;
            change = true;
        }
        if change {
            self.disc = 0;
            self.disc1 = 0;
        }
        self.disc = self.disc.clamp(-THRESHOLD * MEMORY, THRESHOLD * MEMORY);
        self.disc1 = self.disc1.clamp(-THRESHOLD * MEMORY, THRESHOLD * MEMORY);
        let max = MAX_TABLES[sym];
        // The bounds below keep the index in 0..max; clamp anyway so no input can index past.
        let t = self.table_index.clamp(0, (max - 1).max(0));
        self.table_index = t;
        self.lower = if t == 0 { i32::MIN } else { -THRESHOLD };
        self.upper = if t == max - 1 { 1 << 30 } else { THRESHOLD };
        let t = t as usize;
        let n = sym;
        // jxrlib: pDelta = table + (t - 1 + (t == 0)) * n, pDelta1 = table + n * (t - (t + 1 == max))
        let row = t.saturating_sub(1);
        let row1 = t.saturating_sub(usize::from(t as i32 + 1 == max));
        match sym {
            4 => {
                self.dec = &G4_DEC;
                self.delta = &[];
            }
            5 => {
                self.dec = &G5_DEC[t];
                self.delta = &G5_DELTA;
            }
            6 => {
                self.dec = &G6_DEC[t];
                self.delta1 = &G6_DELTA[n * row1..n * row1 + n];
                self.delta = &G6_DELTA[row * n..row * n + n];
            }
            7 => {
                self.dec = &G7_DEC[t];
                self.delta = &G7_DELTA;
            }
            8 => {
                self.dec = &G8_DEC[0];
                self.delta = &[];
            }
            9 => {
                self.dec = &G9_DEC[t];
                self.delta = &G9_DELTA;
            }
            12 => {
                self.dec = &G12_DEC[t];
                self.delta1 = &G12_DELTA[n * row1..n * row1 + n];
                self.delta = &G12_DELTA[row * n..row * n + n];
            }
            _ => {}
        }
    }

    /// Add the first discriminant's delta for `symbol` (a symbol the table just produced).
    #[inline]
    pub(crate) fn update(&mut self, symbol: i32) {
        if let Some(d) = self.delta.get(symbol as usize) {
            self.disc += d;
        }
    }

    /// Add both discriminants' deltas for `symbol`.
    #[inline]
    pub(crate) fn update2(&mut self, symbol: i32) {
        if let Some(d) = self.delta.get(symbol as usize) {
            self.disc += d;
        }
        if let Some(d) = self.delta1.get(symbol as usize) {
            self.disc1 += d;
        }
    }

    /// jxrlib `getHuff`: decode one symbol, following the tree for codes longer than 5 bits.
    #[inline]
    pub(crate) fn decode(&self, r: &mut BitReader<'_>) -> Result<i32> {
        let entry = self.entry(r.peek(ROOT_BITS) as usize)?;
        if entry >= 0 {
            r.flush(entry as u32 & ((1 << ROOT_BITS_LOG) - 1));
            return Ok(entry >> ROOT_BITS_LOG);
        }
        r.flush(ROOT_BITS);
        let mut node = entry;
        // A tree is at most a few levels deep; bound the walk anyway.
        for _ in 0..32 {
            let idx = node + 32768 + r.get(1) as i32;
            node = self.entry(idx as usize)?;
            if node >= 0 {
                return Ok(node);
            }
        }
        Err(Error::decode("invalid variable-length code"))
    }

    /// jxrlib `_getHuffShort`: tables whose codes all fit in 5 bits.
    #[inline]
    pub(crate) fn decode_short(&self, r: &mut BitReader<'_>) -> Result<i32> {
        let entry = self.entry(r.peek(ROOT_BITS) as usize)?;
        if entry < 0 {
            return Err(Error::decode("invalid short variable-length code"));
        }
        r.flush(entry as u32 & ((1 << ROOT_BITS_LOG) - 1));
        Ok(entry >> ROOT_BITS_LOG)
    }

    #[inline]
    fn entry(&self, i: usize) -> Result<i32> {
        self.dec
            .get(i)
            .map(|&v| i32::from(v))
            .ok_or_else(|| Error::decode("variable-length code table index out of range"))
    }
}

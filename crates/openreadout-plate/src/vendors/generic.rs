//! Any delimited text or workbook with plate matrices but no recognised exporter: a line of
//! consecutive column numbers followed by lines starting with increasing row letters. Each
//! matrix becomes one channel named after the nearest title line above it; the detection mode
//! is guessed from that title (`Absorbance`, `OD600`, `Luminescence`, …) and otherwise left
//! unknown. One table per sheet.

use crate::grid::find_grids;
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType, first_number};
use crate::sheet::Book;

/// True when the text holds at least one matrix with two or more rows and three or more
/// columns of numbers.
pub(crate) fn sniff(book: &Book) -> bool {
    book.sheets.iter().any(|s| {
        find_grids(s).iter().any(|g| {
            g.rows.len() >= 2
                && g.columns.len() >= 3
                && g.cells(s).filter(|(_, _, c)| c.number().is_some()).count() >= 4
        })
    })
}

pub(crate) fn parse(book: &Book) -> Export {
    let mut ex = Export::new(Kind::Generic, book.container.clone());
    for (si, s) in book.sheets.iter().enumerate() {
        let grids = super::titled_grids(s);
        if grids.is_empty() {
            continue;
        }
        let name = if s.name.is_empty() {
            format!("Plate {}", si + 1)
        } else {
            s.name.clone()
        };
        let mut b = Block::new(name.clone(), name);
        b.read_type = Some(ReadType::Endpoint);
        b.sheet = (!s.name.is_empty()).then(|| s.name.clone());
        b.line = Some(grids[0].grid.header_row + 1);
        for (i, t) in grids.iter().enumerate() {
            if !t.grid.cells(s).any(|(_, _, c)| c.number().is_some()) {
                continue; // a layout of names, not values
            }
            let title = if t.title().is_empty() {
                t.grid.corner.clone()
            } else {
                t.title().to_string()
            };
            let label = if title.is_empty() {
                format!("matrix {}", i + 1)
            } else {
                title.clone()
            };
            let mode = match Mode::from_text(&title) {
                Mode::Unknown => Mode::from_text(&t.grid.corner),
                m => m,
            };
            let mut ch = Channel::derived(label, mode, crate::model::ModeBasis::Label);
            if ch.mode == Mode::Absorbance {
                ch.wavelength_nm = first_number(&title);
            }
            let idx = b.channel(ch);
            super::push_grid(s, &t.grid, &mut b, idx, None);
        }
        if !b.obs.is_empty() {
            ex.blocks.push(b);
        }
    }
    ex.notes.push("no exporter signature recognised: plate matrices were read generically; detection modes are guessed from titles".into());
    ex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn generic_matrix() {
        let t = "My assay OD600\n\t1\t2\t3\nA\t0.1\t0.2\t0.3\nB\t0.4\t0.5\t0.6\n";
        let book = text_book(t.as_bytes());
        assert!(sniff(&book));
        let ex = parse(&book);
        let b = &ex.blocks[0];
        assert_eq!(b.obs.len(), 6);
        assert_eq!(b.channels[0].mode, Mode::Absorbance);
        assert_eq!(b.channels[0].wavelength_nm, Some(600.0));
    }
}

//! What the terminal screen looks like, as plain data: cells, their colours,
//! and a line of cells folded into text plus styled spans. No UI types.

use std::ops::Range;

pub type Rgb = (u8, u8, u8);

/// The terminal's own colours, matching the app theme.
pub const DEFAULT_FG: Rgb = (0xe7, 0xe8, 0xea);
pub const DEFAULT_BG: Rgb = (0x0c, 0x0d, 0x0f);

const ANSI: [Rgb; 16] = [
    (0x1d, 0x1f, 0x21),
    (0xe5, 0x53, 0x4b),
    (0x3f, 0xb5, 0x6b),
    (0xe0, 0xb0, 0x4a),
    (0x4a, 0x82, 0xe6),
    (0xb0, 0x6f, 0xe0),
    (0x4a, 0xc0, 0xc8),
    (0xc8, 0xcb, 0xd0),
    (0x5c, 0x61, 0x6b),
    (0xff, 0x7b, 0x72),
    (0x5a, 0xd1, 0x8a),
    (0xf2, 0xc8, 0x6a),
    (0x6a, 0x98, 0xec),
    (0xc9, 0x8f, 0xf0),
    (0x6a, 0xd8, 0xe0),
    (0xff, 0xff, 0xff),
];

/// One character cell with the style it is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    /// `None` is the terminal's own background.
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub underline: bool,
}

impl Cell {
    pub const BLANK: Self = Self { ch: ' ', fg: DEFAULT_FG, bg: None, bold: false, underline: false };

    fn is_plain_blank(&self) -> bool {
        self.ch == ' ' && self.bg.is_none() && !self.underline
    }

    fn same_style(&self, other: &Self) -> bool {
        (self.fg, self.bg, self.bold, self.underline) == (other.fg, other.bg, other.bold, other.underline)
    }
}

/// A run of text drawn in one style. `range` is in bytes of the line's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub underline: bool,
}

/// One screen line as text and the spans that style it. Trailing plain blanks
/// are dropped.
#[must_use]
pub fn fold_line(cells: &[Cell]) -> (String, Vec<Span>) {
    let end = cells.iter().rposition(|c| !c.is_plain_blank()).map_or(0, |i| i + 1);
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut previous: Option<&Cell> = None;
    for cell in &cells[..end] {
        let start = text.len();
        text.push(if cell.ch == '\0' { ' ' } else { cell.ch });
        match (previous, spans.last_mut()) {
            (Some(before), Some(span)) if before.same_style(cell) => span.range.end = text.len(),
            _ => spans.push(Span {
                range: start..text.len(),
                fg: cell.fg,
                bg: cell.bg,
                bold: cell.bold,
                underline: cell.underline,
            }),
        }
        previous = Some(cell);
    }
    (text, spans)
}

/// The colour for a 256-colour palette index: the 16 named colours, then a 6×6×6 cube, then greys.
#[must_use]
pub fn indexed(index: u8) -> Rgb {
    match index {
        0..=15 => ANSI[usize::from(index)],
        16..=231 => {
            let n = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            (level(n / 36), level(n / 6 % 6), level(n % 6))
        }
        232..=255 => {
            let grey = 8 + 10 * (index - 232);
            (grey, grey, grey)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(ch: char, fg: Rgb) -> Cell {
        Cell { ch, fg, ..Cell::BLANK }
    }

    #[test]
    fn a_line_folds_into_text_and_runs_of_one_style() {
        let red = (255, 0, 0);
        let cells = [cell('a', red), cell('b', red), cell('c', DEFAULT_FG), Cell::BLANK, Cell::BLANK];
        let (text, spans) = fold_line(&cells);
        assert_eq!(text, "abc");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].range, 0..2);
        assert_eq!(spans[1].range, 2..3);
    }

    #[test]
    fn an_empty_line_has_no_text() {
        assert_eq!(fold_line(&[Cell::BLANK; 4]), (String::new(), vec![]));
    }

    #[test]
    fn a_coloured_blank_is_kept_so_a_cursor_shows() {
        let cursor = Cell { bg: Some(DEFAULT_FG), ..Cell::BLANK };
        let (text, spans) = fold_line(&[cell('$', DEFAULT_FG), cursor, Cell::BLANK]);
        assert_eq!(text, "$ ");
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn the_colour_cube_and_greys_follow_the_standard_layout() {
        assert_eq!(indexed(1), ANSI[1]);
        assert_eq!(indexed(16), (0, 0, 0));
        assert_eq!(indexed(231), (255, 255, 255));
        assert_eq!(indexed(232), (8, 8, 8));
        assert_eq!(indexed(255), (238, 238, 238));
    }
}

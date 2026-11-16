use std::sync::Arc;

use anyhow::Result;
use libghostty_vt::{
    Error, RenderState, Terminal,
    render::{CellIterator, RowIterator},
    screen::{CellContentTag, CellWide, GridRef},
    style::{Palette, RgbColor, StyleColor, Underline},
    terminal::{Mode, Point, PointCoordinate, ScrollViewport},
};

use crate::MAX_LINES;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum TermColor {
    Ansi(u8),
    Rgb(u8, u8, u8),
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Style {
    pub(crate) fg: Option<TermColor>,
    pub(crate) bg: Option<TermColor>,
    pub(crate) underline_color: Option<TermColor>,
    pub(crate) bold: bool,
    pub(crate) faint: bool,
    pub(crate) italic: bool,
    pub(crate) underline: Option<Underline>,
    pub(crate) blink: bool,
    pub(crate) reverse: bool,
    pub(crate) invisible: bool,
    pub(crate) strikethrough: bool,
    pub(crate) overline: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) width: u8,
    pub(crate) style: Style,
    pub(crate) hyperlink: Option<Arc<str>>,
}

#[derive(Clone, Debug)]
struct Span {
    text: String,
    widths: Vec<u8>,
    style: Style,
    hyperlink: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    spans: Vec<Span>,
    width: usize,
}

impl Line {
    fn push(&mut self, cell: Cell) {
        self.width += cell.width as usize;
        if let Some(last) = self.spans.last_mut()
            && last.style == cell.style
            && last.hyperlink == cell.hyperlink
        {
            last.text.push(cell.ch);
            last.widths.push(cell.width);
            return;
        }
        self.spans.push(Span {
            text: cell.ch.to_string(),
            widths: vec![cell.width],
            style: cell.style,
            hyperlink: cell.hyperlink,
        });
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Cell> + '_ {
        self.spans.iter().flat_map(|span| {
            span.text
                .chars()
                .zip(&span.widths)
                .map(|(ch, &width)| Cell {
                    ch,
                    width,
                    style: span.style,
                    hyperlink: span.hyperlink.clone(),
                })
        })
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }
}

pub(crate) fn sanitize_terminal_text(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '\0'..='\u{1f}' => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{fffd}'),
            '\u{7f}' => '\u{2421}',
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// Decode the original bytes once, then join soft wraps for horizontal paging.
/// Geometry affects VT cursor addressing, but never the stored row wrapping.
pub(crate) fn parse_vt(input: &[u8], cols: u16, rows: u16) -> Result<Vec<Line>> {
    let mut terminal = vt_terminal(cols, rows)?;
    terminal.vt_write(input);
    snapshot_lines(&mut terminal)
}

pub(crate) fn vt_terminal<'cb>(cols: u16, rows: u16) -> Result<Terminal<'static, 'cb>> {
    let mut terminal = Terminal::new(cols.max(1), rows.max(1))?;
    terminal
        .set_default_mode(Mode::LINEFEED, true)?
        .set_default_mode(Mode::GRAPHEME_CLUSTER, true)?
        .set_scrollback_max_bytes(Some(usize::MAX))?
        .set_scrollback_max_lines(None)?;
    // Host title, clipboard, and other effects remain unregistered. Command
    // capture may route query replies to its child PTY, never to the host.
    Ok(terminal)
}

pub(crate) fn snapshot_lines(terminal: &mut Terminal<'_, '_>) -> Result<Vec<Line>> {
    let total = terminal.total_rows()?;
    let history = terminal.scrollback_rows()?;
    let cursor_end = history
        + terminal.cursor_y()? as usize
        + usize::from(terminal.cursor_x()? > 0 || terminal.is_cursor_pending_wrap()?);
    let colors = Colors {
        fg: terminal.fg_color()?,
        bg: terminal.bg_color()?,
        palette: terminal.color_palette()?,
        defaults: terminal.default_color_palette()?,
        reverse: terminal.mode(Mode::REVERSE_COLORS)?,
    };
    let mut render = RenderState::new()?;
    let mut row_iter = RowIterator::new()?;
    let mut cell_iter = CellIterator::new()?;
    let mut text = String::new();
    let mut uri = Vec::new();
    let mut out = Vec::new();
    let mut current = Line::default();
    let mut extent = 0;
    let mut processed = 0;
    let mut previous_offset = 0;
    terminal.scroll_viewport(ScrollViewport::Top);
    while processed < total && out.len() < MAX_LINES {
        // The last window clamps to the active area. Skip overlapping rows.
        let offset = processed.min(history);
        terminal.scroll_viewport(ScrollViewport::Delta((offset - previous_offset) as isize));
        previous_offset = offset;
        let snapshot = render.update(terminal)?;
        let mut row = row_iter.update(&snapshot)?;
        while let Some(row) = row.next() {
            let y = row.viewport_y()? as usize;
            let index = offset + y;
            if index < processed {
                continue;
            }
            let wrapped = row.raw_row()?.is_wrapped()?;
            let mut cells = cell_iter.update(row)?;
            let mut pending_blanks = Vec::new();
            let mut x = 0;
            while let Some(cell) = cells.next() {
                let raw = cell.raw_cell()?;
                let column = x;
                x += 1;
                let wide = raw.wide()?;
                if matches!(wide, CellWide::SpacerHead | CellWide::SpacerTail) {
                    continue;
                }
                let mut style = cell.style()?;
                style.bg_color = match raw.content_tag()? {
                    CellContentTag::BgColorPalette => StyleColor::Palette(raw.bg_color_palette()?),
                    CellContentTag::BgColorRgb => StyleColor::Rgb(raw.bg_color_rgb()?),
                    _ => style.bg_color,
                };
                let style = colors.style(style);
                let hyperlink = if raw.has_hyperlink()? {
                    let grid = terminal.grid_ref(Point::Viewport(PointCoordinate {
                        x: column,
                        y: y as u32,
                    }))?;
                    read_hyperlink(&grid, &mut uri)?
                } else {
                    None
                };
                cell.graphemes_utf8(&mut text)?;
                let width = if wide == CellWide::Wide { 2 } else { 1 };
                if text.is_empty() {
                    let blank = Cell {
                        ch: ' ',
                        width,
                        style,
                        hyperlink,
                    };
                    if !wrapped && style == Style::default() && blank.hyperlink.is_none() {
                        pending_blanks.push(blank);
                        continue;
                    }
                    for blank in pending_blanks.drain(..) {
                        current.push(blank);
                    }
                    current.push(blank);
                } else {
                    for blank in pending_blanks.drain(..) {
                        current.push(blank);
                    }
                    for (i, ch) in text.chars().enumerate() {
                        current.push(Cell {
                            ch,
                            width: if i == 0 { width } else { 0 },
                            style,
                            hyperlink: hyperlink.clone(),
                        });
                    }
                }
            }
            processed = index + 1;
            if !wrapped {
                let meaningful = current.width() > 0 || processed <= cursor_end;
                out.push(std::mem::take(&mut current));
                if meaningful {
                    extent = out.len();
                }
                if out.len() >= MAX_LINES {
                    break;
                }
            }
        }
    }
    out.truncate(extent);
    Ok(out)
}

fn read_hyperlink(grid: &GridRef<'_>, buf: &mut Vec<u8>) -> Result<Option<Arc<str>>> {
    let len = match grid.hyperlink_uri(buf) {
        Err(Error::OutOfSpace { required }) => {
            buf.resize(required, 0);
            grid.hyperlink_uri(buf)?
        }
        result => result?,
    };
    let uri = std::str::from_utf8(&buf[..len])?;
    Ok((!uri.is_empty() && !uri.chars().any(char::is_control)).then(|| Arc::from(uri)))
}

struct Colors {
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
    palette: Palette,
    defaults: Palette,
    reverse: bool,
}

impl Colors {
    fn color(&self, color: StyleColor, default: Option<RgbColor>) -> Option<TermColor> {
        match color {
            StyleColor::None => default.map(|c| TermColor::Rgb(c.r, c.g, c.b)),
            StyleColor::Rgb(c) => Some(TermColor::Rgb(c.r, c.g, c.b)),
            StyleColor::Palette(index) => {
                let c = self.palette.get(index);
                Some(if c == self.defaults.get(index) {
                    TermColor::Ansi(index.0)
                } else {
                    TermColor::Rgb(c.r, c.g, c.b)
                })
            }
        }
    }

    fn style(&self, s: libghostty_vt::style::Style) -> Style {
        Style {
            fg: self.color(s.fg_color, self.fg),
            bg: self.color(s.bg_color, self.bg),
            underline_color: self.color(s.underline_color, None),
            bold: s.bold,
            faint: s.faint,
            italic: s.italic,
            underline: (s.underline != Underline::None).then_some(s.underline),
            blink: s.blink,
            reverse: s.inverse ^ self.reverse,
            invisible: s.invisible,
            strikethrough: s.strikethrough,
            overline: s.overline,
        }
    }
}

#[cfg(test)]
pub(crate) fn parse_ansi(input: &str) -> Result<Vec<Line>> {
    parse_vt(input.as_bytes(), 80, 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.iter().map(|cell| cell.ch).collect())
            .collect()
    }

    #[test]
    fn controls_are_interpreted_even_without_escape_bytes() -> Result<()> {
        for (input, expected) in [
            ("let count = 42;", "let count = 42;"),
            ("abcdef\rxy", "xycdef"),
            ("A\x08A", "A"),
            ("_\x08X", "X"),
            ("e\u{301}\x08X", "X"),
            ("a\tb\t界\tZ", "a       b       界      Z"),
            ("\x1b(0lqk\x1b(B", "┌─┐"),
            ("abc\x1b[2DX", "aXc"),
            ("abcdef\x08\x08\x1b[KX", "abcdX"),
        ] {
            let lines = parse_ansi(input)?;
            assert_eq!(plain(&lines), [expected], "{input:?}");
            assert!(lines[0].iter().all(|cell| cell.style == Style::default()));
        }
        assert_eq!(plain(&parse_ansi("one\ntwo\x1b[A\rX")?), ["Xne", "two"]);
        Ok(())
    }

    #[test]
    fn line_endings_and_blank_lines_survive_without_screen_padding() -> Result<()> {
        for (input, expected) in [
            ("", vec![]),
            ("\n", vec![""]),
            ("one\n\n", vec!["one", ""]),
            ("one\r\ntwo\r\n", vec!["one", "two"]),
            ("discard\x1bcfirst\nsecond", vec!["first", "second"]),
            ("\x1b]8;;https://example.com", vec![]),
        ] {
            assert_eq!(plain(&parse_ansi(input)?), expected, "{input:?}");
        }
        Ok(())
    }

    #[test]
    fn soft_wraps_and_scrollback_become_unwrapped_pageable_lines() -> Result<()> {
        let long = "a".repeat(100_000);
        let lines = parse_ansi(&long)?;
        assert_eq!(plain(&lines), [long]);
        assert_eq!(lines[0].width(), 100_000);
        assert_eq!(lines[0].spans.len(), 1);
        let input = (0..500).map(|i| format!("row {i}\n")).collect::<String>();
        let lines = parse_vt(input.as_bytes(), 16, 3)?;
        assert_eq!(lines.len(), 500);
        assert_eq!(plain(&lines)[0], "row 0");
        assert_eq!(plain(&lines)[499], "row 499");
        Ok(())
    }

    #[test]
    fn wide_graphemes_keep_the_emulators_column_widths() -> Result<()> {
        let lines = parse_vt("abc界e\u{301}Z".as_bytes(), 4, 2)?;
        assert_eq!(plain(&lines), ["abc界e\u{301}Z"]);
        assert_eq!(lines[0].width(), 7);
        let emoji = parse_ansi("👩\u{200d}💻X")?;
        assert_eq!(plain(&emoji), ["👩\u{200d}💻X"]);
        assert_eq!(emoji[0].width(), 3);
        assert_eq!(plain(&parse_ansi("界A\x08\x08Z")?), [" ZA"]);
        Ok(())
    }

    #[test]
    fn osc_colors_and_erased_backgrounds_are_not_discarded() -> Result<()> {
        let lines = parse_ansi("\x1b]4;1;rgb:11/82/e7\x1b\\\x1b[31mX\n\x1b[48;5;25m\x1b[K")?;
        assert_eq!(
            lines[0].iter().next().unwrap().style.fg,
            Some(TermColor::Rgb(17, 130, 231))
        );
        assert_eq!(lines[1].width(), 80);
        assert!(
            lines[1]
                .iter()
                .all(|cell| cell.ch == ' ' && cell.style.bg == Some(TermColor::Ansi(25)))
        );
        Ok(())
    }

    #[test]
    fn hyperlinks_survive_but_terminal_effects_are_not_replayed() -> Result<()> {
        let lines = parse_ansi(
            "\x1b]0;title\x07\x1b]52;c;AAAA\x07\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\plain",
        )?;
        assert_eq!(plain(&lines), ["clickplain"]);
        assert!(
            lines[0]
                .iter()
                .take(5)
                .all(|cell| cell.hyperlink.as_deref() == Some("https://example.com"))
        );
        assert!(lines[0].iter().skip(5).all(|cell| cell.hyperlink.is_none()));
        assert_eq!(sanitize_terminal_text("x\x1b]52;y\n"), "x␛]52;y␊");
        Ok(())
    }
}

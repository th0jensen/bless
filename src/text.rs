use std::path::PathBuf;

use anyhow::Result;
use libghostty_vt::{
    sgr::{Attribute, Parser as SgrParser},
    style::Underline,
};

use crate::{MAX_LINES, render::ansi_for};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum TermColor {
    Ansi(u8),
    Rgb(u8, u8, u8),
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Style {
    pub(crate) fg: Option<TermColor>,
    pub(crate) bg: Option<TermColor>,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) reverse: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) style: Style,
}

#[derive(Clone, Debug)]
struct Span {
    text: String,
    style: Style,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    spans: Vec<Span>,
}

impl Line {
    pub(crate) fn push(&mut self, cell: Cell) {
        if let Some(last) = self.spans.last_mut()
            && last.style == cell.style
        {
            last.text.push(cell.ch);
            return;
        }
        self.spans.push(Span {
            text: cell.ch.to_string(),
            style: cell.style,
        });
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Cell> + '_ {
        self.spans.iter().flat_map(|span| {
            span.text.chars().map(|ch| Cell {
                ch,
                style: span.style,
            })
        })
    }
}

pub(crate) fn sanitize_terminal_text(input: &str) -> String {
    input.chars().map(visible_control).collect()
}

fn visible_control(ch: char) -> char {
    match ch {
        '\0'..='\u{1f}' => char::from_u32(0x2400 + ch as u32).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        c if c.is_control() => '\u{fffd}',
        c => c,
    }
}

fn apply_sgr(sgr: &mut SgrParser<'_>, params: &[u16], seps: &[u8], style: &mut Style) {
    let params = if params.is_empty() {
        &[0u16][..]
    } else {
        params
    };
    let seps = (!seps.is_empty()).then_some(seps);
    if sgr.set_params(params, seps).is_err() {
        return;
    }
    while let Ok(Some(attr)) = sgr.next() {
        apply_attr(attr, style);
    }
    apply_basic_sgr(params, style);
}

fn apply_basic_sgr(params: &[u16], s: &mut Style) {
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => *s = Style::default(),
            1 => s.bold = true,
            3 => s.italic = true,
            4 => s.underline = true,
            7 => s.reverse = true,
            22 => s.bold = false,
            23 => s.italic = false,
            24 => s.underline = false,
            27 => s.reverse = false,
            30..=37 => s.fg = Some(TermColor::Ansi((params[i] - 30) as u8)),
            40..=47 => s.bg = Some(TermColor::Ansi((params[i] - 40) as u8)),
            90..=97 => s.fg = Some(TermColor::Ansi((params[i] - 90 + 8) as u8)),
            100..=107 => s.bg = Some(TermColor::Ansi((params[i] - 100 + 8) as u8)),
            39 => s.fg = None,
            49 => s.bg = None,
            38 | 48 if i + 2 < params.len() && params[i + 1] == 5 => {
                let color = TermColor::Ansi(params[i + 2].min(255) as u8);
                if params[i] == 38 {
                    s.fg = Some(color);
                } else {
                    s.bg = Some(color);
                }
                i += 2;
            }
            38 | 48 if i + 4 < params.len() && params[i + 1] == 2 => {
                let color = TermColor::Rgb(
                    params[i + 2].min(255) as u8,
                    params[i + 3].min(255) as u8,
                    params[i + 4].min(255) as u8,
                );
                if params[i] == 38 {
                    s.fg = Some(color);
                } else {
                    s.bg = Some(color);
                }
                i += 4;
            }
            _ => {}
        }
        i += 1;
    }
}

fn apply_attr(attr: Attribute<'_>, s: &mut Style) {
    match attr {
        Attribute::Unset => *s = Style::default(),
        Attribute::Bold => s.bold = true,
        Attribute::ResetBold => s.bold = false,
        Attribute::Italic => s.italic = true,
        Attribute::ResetItalic => s.italic = false,
        Attribute::Underline(underline) => s.underline = underline != Underline::None,
        Attribute::Inverse => s.reverse = true,
        Attribute::ResetInverse => s.reverse = false,
        Attribute::DirectColorFg(c) => s.fg = Some(TermColor::Rgb(c.r, c.g, c.b)),
        Attribute::DirectColorBg(c) => s.bg = Some(TermColor::Rgb(c.r, c.g, c.b)),
        Attribute::Fg8(c) => s.fg = Some(TermColor::Ansi(c.0)),
        Attribute::Bg8(c) => s.bg = Some(TermColor::Ansi(c.0)),
        Attribute::BrightFg8(c) => s.fg = Some(TermColor::Ansi(c.0)),
        Attribute::BrightBg8(c) => s.bg = Some(TermColor::Ansi(c.0)),
        Attribute::Fg256(c) => s.fg = Some(TermColor::Ansi(c.0)),
        Attribute::Bg256(c) => s.bg = Some(TermColor::Ansi(c.0)),
        Attribute::ResetFg => s.fg = None,
        Attribute::ResetBg => s.bg = None,
        _ => {}
    }
}

pub(crate) fn parse_ansi(input: &str) -> Result<(Vec<Line>, Vec<String>)> {
    let mut out = Vec::new();
    let mut raw_out = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut col = 0usize;
    let mut raw_cur = String::new();
    let mut style = Style::default();
    let mut sgr = SgrParser::new()?;
    let mut it = input.chars().peekable();
    while let Some(ch) = it.next() {
        match ch {
            '\n' => {
                if out.len() >= MAX_LINES {
                    break;
                }
                finish_line(&mut out, &mut raw_out, &mut cur, &mut raw_cur, &mut col);
            }
            '\r' if it.peek() != Some(&'\n') => {
                cur.clear();
                col = 0;
                raw_cur.clear();
                if style != Style::default() {
                    raw_cur.push_str(&ansi_for(style));
                }
            }
            '\x08' => {
                col = col.saturating_sub(1);
                raw_cur.push('\x08');
            }
            '\t' => {
                raw_cur.push('\t');
                for _ in 0..4 {
                    overwrite_cell(&mut cur, &mut col, Cell { ch: ' ', style });
                }
            }
            '\x1b' => match it.next() {
                Some('[') => {
                    let mut raw = String::new();
                    for c in it.by_ref() {
                        if c.is_ascii_alphabetic() || matches!(c, '@' | '`' | '~') {
                            if c == 'm' {
                                let (p, seps) = parse_sgr_params(&raw);
                                apply_sgr(&mut sgr, &p, &seps, &mut style);
                                raw_cur.push_str("\x1b[");
                                raw_cur.push_str(&raw);
                                raw_cur.push('m');
                            }
                            break;
                        }
                        raw.push(c);
                    }
                }
                Some(']') => {
                    if let Some(seq) = capture_osc_hyperlink(&mut it) {
                        raw_cur.push_str(&seq);
                    }
                }
                Some('P' | '_' | '^' | 'X' | 'G') => skip_until_st(&mut it),
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                    it.next();
                }
                _ => {}
            },
            c if !c.is_control() => {
                raw_cur.push(c);
                overwrite_cell(&mut cur, &mut col, Cell { ch: c, style });
            }
            _ => {}
        }
    }
    if !input.ends_with('\n') {
        finish_line(&mut out, &mut raw_out, &mut cur, &mut raw_cur, &mut col);
    }
    Ok((out, raw_out))
}

fn overwrite_cell(cur: &mut Vec<Cell>, col: &mut usize, cell: Cell) {
    match cur.get_mut(*col) {
        Some(existing) if existing.ch == cell.ch => existing.style.bold = true,
        Some(existing) if existing.ch == '_' && cell.ch != '_' => {
            *existing = Cell {
                ch: cell.ch,
                style: Style {
                    underline: true,
                    ..cell.style
                },
            };
        }
        Some(existing) => *existing = cell,
        None => cur.push(cell),
    }
    *col += 1;
}

fn finish_line(
    out: &mut Vec<Line>,
    raw_out: &mut Vec<String>,
    cur: &mut Vec<Cell>,
    raw_cur: &mut String,
    col: &mut usize,
) {
    let mut line = Line::default();
    for cell in cur.drain(..) {
        line.push(cell);
    }
    out.push(line);
    raw_out.push(std::mem::take(raw_cur));
    *col = 0;
}

fn skip_until_st<I: Iterator<Item = char>>(it: &mut std::iter::Peekable<I>) {
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'\\') {
            it.next();
            break;
        }
    }
}

fn capture_osc_hyperlink<I: Iterator<Item = char>>(
    it: &mut std::iter::Peekable<I>,
) -> Option<String> {
    let mut body = String::new();
    let mut terminator = String::new();
    while let Some(c) = it.next() {
        if c == '\x07' {
            terminator.push(c);
            break;
        }
        if c == '\x1b' && it.peek() == Some(&'\\') {
            it.next();
            terminator.push_str("\x1b\\");
            break;
        }
        body.push(c);
    }
    (!terminator.is_empty() && body.starts_with("8;")).then(|| format!("\x1b]{body}{terminator}"))
}

fn parse_sgr_params(raw: &str) -> (Vec<u16>, Vec<u8>) {
    let mut nums = Vec::new();
    let mut seps = Vec::new();
    let mut cur = String::new();
    for c in raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ';' || *c == ':')
    {
        if c == ';' || c == ':' {
            nums.push(cur.parse().unwrap_or(0));
            seps.push(c as u8);
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() || raw.ends_with(';') || raw.ends_with(':') {
        nums.push(cur.parse().unwrap_or(0));
        seps.push(b';');
    }
    (nums, seps)
}

pub(crate) fn highlight_text(input: &str, path: Option<&PathBuf>) -> Vec<Line> {
    let lang = path
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    input
        .lines()
        .take(MAX_LINES)
        .map(|line| highlight_line_simple(line, lang))
        .collect()
}

fn highlight_line_simple(line: &str, lang: &str) -> Line {
    let plain = Style::default();
    let comment = Style {
        fg: Some(TermColor::Ansi(8)),
        italic: true,
        ..Style::default()
    };
    let string = Style {
        fg: Some(TermColor::Ansi(10)),
        ..Style::default()
    };
    let number = Style {
        fg: Some(TermColor::Ansi(11)),
        ..Style::default()
    };
    let keyword = Style {
        fg: Some(TermColor::Ansi(13)),
        bold: true,
        ..Style::default()
    };
    let punctuation = Style {
        fg: Some(TermColor::Ansi(12)),
        ..Style::default()
    };

    let comment_marker = match lang {
        "rs" | "c" | "h" | "cpp" | "hpp" | "js" | "ts" | "go" | "java" | "zig" => Some("//"),
        "sh" | "bash" | "zsh" | "fish" | "toml" | "yaml" | "yml" | "py" => Some("#"),
        _ => None,
    };

    let mut cells = Line::default();
    let mut chars = line.char_indices().peekable();
    let mut in_string: Option<char> = None;

    while let Some((_, ch)) = chars.next() {
        if let Some(q) = in_string {
            push_cell(&mut cells, ch, string);
            if ch == '\\' {
                if let Some((_, escaped)) = chars.next() {
                    push_cell(&mut cells, escaped, string);
                }
            } else if ch == q {
                in_string = None;
            }
            continue;
        }

        let starts_comment = match comment_marker {
            Some("#") => ch == '#',
            Some("//") => ch == '/' && chars.peek().is_some_and(|(_, next)| *next == '/'),
            _ => false,
        };
        if starts_comment {
            push_cell(&mut cells, ch, comment);
            if comment_marker == Some("//")
                && let Some((_, slash)) = chars.next()
            {
                push_cell(&mut cells, slash, comment);
            }
            for (_, rest) in chars {
                push_cell(&mut cells, rest, comment);
            }
            break;
        }

        if ch == '"' || ch == '\'' || ch == '`' {
            in_string = Some(ch);
            push_cell(&mut cells, ch, string);
        } else if ch.is_ascii_digit() {
            push_cell(&mut cells, ch, number);
        } else if is_ident_start(ch) {
            let mut word = String::from(ch);
            while let Some((_, next)) = chars.peek().copied() {
                if is_ident_continue(next) {
                    word.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            let st = if is_keyword(&word, lang) {
                keyword
            } else {
                plain
            };
            for c in word.chars() {
                push_cell(&mut cells, c, st);
            }
        } else if "{}[]()<>:=+-*/%!|&.,;".contains(ch) {
            push_cell(&mut cells, ch, punctuation);
        } else {
            push_cell(&mut cells, ch, plain);
        }
    }
    cells
}

fn push_cell(cells: &mut Line, ch: char, style: Style) {
    if ch == '\t' {
        for _ in 0..4 {
            cells.push(Cell { ch: ' ', style });
        }
    } else {
        cells.push(Cell {
            ch: visible_control(ch),
            style,
        });
    }
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

fn is_keyword(word: &str, lang: &str) -> bool {
    match lang {
        "rs" => matches!(
            word,
            "as" | "async"
                | "await"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "fn"
                | "for"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "Self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "type"
                | "unsafe"
                | "use"
                | "where"
                | "while"
        ),
        "sh" | "bash" | "zsh" | "fish" => matches!(
            word,
            "if" | "then"
                | "else"
                | "elif"
                | "fi"
                | "for"
                | "while"
                | "do"
                | "done"
                | "case"
                | "esac"
                | "function"
                | "return"
                | "export"
                | "local"
                | "set"
        ),
        "toml" | "yaml" | "yml" | "json" => matches!(word, "true" | "false" | "null"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn bare_carriage_return_collapses_progress_output() -> Result<()> {
        let input = "\x1b[32mLoading\x1b[0m\r\x1b[32mLoading..\x1b[0m\r\x1b[32mDone!\x1b[0m\n";
        let (lines, raw) = parse_ansi(input)?;
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].iter().map(|c| c.ch).collect();
        assert_eq!(text, "Done!");
        assert!(raw[0].contains("Done!"), "{:?}", raw[0]);
        assert!(!raw[0].contains("Loading"), "{:?}", raw[0]);
        Ok(())
    }

    #[test]
    fn crlf_is_a_plain_newline_not_an_overwrite() -> Result<()> {
        let input = "first line\r\nsecond line\r\n";
        let (lines, _raw) = parse_ansi(input)?;
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0].iter().map(|c| c.ch).collect::<String>(),
            "first line"
        );
        assert_eq!(
            lines[1].iter().map(|c| c.ch).collect::<String>(),
            "second line"
        );
        Ok(())
    }

    #[test]
    fn osc8_hyperlink_is_preserved_but_other_osc_is_stripped() -> Result<()> {
        let input =
            "\x1b]8;;https://example.com\x1b\\click\x1b]8;;\x1b\\\n\x1b]0;window title\x07plain\n";
        let (_lines, raw) = parse_ansi(input)?;
        assert!(
            raw[0].contains("\x1b]8;;https://example.com\x1b\\"),
            "{:?}",
            raw[0]
        );
        assert!(raw[0].contains("click"), "{:?}", raw[0]);
        assert!(raw[0].contains("\x1b]8;;\x1b\\"), "{:?}", raw[0]);
        assert!(!raw[1].contains("window title"), "{:?}", raw[1]);
        assert!(raw[1].contains("plain"), "{:?}", raw[1]);
        Ok(())
    }

    #[test]
    fn unterminated_osc8_is_never_replayed() -> Result<()> {
        let (_lines, raw) = parse_ansi("\x1b]8;;https://example.com")?;
        assert_eq!(raw, vec![String::new()]);
        Ok(())
    }

    #[test]
    fn backspace_overstrike_bold_collapses_doubled_characters() -> Result<()> {
        let (lines, raw) = parse_ansi("N\x08NA\x08AM\x08ME\x08E\n")?;
        assert_eq!(lines.len(), 1);
        let cells: Vec<Cell> = lines[0].iter().collect();
        let text: String = cells.iter().map(|c| c.ch).collect();
        assert_eq!(text, "NAME");
        assert!(cells.iter().all(|c| c.style.bold), "{cells:?}");
        assert!(raw[0].contains('\x08'), "{:?}", raw[0]);
        Ok(())
    }

    #[test]
    fn backspace_overstrike_underline_keeps_the_real_character() -> Result<()> {
        let (lines, _raw) = parse_ansi("_\x08X\n")?;
        let cells: Vec<Cell> = lines[0].iter().collect();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].ch, 'X');
        assert!(cells[0].style.underline, "{cells:?}");
        Ok(())
    }

    #[test]
    fn comment_markers_inside_strings_stay_string_colored() -> Result<()> {
        let line = highlight_line_simple(r#"let url = "https://example"; // comment"#, "rs");
        let cells: Vec<Cell> = line.iter().collect();
        let text: String = cells.iter().map(|cell| cell.ch).collect();
        let url_idx = text.find("https").context("URL in highlighted test line")?;
        let comment_idx = text
            .find("// comment")
            .context("comment in highlighted test line")?;
        assert_eq!(cells[url_idx].style.fg, Some(TermColor::Ansi(10)));
        assert_eq!(cells[comment_idx].style.fg, Some(TermColor::Ansi(8)));
        assert!(cells[comment_idx].style.italic);
        Ok(())
    }

    #[test]
    fn plain_text_is_compressed_into_one_style_run() {
        let lines = highlight_text(&"a".repeat(100_000), None);
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(lines[0].spans[0].text.len(), 100_000);
    }

    #[test]
    fn plain_controls_and_status_text_are_made_visible() {
        let lines = highlight_text("a\x07b\rc", None);
        let text: String = lines[0].iter().map(|cell| cell.ch).collect();
        assert_eq!(text, "a␇b␍c");
        assert_eq!(sanitize_terminal_text("x\x1b]52;y\n"), "x␛]52;y␊");
    }
}

use std::path::PathBuf;

use anyhow::Result;
use libghostty_vt::{
    sgr::{Attribute, Parser as SgrParser},
    style::Underline,
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
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) reverse: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) style: Style,
}

pub(crate) type Line = Vec<Cell>;

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
    // Harden common SGR handling too. libghostty-vt is still the parser of
    // record above, but this makes simple producer output (fastfetch, rg,
    // git, etc.) visibly correct even if the WIP safe API changes semantics.
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
        // libghostty-vt already returns bright named colors as palette indexes 8..15.
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
    let mut out = vec![Vec::new()];
    let mut raw_out = vec![String::new()];
    let mut style = Style::default();
    let mut sgr = SgrParser::new()?;
    let mut it = input.chars().peekable();
    while let Some(ch) = it.next() {
        match ch {
            '\n' => {
                if out.len() >= MAX_LINES {
                    break;
                }
                out.push(Vec::new());
                raw_out.push(String::new());
            }
            '\r' => {}
            '\t' => {
                raw_out.last_mut().unwrap().push('\t');
                for _ in 0..4 {
                    out.last_mut().unwrap().push(Cell { ch: ' ', style });
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
                                raw_out.last_mut().unwrap().push_str("\x1b[");
                                raw_out.last_mut().unwrap().push_str(&raw);
                                raw_out.last_mut().unwrap().push('m');
                            }
                            break;
                        }
                        raw.push(c);
                    }
                }
                Some(']') => skip_until_bel_or_st(&mut it),
                Some('P' | '_' | '^' | 'X' | 'G') => skip_until_st(&mut it),
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/') => {
                    it.next();
                }
                _ => {}
            },
            c if !c.is_control() => {
                raw_out.last_mut().unwrap().push(c);
                out.last_mut().unwrap().push(Cell { ch: c, style });
            }
            _ => {}
        }
    }
    if out.last().is_some_and(|line| line.is_empty()) && input.ends_with('\n') {
        out.pop();
        raw_out.pop();
    }
    Ok((out, raw_out))
}

fn skip_until_st<I: Iterator<Item = char>>(it: &mut std::iter::Peekable<I>) {
    while let Some(c) = it.next() {
        if c == '\x1b' && it.peek() == Some(&'\\') {
            it.next();
            break;
        }
    }
}

fn skip_until_bel_or_st<I: Iterator<Item = char>>(it: &mut std::iter::Peekable<I>) {
    while let Some(c) = it.next() {
        if c == '\x07' {
            break;
        }
        if c == '\x1b' && it.peek() == Some(&'\\') {
            it.next();
            break;
        }
    }
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

    let comment_start = match lang {
        "rs" | "c" | "h" | "cpp" | "hpp" | "js" | "ts" | "go" | "java" | "zig" => line.find("//"),
        "sh" | "bash" | "zsh" | "fish" | "toml" | "yaml" | "yml" | "py" => line.find('#'),
        _ => None,
    };

    let mut cells = Vec::new();
    let mut chars = line.char_indices().peekable();
    let mut in_string: Option<char> = None;

    while let Some((i, ch)) = chars.next() {
        if comment_start.is_some_and(|start| i >= start) {
            push_cell(&mut cells, ch, comment);
            continue;
        }

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
        cells.push(Cell { ch, style });
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

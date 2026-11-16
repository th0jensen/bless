use libghostty_vt::{style::Underline, unicode::codepoint_width};

use crate::text::{Line, Style, TermColor};

pub(crate) fn render_line(
    line: &Line,
    left: usize,
    width: usize,
    search: Option<&str>,
    current_match: bool,
) -> (String, usize) {
    let hit = search.and_then(|query| find_match(line, query));
    let mut rendered = String::new();
    let mut col = 0usize;
    let mut logical = 0usize;
    let mut current = Style::default();
    let mut current_link = None;
    let right = left.saturating_add(width);
    let mut previous_visible = false;
    rendered.push_str("\x1b[0m");
    for (cell_idx, cell) in line.iter().enumerate() {
        let cw = cell.width as usize;
        let start = logical;
        logical += cw;
        if cw == 0 && !previous_visible {
            continue;
        }
        if cw > 0 && logical <= left {
            previous_visible = false;
            continue;
        }
        if cw > 0 && start >= right {
            break;
        }
        let visible_width = logical.min(right).saturating_sub(start.max(left));
        let whole = cw == 0 || (start >= left && logical <= right);
        let mut st = cell.style;
        if hit
            .map(|(start, end)| cell_idx >= start && cell_idx < end)
            .unwrap_or(false)
        {
            if current_match {
                st.bg = Some(TermColor::Ansi(1));
                st.fg = Some(TermColor::Ansi(15));
            } else {
                st.bg = Some(TermColor::Ansi(3));
                st.fg = Some(TermColor::Ansi(0));
            }
        }
        if st != current {
            rendered.push_str(&ansi_for(st));
            current = st;
        }
        if cell.hyperlink != current_link {
            if current_link.is_some() {
                rendered.push_str("\x1b]8;;\x1b\\");
            }
            if let Some(uri) = cell.hyperlink.as_deref() {
                rendered.push_str("\x1b]8;;");
                rendered.push_str(uri);
                rendered.push_str("\x1b\\");
            }
            current_link = cell.hyperlink;
        }
        if whole {
            rendered.push(cell.ch);
        } else {
            rendered.extend(std::iter::repeat_n(' ', visible_width));
        }
        col += visible_width;
        previous_visible = whole;
    }
    if current_link.is_some() {
        rendered.push_str("\x1b]8;;\x1b\\");
    }
    rendered.push_str("\x1b[0m");
    (rendered, col)
}

pub(crate) fn line_matches(line: &Line, query: &str) -> bool {
    find_match(line, query).is_some()
}

fn find_match(line: &Line, query: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }

    let mut haystack = Vec::new();
    for (cell_idx, cell) in line.iter().enumerate() {
        for folded in cell.ch.to_lowercase() {
            haystack.push((folded, cell_idx));
        }
    }
    let start = haystack
        .windows(needle.len())
        .position(|window| window.iter().map(|(ch, _)| *ch).eq(needle.iter().copied()))?;
    Some((haystack[start].1, haystack[start + needle.len() - 1].1 + 1))
}

pub(crate) fn ansi_for(s: Style) -> String {
    let mut parts: Vec<String> = vec!["0".into()];
    if s.bold {
        parts.push("1".into());
    }
    if s.faint {
        parts.push("2".into());
    }
    if s.italic {
        parts.push("3".into());
    }
    if let Some(underline) = s.underline {
        parts.push(
            match underline {
                Underline::Double => "4:2",
                Underline::Curly => "4:3",
                Underline::Dotted => "4:4",
                Underline::Dashed => "4:5",
                _ => "4",
            }
            .into(),
        );
    }
    if s.blink {
        parts.push("5".into());
    }
    if s.reverse {
        parts.push("7".into());
    }
    if s.invisible {
        parts.push("8".into());
    }
    if s.strikethrough {
        parts.push("9".into());
    }
    if s.overline {
        parts.push("53".into());
    }
    if let Some(c) = s.fg {
        push_color(&mut parts, 38, c);
    }
    if let Some(c) = s.bg {
        push_color(&mut parts, 48, c);
    }
    if let Some(c) = s.underline_color {
        push_color(&mut parts, 58, c);
    }
    format!("\x1b[{}m", parts.join(";"))
}

fn push_color(parts: &mut Vec<String>, base: u8, c: TermColor) {
    match c {
        TermColor::Ansi(n @ 0..=7) if base == 38 => parts.push((30 + n).to_string()),
        TermColor::Ansi(n @ 8..=15) if base == 38 => parts.push((90 + n - 8).to_string()),
        TermColor::Ansi(n @ 0..=7) if base == 48 => parts.push((40 + n).to_string()),
        TermColor::Ansi(n @ 8..=15) if base == 48 => parts.push((100 + n - 8).to_string()),
        TermColor::Ansi(n) => parts.extend([base.to_string(), "5".into(), n.to_string()]),
        TermColor::Rgb(r, g, b) => parts.extend([
            base.to_string(),
            "2".into(),
            r.to_string(),
            g.to_string(),
            b.to_string(),
        ]),
    }
}

pub(crate) fn char_width(c: char) -> usize {
    codepoint_width(c) as usize
}

pub(crate) fn fit_width(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out
}

pub(crate) fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

pub(crate) fn line_width(line: &Line) -> usize {
    line.width()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::parse_ansi;

    #[test]
    fn fastfetch_style_bright_named_color_survives() -> anyhow::Result<()> {
        let lines = parse_ansi(
            "\x1b]1337;File=inline=1:AAAA\x07\x1b[90mgray\x1b[m\n\x1b_Ga=T;AAAA\x1b\\\x1b[1m\x1b[32mgreen\x1b[m",
        )?;
        assert_eq!(lines.len(), 2);
        let (rendered0, _) = render_line(&lines[0], 0, 80, None, false);
        let (rendered1, _) = render_line(&lines[1], 0, 80, None, false);
        assert!(rendered0.contains(";90m"), "{rendered0:?}");
        assert!(rendered1.contains("\x1b[0;1;32m"), "{rendered1:?}");
        Ok(())
    }

    #[test]
    fn horizontal_clipping_keeps_terminal_columns() -> anyhow::Result<()> {
        for (input, left, width, expected) in [
            ("a界bc", 0, 2, "a "),
            ("a界bc", 2, 3, " bc"),
            ("a界bc", 3, 2, "bc"),
            ("a界bc", 0, 0, ""),
            ("e\u{301}XY", 0, 2, "e\u{301}X"),
            ("e\u{301}XY", 1, 2, "XY"),
            ("界\u{301}XY", 1, 3, " XY"),
            ("👩\u{200d}💻X", 0, 2, "👩\u{200d}💻"),
            ("👩\u{200d}💻X", 1, 2, " X"),
            ("👩\u{200d}💻X", 2, 1, "X"),
        ] {
            let lines = parse_ansi(input)?;
            let (rendered, used) = render_line(&lines[0], left, width, None, false);
            let visible = parse_ansi(&rendered)?;
            let text: String = visible
                .iter()
                .flat_map(|line| line.iter())
                .map(|cell| cell.ch)
                .collect();
            assert_eq!(text, expected, "left={left}, width={width}, {input:?}");
            assert_eq!(used, width);
        }
        Ok(())
    }

    #[test]
    fn unicode_search_uses_cell_indexes_not_utf8_offsets() -> anyhow::Result<()> {
        let lines = parse_ansi("אבג café İST")?;
        assert!(line_matches(&lines[0], "בג"));
        assert!(line_matches(&lines[0], "CAFÉ"));
        assert!(line_matches(&lines[0], "i̇st"));
        let (rendered, _) = render_line(&lines[0], 0, 80, Some("café"), true);
        assert!(rendered.contains("\x1b[0;97;41mcafé"), "{rendered:?}");
        Ok(())
    }
}

use libghostty_vt::unicode::codepoint_width;

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
    rendered.push_str("\x1b[0m");
    for (cell_idx, cell) in line.iter().enumerate() {
        let cw = char_width(cell.ch);
        if logical + cw <= left {
            logical += cw;
            continue;
        }
        if col + cw > width {
            break;
        }
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
        rendered.push(cell.ch);
        col += cw;
        logical += cw;
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
    if s.italic {
        parts.push("3".into());
    }
    if s.underline {
        parts.push("4".into());
    }
    if s.reverse {
        parts.push("7".into());
    }
    if let Some(c) = s.fg {
        push_color(&mut parts, 38, c);
    }
    if let Some(c) = s.bg {
        push_color(&mut parts, 48, c);
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
    line.iter().map(|cell| char_width(cell.ch)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::parse_ansi;

    #[test]
    fn fastfetch_style_bright_named_color_survives() -> anyhow::Result<()> {
        let (lines, _raw) = parse_ansi(
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
    fn unicode_search_uses_cell_indexes_not_utf8_offsets() {
        let lines = crate::text::highlight_text("אבג café İST", None);
        assert!(line_matches(&lines[0], "בג"));
        assert!(line_matches(&lines[0], "CAFÉ"));
        assert!(line_matches(&lines[0], "i̇st"));
        let (rendered, _) = render_line(&lines[0], 0, 80, Some("café"), true);
        assert!(rendered.contains("\x1b[0;97;41mcafé"), "{rendered:?}");
    }
}

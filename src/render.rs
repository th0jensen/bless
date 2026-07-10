use crate::text::{Line, Style, TermColor};

pub(crate) fn render_line(
    line: &Line,
    left: usize,
    width: usize,
    search: Option<&str>,
) -> (String, usize) {
    let plain: String = line.iter().map(|c| c.ch).collect();
    let hit = search.and_then(|q| {
        plain
            .to_lowercase()
            .find(&q.to_lowercase())
            .map(|start| (start, start + q.chars().count()))
    });
    let mut rendered = String::new();
    let mut col = 0usize;
    let mut logical = 0usize;
    let mut current = Style::default();
    rendered.push_str("\x1b[0m");
    for cell in line {
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
            .map(|(a, b)| logical >= a && logical < b)
            .unwrap_or(false)
        {
            st.bg = Some(TermColor::Ansi(3));
            st.fg = Some(TermColor::Ansi(0));
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

fn ansi_for(s: Style) -> String {
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
        // Preserve named colors as named SGR so Ghostty applies the user's
        // configured palette. Use 256-color SGR only for true palette indexes.
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
    match c {
        '\u{0000}'..='\u{001f}' | '\u{007f}' => 0,
        '\u{1100}'..='\u{115f}'
        | '\u{2329}'
        | '\u{232a}'
        | '\u{2e80}'..='\u{a4cf}'
        | '\u{ac00}'..='\u{d7a3}'
        | '\u{f900}'..='\u{faff}'
        | '\u{fe10}'..='\u{fe19}'
        | '\u{fe30}'..='\u{fe6f}'
        | '\u{ff00}'..='\u{ff60}'
        | '\u{ffe0}'..='\u{ffe6}' => 2,
        _ => 1,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::parse_ansi;

    #[test]
    fn fastfetch_style_bright_named_color_survives() {
        let (lines, _raw) = parse_ansi("\x1b]1337;File=inline=1:AAAA\x07\x1b[90mgray\x1b[m\n\x1b_Ga=T;AAAA\x1b\\\x1b[1m\x1b[32mgreen\x1b[m").unwrap();
        assert_eq!(lines.len(), 2);
        let (rendered0, _) = render_line(&lines[0], 0, 80, None);
        let (rendered1, _) = render_line(&lines[1], 0, 80, None);
        assert!(rendered0.contains(";90m"), "{rendered0:?}");
        assert!(rendered1.contains("\x1b[0;1;32m"), "{rendered1:?}");
    }
}

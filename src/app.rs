use std::{
    fs,
    io::{self, Write},
    os::fd::AsRawFd,
    time::Duration,
};

use anyhow::Result;

use crate::{
    render::{display_width, fit_width, line_matches, line_width, render_line},
    terminal::{
        Input, Terminal, WaitEvent, poll_fd, read_input, read_prompt_input, wait_for_event,
    },
    text::{Line, sanitize_terminal_text},
};

pub(crate) struct App {
    lines: Vec<Line>,
    name: String,
    notice: Option<String>,
    max_line_width: usize,
    top: usize,
    left: usize,
    search: Option<String>,
    matches: Vec<usize>,
    match_idx: usize,
    last: Option<View>,
    last_rows: Vec<String>,
    last_status: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct View {
    top: usize,
    left: usize,
    w: u16,
    h: u16,
    search_rev: u64,
}

impl App {
    pub(crate) fn new(lines: Vec<Line>, name: String, notice: Option<String>) -> Self {
        let max_line_width = lines.iter().map(line_width).max().unwrap_or(0);
        Self {
            lines,
            name: sanitize_terminal_text(&name),
            notice: notice.map(|text| sanitize_terminal_text(&text)),
            max_line_width,
            top: 0,
            left: 0,
            search: None,
            matches: Vec::new(),
            match_idx: 0,
            last: None,
            last_rows: Vec::new(),
            last_status: String::new(),
        }
    }

    pub(crate) fn run(&mut self) -> Result<()> {
        let mut out = io::stdout();
        let mut terminal = Terminal::enter(&mut out)?;
        let resize_fd = terminal.resize_fd();
        self.event_loop(&mut out, &mut terminal.tty, resize_fd)
    }

    fn event_loop(
        &mut self,
        out: &mut io::Stdout,
        tty: &mut fs::File,
        resize_fd: i32,
    ) -> Result<()> {
        self.draw(out)?;
        loop {
            let before = self.state_key();
            let mut dirty = false;
            if let WaitEvent::Input = wait_for_event(tty.as_raw_fd(), resize_fd)? {
                let input = read_input(tty)?;
                let (cont, d) = self.handle_input(out, tty, input, resize_fd)?;
                dirty = d;
                if !cont {
                    break;
                }
                while poll_fd(tty.as_raw_fd(), Duration::from_millis(0))? {
                    let input = read_input(tty)?;
                    let (cont, d) = self.handle_input(out, tty, input, resize_fd)?;
                    dirty |= d;
                    if !cont {
                        return Ok(());
                    }
                }
            }
            self.clamp_view();
            if dirty || self.state_key() != before {
                self.draw(out)?;
            }
        }
        Ok(())
    }

    fn handle_input(
        &mut self,
        out: &mut io::Stdout,
        tty: &mut fs::File,
        input: Input,
        resize_fd: i32,
    ) -> Result<(bool, bool)> {
        let mut dirty = false;
        if self.cycle_match_for_direction(&input) {
            return Ok((true, false));
        }
        match input {
            Input::Quit => return Ok((false, false)),
            Input::Down => self.scroll(1),
            Input::Up => self.top = self.top.saturating_sub(1),
            Input::PageDown => self.scroll(page_height() / 2),
            Input::PageUp => self.top = self.top.saturating_sub(page_height() / 2),
            Input::Home => self.top = 0,
            Input::End => self.top = self.max_top(),
            Input::Right => self.left = (self.left + 4).min(self.max_left()),
            Input::Left => self.left = self.left.saturating_sub(4),
            Input::PageRight => {
                self.left =
                    (self.left + (Terminal::size().0 as usize / 2).max(1)).min(self.max_left());
            }
            Input::PageLeft => {
                self.left = self
                    .left
                    .saturating_sub((Terminal::size().0 as usize / 2).max(1));
            }
            Input::LineStart => self.left = 0,
            Input::LineEnd => self.left = self.max_left(),
            Input::Search => {
                self.prompt_search(out, tty, resize_fd)?;
                dirty = true;
            }
            Input::Next => self.next_match(false),
            Input::Prev => self.next_match(true),
            _ => {}
        }
        Ok((true, dirty))
    }

    fn scroll(&mut self, n: usize) {
        self.top = (self.top + n).min(self.max_top());
    }

    fn max_top(&self) -> usize {
        let body_h = page_height();
        self.lines.len().saturating_sub(body_h)
    }

    fn clamp_top(&mut self) {
        self.top = self.top.min(self.max_top());
    }

    fn max_left(&self) -> usize {
        self.max_line_width
            .saturating_sub(Terminal::size().0 as usize)
    }

    fn clamp_left(&mut self) {
        self.left = self.left.min(self.max_left());
    }

    fn clamp_view(&mut self) {
        self.clamp_top();
        self.clamp_left();
    }

    fn prompt_search(
        &mut self,
        out: &mut io::Stdout,
        tty: &mut fs::File,
        resize_fd: i32,
    ) -> Result<()> {
        let mut prompt_row = Terminal::size().1;
        let mut q = String::new();
        loop {
            let (_, current_row) = Terminal::size();
            if current_row != prompt_row {
                write!(out, "\x1b[{};1H\x1b[2K", prompt_row)?;
                prompt_row = current_row;
            }
            write!(out, "\x1b[{};1H\x1b[2K/{q}", prompt_row)?;
            out.flush()?;
            if let WaitEvent::Resize = wait_for_event(tty.as_raw_fd(), resize_fd)? {
                continue;
            }
            match read_prompt_input(tty)? {
                Input::Char('\n') => break,
                Input::Quit => return Ok(()),
                Input::Char('\x08') => {
                    q.pop();
                }
                Input::Char(c) if !c.is_control() => q.push(c),
                _ => {}
            }
        }
        if !q.is_empty() {
            self.search = Some(q);
            self.rebuild_matches();
            self.match_idx = 0;
            if let Some(&m) = self.matches.first() {
                self.top = m;
            }
            self.clamp_top();
        }
        Ok(())
    }

    fn rebuild_matches(&mut self) {
        self.matches.clear();
        let Some(q) = &self.search else {
            return;
        };
        for (i, line) in self.lines.iter().enumerate() {
            if line_matches(line, q) {
                self.matches.push(i);
            }
        }
    }

    fn cycle_match_for_direction(&mut self, input: &Input) -> bool {
        let prev = match input {
            Input::Up | Input::Left => true,
            Input::Down | Input::Right => false,
            _ => return false,
        };
        if self.matches.is_empty() {
            return false;
        }
        self.next_match(prev);
        true
    }

    fn next_match(&mut self, prev: bool) {
        if self.matches.is_empty() {
            return;
        }
        if prev {
            self.match_idx = self
                .match_idx
                .checked_sub(1)
                .unwrap_or(self.matches.len() - 1);
        } else {
            self.match_idx = (self.match_idx + 1) % self.matches.len();
        }
        self.top = self.matches[self.match_idx].min(self.max_top());
    }

    fn draw(&mut self, out: &mut io::Stdout) -> Result<()> {
        self.clamp_view();
        let (w, h) = Terminal::size();
        let width = w as usize;
        let body_h = h.saturating_sub(1) as usize;
        let view = View {
            top: self.top,
            left: self.left,
            w,
            h,
            search_rev: self.search_rev(),
        };
        let old = self.last;
        let mut frame = String::with_capacity(width.saturating_mul(h as usize + 1));

        frame.push_str("\x1b[?25l\x1b[?7l\x1b[r");
        if old.is_none_or(|o| o.w != view.w || o.h != view.h) {
            self.last_rows.clear();
            self.last_status.clear();
            frame.push_str("\x1b[2J");
        }
        if self.last_rows.len() != body_h {
            self.last_rows.resize(body_h, String::new());
        }
        for row in 0..body_h {
            let idx = self.top + row;
            let (rendered, _) = self.render_row(idx, width);
            if self.last_rows[row] != rendered {
                frame.push_str(&frame_row(row, &rendered));
                self.last_rows[row] = rendered;
            }
        }

        let (status, _) = self.render_status(width, h);
        if self.last_status != status {
            frame.push_str(&status);
            self.last_status = status;
        }
        out.write_all(frame.as_bytes())?;
        out.flush()?;
        self.last = Some(view);
        Ok(())
    }

    fn render_row(&self, idx: usize, width: usize) -> (String, usize) {
        if idx >= self.lines.len() {
            return (String::from("\x1b[0m"), 0);
        }
        let is_current_match = self.current_match_line() == Some(idx);
        render_line(
            &self.lines[idx],
            self.left,
            width,
            self.search.as_deref(),
            is_current_match,
        )
    }

    fn current_match_line(&self) -> Option<usize> {
        self.matches.get(self.match_idx).copied()
    }

    fn render_status(&self, width: usize, h: u16) -> (String, usize) {
        let body_h = h.saturating_sub(1) as usize;
        let total = self.lines.len();
        let first = if total == 0 {
            0
        } else {
            self.top.min(total - 1) + 1
        };
        let last = (self.top + body_h).min(total);
        let pct = (last * 100).checked_div(total).unwrap_or(100);
        let search = self
            .search
            .as_ref()
            .map(|s| {
                if self.matches.is_empty() {
                    format!(" /{s} [0]")
                } else {
                    format!(" /{s} [{}/{}]", self.match_idx + 1, self.matches.len())
                }
            })
            .unwrap_or_default();
        let notice = self
            .notice
            .as_ref()
            .map(|n| format!("  ⚠ {n}"))
            .unwrap_or_default();
        let first_col = (self.left + 1).min(self.max_line_width);
        let last_col = (self.left + width).min(self.max_line_width);
        let label = format!(
            "bless ✨  cols {}-{}/{}  {}  lines {}-{}/{}  {}%{}{}",
            first_col,
            last_col,
            self.max_line_width,
            self.name,
            first,
            last,
            total,
            pct,
            search,
            notice
        );
        let text = fit_width(&format!(" {label} "), width);
        let used = display_width(&text).min(width);
        (format!("\x1b[{};1H\x1b[0m\x1b[2K{}\x1b[0m", h, text), used)
    }

    fn search_rev(&self) -> u64 {
        let mut h = self.matches.len() as u64;
        h = h.wrapping_mul(16777619) ^ self.match_idx as u64;
        if let Some(s) = &self.search {
            for b in s.as_bytes() {
                h = h.wrapping_mul(16777619) ^ *b as u64;
            }
        }
        h
    }

    fn state_key(&self) -> (usize, usize, u64, (u16, u16)) {
        (self.top, self.left, self.search_rev(), Terminal::size())
    }
}

fn frame_row(row: usize, rendered: &str) -> String {
    format!("\x1b[{};1H\x1b[2K{}", row + 1, rendered)
}

fn page_height() -> usize {
    Terminal::size().1.saturating_sub(1) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::parse_ansi;

    #[test]
    fn changed_rows_are_cleared_before_redraw() {
        assert_eq!(frame_row(2, "short"), "\x1b[3;1H\x1b[2Kshort");
    }

    #[test]
    fn ansi_rows_use_styled_renderer_when_searching() -> Result<()> {
        let lines = parse_ansi("\x1b[32mneedle\x1b[0m")?;
        let mut app = App::new(lines, "test".into(), None);
        app.search = Some("needle".into());
        app.rebuild_matches();

        let (rendered, _) = app.render_row(0, 80);
        assert!(rendered.contains("\x1b[0;97;41mneedle"), "{rendered:?}");
        Ok(())
    }

    #[test]
    fn caches_width_and_sanitizes_status_metadata() -> Result<()> {
        let lines = parse_ansi("x\nabcdef")?;
        let app = App::new(lines, "bad\x1b]52;name".into(), Some("bell\x07".into()));
        assert_eq!(app.max_line_width, 6);
        let (status, _) = app.render_status(80, 24);
        assert!(status.contains("bad␛]52;name"), "{status:?}");
        assert!(status.contains("bell␇"), "{status:?}");
        assert!(!status.contains("\x1b]52"), "{status:?}");
        Ok(())
    }

    #[test]
    fn long_and_tabbed_ansi_rows_use_clipped_renderer() -> Result<()> {
        let lines = parse_ansi("\x1b[31mabcdef\x1b[0m")?;
        let app = App::new(lines, "test".into(), None);
        let (rendered, used) = app.render_row(0, 4);
        assert_eq!(used, 4);
        assert!(rendered.contains("abcd"), "{rendered:?}");
        assert!(!rendered.contains("abcdef"), "{rendered:?}");

        let lines = parse_ansi("a\tb")?;
        let app = App::new(lines, "test".into(), None);
        let (rendered, _) = app.render_row(0, 80);
        assert!(!rendered.contains('\t'), "{rendered:?}");
        Ok(())
    }

    #[test]
    fn horizontal_views_preserve_inherited_styles_and_links() -> Result<()> {
        use crate::text::{Style, TermColor};
        use libghostty_vt::style::Underline;

        let lines = parse_ansi(
            "\x1b[2;3;9;4:3;38:2::17:129:231;48;5;235;58:2::201:29:137mfirst\n\x1b]8;;https://example.com\x1b\\0123456789\x1b]8;;\x1b\\\x1b[0mplain",
        )?;
        let mut app = App::new(lines, "test".into(), None);
        let expected_style = Style {
            fg: Some(TermColor::Rgb(17, 129, 231)),
            bg: Some(TermColor::Ansi(235)),
            underline_color: Some(TermColor::Rgb(201, 29, 137)),
            faint: true,
            italic: true,
            underline: Some(Underline::Curly),
            strikethrough: true,
            ..Style::default()
        };
        for (left, width, expected) in [(0, 20, "0123456789plain"), (4, 4, "4567"), (8, 5, "89pla")]
        {
            app.left = left;
            let (rendered, used) = app.render_row(1, width);
            assert_eq!(used, expected.len());
            let visible = parse_ansi(&format!("{rendered}!"))?;
            assert_eq!(
                visible[0].iter().map(|cell| cell.ch).collect::<String>(),
                format!("{expected}!")
            );
            for (i, cell) in visible[0].iter().enumerate() {
                if i < expected.len() && left + i < 10 {
                    assert_eq!(cell.style, expected_style);
                    assert_eq!(cell.hyperlink.as_deref(), Some("https://example.com"));
                } else {
                    assert_eq!(cell.style, Style::default());
                    assert_eq!(cell.hyperlink, None);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn directional_keys_cycle_search_results_and_wrap() -> Result<()> {
        let input = (0..100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = App::new(parse_ansi(&input)?, "test".into(), None);
        app.matches = vec![10, 50, 90];

        assert!(app.cycle_match_for_direction(&Input::Down));
        assert_eq!(app.match_idx, 1);
        assert!(app.cycle_match_for_direction(&Input::Right));
        assert_eq!(app.match_idx, 2);
        assert!(app.cycle_match_for_direction(&Input::Down));
        assert_eq!(app.match_idx, 0);
        assert!(app.cycle_match_for_direction(&Input::Up));
        assert_eq!(app.match_idx, 2);
        assert!(app.cycle_match_for_direction(&Input::Left));
        assert_eq!(app.match_idx, 1);
        Ok(())
    }
}

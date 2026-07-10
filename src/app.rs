use std::{
    fs,
    io::{self, Write},
    os::fd::AsRawFd,
    time::Duration,
};

use anyhow::Result;

use crate::{
    render::{char_width, display_width, fit_width, render_line},
    terminal::{poll_fd, read_input, Input, Terminal},
    text::Line,
};

pub(crate) struct App {
    lines: Vec<Line>,
    raw_lines: Option<Vec<String>>,
    name: String,
    notice: Option<String>,
    top: usize,
    left: usize,
    search: Option<String>,
    matches: Vec<usize>,
    match_idx: usize,
    last: Option<View>,
    last_rows: Vec<String>,
    last_status: String,
    last_widths: Vec<usize>,
    last_status_width: usize,
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
    pub(crate) fn new(
        lines: Vec<Line>,
        raw_lines: Option<Vec<String>>,
        name: String,
        notice: Option<String>,
    ) -> Self {
        Self {
            lines,
            raw_lines,
            name,
            notice,
            top: 0,
            left: 0,
            search: None,
            matches: Vec::new(),
            match_idx: 0,
            last: None,
            last_rows: Vec::new(),
            last_status: String::new(),
            last_widths: Vec::new(),
            last_status_width: 0,
        }
    }

    pub(crate) fn run(&mut self) -> Result<()> {
        let mut out = io::stdout();
        let mut terminal = Terminal::enter(&mut out)?;
        self.event_loop(&mut out, &mut terminal.tty)
    }

    fn event_loop(&mut self, out: &mut io::Stdout, tty: &mut fs::File) -> Result<()> {
        self.draw(out)?;
        loop {
            let before = self.state_key();
            let input = read_input(tty)?;
            let (cont, mut dirty) = self.handle_input(out, tty, input)?;
            if !cont {
                break;
            }
            while poll_fd(tty.as_raw_fd(), Duration::from_millis(0))? {
                let input = read_input(tty)?;
                let (cont, d) = self.handle_input(out, tty, input)?;
                dirty |= d;
                if !cont {
                    return Ok(());
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
    ) -> Result<(bool, bool)> {
        let mut dirty = false;
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
            Input::Search => {
                self.prompt_search(out, tty)?;
                dirty = true;
            }
            Input::Next => self.next_match(false),
            Input::Prev => self.next_match(true),
            // BEL and other ignored/control input should not cause a repaint.
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
        let width = Terminal::size().0 as usize;
        self.lines
            .iter()
            .map(|line| line.iter().map(|c| char_width(c.ch)).sum::<usize>())
            .max()
            .unwrap_or(0)
            .saturating_sub(width)
    }

    fn clamp_left(&mut self) {
        self.left = self.left.min(self.max_left());
    }

    fn clamp_view(&mut self) {
        self.clamp_top();
        self.clamp_left();
    }

    fn prompt_search(&mut self, out: &mut io::Stdout, tty: &mut fs::File) -> Result<()> {
        let (_, h) = Terminal::size();
        let mut q = String::new();
        loop {
            write!(out, "\x1b[{};1H\x1b[2K/{q}", h)?;
            out.flush()?;
            match read_input(tty)? {
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
        let q = q.to_lowercase();
        for (i, l) in self.lines.iter().enumerate() {
            let s: String = l.iter().map(|c| c.ch).collect();
            if s.to_lowercase().contains(&q) {
                self.matches.push(i);
            }
        }
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

        // Damage rendering. No per-frame synchronized-update toggling and no
        // unconditional erase-to-EOL: both can look like flicker in Ghostty.
        frame.push_str("\x1b[?25l\x1b[?7l\x1b[r");
        if old.is_none_or(|o| o.w != view.w || o.h != view.h) {
            self.last_rows.clear();
            self.last_status.clear();
            self.last_widths.clear();
            self.last_status_width = 0;
            frame.push_str("\x1b[2J");
        }
        if self.last_rows.len() != body_h {
            self.last_rows.resize(body_h, String::new());
        }
        if self.last_widths.len() != body_h {
            self.last_widths.resize(body_h, 0);
        }
        for row in 0..body_h {
            let idx = self.top + row;
            let (rendered, used) = self.render_row(idx, width);
            if self.last_rows[row] != rendered {
                frame.push_str(&format!("\x1b[{};1H{}", row + 1, rendered));
                if self.last_widths[row] > used {
                    frame.push_str("\x1b[K");
                }
                self.last_rows[row] = rendered;
                self.last_widths[row] = used;
            }
        }

        let (status, status_width) = self.render_status(width, h);
        if self.last_status != status {
            frame.push_str(&status);
            if self.last_status_width > status_width {
                frame.push_str("\x1b[K");
            }
            self.last_status = status;
            self.last_status_width = status_width;
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
        if self.left == 0 {
            if let Some(raw) = self.raw_lines.as_ref().and_then(|r| r.get(idx)) {
                // For already-colored command output, replay sanitized SGR bytes
                // verbatim so Ghostty renders exactly as it would without bless.
                let used = self.lines[idx]
                    .iter()
                    .map(|c| char_width(c.ch))
                    .sum::<usize>()
                    .min(width);
                return (format!("\x1b[0m{}\x1b[0m", raw), used);
            }
        }
        render_line(&self.lines[idx], self.left, width, self.search.as_deref())
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
        let pct = if total == 0 {
            100
        } else {
            (last * 100) / total
        };
        let search = self
            .search
            .as_ref()
            .map(|s| format!(" /{s} [{}]", self.matches.len()))
            .unwrap_or_default();
        let notice = self
            .notice
            .as_ref()
            .map(|n| format!("  ⚠ {n}"))
            .unwrap_or_default();
        let label = format!(
            "bless ✨  {}  lines {}-{}/{}  {}%{}{}",
            self.name, first, last, total, pct, search, notice
        );
        let text = fit_width(&format!(" {label} "), width);
        let used = display_width(&text).min(width);
        // Floating transparent status: reset to Ghostty's default fg/bg, clear
        // the row back to terminal background, then draw only the label with
        // one column of horizontal padding.
        (format!("\x1b[{};1H\x1b[0m\x1b[2K{}\x1b[0m", h, text), used)
    }

    fn search_rev(&self) -> u64 {
        let mut h = self.matches.len() as u64;
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

fn page_height() -> usize {
    Terminal::size().1.saturating_sub(1) as usize
}

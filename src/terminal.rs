use std::{
    fs,
    fs::OpenOptions,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    time::Duration,
};

use anyhow::{bail, Context, Result};

#[derive(Debug)]
pub(crate) enum Input {
    Quit,
    Down,
    Up,
    PageDown,
    PageUp,
    Home,
    End,
    Left,
    Right,
    Search,
    Next,
    Prev,
    Char(char),
    Ignore,
}

pub(crate) struct Terminal {
    original: libc::termios,
    pub(crate) tty: fs::File,
}

impl Terminal {
    pub(crate) fn enter(out: &mut io::Stdout) -> Result<Self> {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening /dev/tty for interactive input")?;
        let fd = tty.as_raw_fd();
        let mut original = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            bail!("tcgetattr failed");
        }
        let mut raw = original;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
        raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
        raw.c_cflag |= libc::CS8;
        raw.c_oflag &= !libc::OPOST;
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            bail!("tcsetattr failed");
        }
        write!(
            out,
            "\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?25l\x1b[2J\x1b[?7l"
        )?;
        out.flush()?;
        Ok(Self { original, tty })
    }

    pub(crate) fn size() -> (u16, u16) {
        let mut ws = unsafe { std::mem::zeroed::<libc::winsize>() };
        if unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut ws) } == 0
            && ws.ws_col > 0
            && ws.ws_row > 0
        {
            (ws.ws_col, ws.ws_row)
        } else {
            (80, 24)
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(self.tty.as_raw_fd(), libc::TCSAFLUSH, &self.original) };
        let _ = write!(
            io::stdout(),
            "\x1b[?7h\x1b[r\x1b[?25h\x1b[?1006l\x1b[?1000l\x1b[?1049l"
        );
    }
}

pub(crate) fn read_input(tty: &mut fs::File) -> Result<Input> {
    let mut b = [0u8; 1];
    tty.read_exact(&mut b)?;
    Ok(match b[0] {
        b'q' | 0x1b => {
            if b[0] == 0x1b && poll_fd(tty.as_raw_fd(), Duration::from_millis(1))? {
                read_escape(tty)?
            } else {
                Input::Quit
            }
        }
        3 => Input::Quit,
        b'j' => Input::Down,
        b'k' => Input::Up,
        b'd' | b' ' => Input::PageDown,
        b'u' => Input::PageUp,
        b'g' => Input::Home,
        b'G' => Input::End,
        b'h' => Input::Left,
        b'l' => Input::Right,
        b'/' => Input::Search,
        b'n' => Input::Next,
        b'N' => Input::Prev,
        b'\r' | b'\n' => Input::Char('\n'),
        0x7f => Input::Char('\x08'),
        c if c.is_ascii() => Input::Char(c as char),
        _ => Input::Ignore,
    })
}

/// Reads a single byte as raw text-entry input for prompts (e.g. search),
/// without the normal-mode vim navigation bindings from `read_input`, so
/// query text can contain any character, including g/h/j/k/l/n/N/q/d/u.
pub(crate) fn read_prompt_input(tty: &mut fs::File) -> Result<Input> {
    let mut b = [0u8; 1];
    tty.read_exact(&mut b)?;
    Ok(match b[0] {
        3 => Input::Quit,
        0x1b => {
            if poll_fd(tty.as_raw_fd(), Duration::from_millis(1))? {
                // Part of an escape sequence (e.g. an arrow key): the prompt
                // has no in-line cursor movement, so just consume and drop it.
                read_escape(tty)?;
                Input::Ignore
            } else {
                Input::Quit
            }
        }
        b'\r' | b'\n' => Input::Char('\n'),
        0x7f => Input::Char('\x08'),
        c if c.is_ascii() => Input::Char(c as char),
        _ => Input::Ignore,
    })
}

fn read_escape(tty: &mut fs::File) -> Result<Input> {
    let mut seq = Vec::new();
    while poll_fd(tty.as_raw_fd(), Duration::from_millis(1))? {
        let mut b = [0u8; 1];
        tty.read_exact(&mut b)?;
        seq.push(b[0]);
        if b[0].is_ascii_alphabetic() || b[0] == b'~' || b[0] == b'M' {
            break;
        }
    }
    Ok(match seq.as_slice() {
        [b'[', b'A'] => Input::Up,
        [b'[', b'B'] => Input::Down,
        [b'[', b'C'] => Input::Right,
        [b'[', b'D'] => Input::Left,
        [b'[', b'5', b'~'] => Input::PageUp,
        [b'[', b'6', b'~'] => Input::PageDown,
        s if s.starts_with(b"[<64;") && s.ends_with(b"M") => Input::PageUp,
        s if s.starts_with(b"[<65;") && s.ends_with(b"M") => Input::PageDown,
        _ => Input::Ignore,
    })
}

pub(crate) fn poll_fd(fd: i32, timeout: Duration) -> Result<bool> {
    // select(2), not poll(2): Darwin's poll() is broken on character devices
    // (returns immediately with POLLNVAL on /dev/tty), which made the input
    // drain loop block on the next keypress before repainting. select() works
    // correctly on both macOS and Linux. The tty fd is always a small number,
    // well below FD_SETSIZE.
    let mut set = unsafe { std::mem::zeroed::<libc::fd_set>() };
    unsafe {
        libc::FD_ZERO(&mut set);
        libc::FD_SET(fd, &mut set);
    }
    let mut tv = libc::timeval {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    let rc = unsafe {
        libc::select(
            fd + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut tv,
        )
    };
    if rc < 0 {
        bail!("select failed");
    }
    Ok(rc > 0 && unsafe { libc::FD_ISSET(fd, &set) })
}

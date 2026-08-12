use std::{
    fs,
    fs::OpenOptions,
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::atomic::{AtomicI32, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail};

static RESIZE_PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn handle_sigwinch(_: libc::c_int) {
    let fd = RESIZE_PIPE_WRITE.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = 0u8;
        unsafe {
            libc::write(fd, (&raw const byte).cast(), 1);
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
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
    resize_read_fd: i32,
    resize_write_fd: i32,
    old_sigwinch: Option<libc::sigaction>,
}

fn raw_termios(original: libc::termios) -> libc::termios {
    let mut raw = original;
    raw.c_lflag &= !(libc::ECHO | libc::ICANON | libc::IEXTEN | libc::ISIG);
    raw.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
    raw.c_cflag |= libc::CS8;
    raw.c_oflag &= !libc::OPOST;
    raw.c_cc[libc::VMIN] = 1;
    raw.c_cc[libc::VTIME] = 0;
    raw
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
        let raw = raw_termios(original);
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            bail!("tcsetattr failed");
        }

        let mut terminal = Self {
            original,
            tty,
            resize_read_fd: -1,
            resize_write_fd: -1,
            old_sigwinch: None,
        };
        let (resize_read_fd, resize_write_fd, old_sigwinch) = install_resize_signal()?;
        terminal.resize_read_fd = resize_read_fd;
        terminal.resize_write_fd = resize_write_fd;
        terminal.old_sigwinch = Some(old_sigwinch);
        write!(
            out,
            "\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?25l\x1b[2J\x1b[?7l"
        )?;
        out.flush()?;
        Ok(terminal)
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

    pub(crate) fn resize_fd(&self) -> i32 {
        self.resize_read_fd
    }
}

fn validate_select_fd(fd: i32) -> Result<()> {
    if fd < 0 || fd as usize >= libc::FD_SETSIZE {
        bail!("file descriptor {fd} exceeds select() capacity");
    }
    Ok(())
}

fn install_resize_signal() -> Result<(i32, i32, libc::sigaction)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        bail!("pipe failed: {}", io::Error::last_os_error());
    }
    for fd in fds {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0 {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
            bail!("configuring resize pipe failed: {error}");
        }
    }
    RESIZE_PIPE_WRITE.store(fds[1], Ordering::Relaxed);

    let mut sa: libc::sigaction = unsafe { std::mem::zeroed() };
    sa.sa_sigaction = handle_sigwinch as *const () as usize;
    unsafe { libc::sigemptyset(&mut sa.sa_mask) };
    sa.sa_flags = 0;
    let mut old_sa: libc::sigaction = unsafe { std::mem::zeroed() };
    if unsafe { libc::sigaction(libc::SIGWINCH, &sa, &mut old_sa) } != 0 {
        RESIZE_PIPE_WRITE.store(-1, Ordering::Relaxed);
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        bail!("sigaction failed: {}", io::Error::last_os_error());
    }
    Ok((fds[0], fds[1], old_sa))
}

fn drain_resize_pipe(fd: i32) {
    let mut buf = [0u8; 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum WaitEvent {
    Input,
    Resize,
}

pub(crate) fn wait_for_event(tty_fd: i32, resize_fd: i32) -> Result<WaitEvent> {
    validate_select_fd(tty_fd)?;
    validate_select_fd(resize_fd)?;
    loop {
        let mut set = unsafe { std::mem::zeroed::<libc::fd_set>() };
        unsafe {
            libc::FD_ZERO(&mut set);
            libc::FD_SET(tty_fd, &mut set);
            libc::FD_SET(resize_fd, &mut set);
        }
        let nfds = tty_fd.max(resize_fd) + 1;
        let rc = unsafe {
            libc::select(
                nfds,
                &mut set,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if rc < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            bail!("select failed: {}", io::Error::last_os_error());
        }
        let resized = unsafe { libc::FD_ISSET(resize_fd, &set) };
        if resized {
            drain_resize_pipe(resize_fd);
        }
        if unsafe { libc::FD_ISSET(tty_fd, &set) } {
            return Ok(WaitEvent::Input);
        }
        if resized {
            return Ok(WaitEvent::Resize);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = unsafe { libc::tcsetattr(self.tty.as_raw_fd(), libc::TCSAFLUSH, &self.original) };
        RESIZE_PIPE_WRITE.store(-1, Ordering::Relaxed);
        unsafe {
            if let Some(old_sa) = &self.old_sigwinch {
                libc::sigaction(libc::SIGWINCH, old_sa, std::ptr::null_mut());
            }
            if self.resize_read_fd >= 0 {
                libc::close(self.resize_read_fd);
            }
            if self.resize_write_fd >= 0 {
                libc::close(self.resize_write_fd);
            }
        }
        let mut out = io::stdout();
        let _ = write!(
            out,
            "\x1b[?7h\x1b[r\x1b[?25h\x1b[?1006l\x1b[?1000l\x1b[?1049l"
        );
        let _ = out.flush();
    }
}

pub(crate) fn read_input(tty: &mut fs::File) -> Result<Input> {
    let mut b = [0u8; 1];
    tty.read_exact(&mut b)?;
    Ok(match b[0] {
        b'q' | 0x1b => {
            if b[0] == 0x1b && poll_fd(tty.as_raw_fd(), Duration::from_millis(25))? {
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
        c => read_utf8_char(tty, c)?.map_or(Input::Ignore, Input::Char),
    })
}

pub(crate) fn read_prompt_input(tty: &mut fs::File) -> Result<Input> {
    let mut b = [0u8; 1];
    tty.read_exact(&mut b)?;
    Ok(match b[0] {
        3 => Input::Quit,
        0x1b => {
            if poll_fd(tty.as_raw_fd(), Duration::from_millis(25))? {
                read_escape(tty)?;
                Input::Ignore
            } else {
                Input::Quit
            }
        }
        b'\r' | b'\n' => Input::Char('\n'),
        0x7f => Input::Char('\x08'),
        c if c.is_ascii() => Input::Char(c as char),
        c => read_utf8_char(tty, c)?.map_or(Input::Ignore, Input::Char),
    })
}

fn read_utf8_char<R: Read>(reader: &mut R, first: u8) -> io::Result<Option<char>> {
    let len = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Ok(None),
    };
    let mut bytes = [0u8; 4];
    bytes[0] = first;
    reader.read_exact(&mut bytes[1..len])?;
    Ok(std::str::from_utf8(&bytes[..len])
        .ok()
        .and_then(|text| text.chars().next()))
}

fn read_escape(tty: &mut fs::File) -> Result<Input> {
    let mut seq = Vec::new();
    while seq.len() < 32 && poll_fd(tty.as_raw_fd(), Duration::from_millis(10))? {
        let mut b = [0u8; 1];
        match tty.read(&mut b) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
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
    validate_select_fd(fd)?;
    loop {
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
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            bail!("select failed: {}", io::Error::last_os_error());
        }
        return Ok(rc > 0 && unsafe { libc::FD_ISSET(fd, &set) });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{Cursor, Write},
        os::fd::{AsRawFd, FromRawFd},
        thread,
        time::Duration,
    };

    use super::*;

    fn pty_pair() -> io::Result<(File, File)> {
        let mut master = -1;
        let mut slave = -1;
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) })
    }

    fn get_termios(file: &File) -> io::Result<libc::termios> {
        let mut attrs = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut attrs) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(attrs)
    }

    fn raw_pty() -> io::Result<(File, File)> {
        let (master, slave) = pty_pair()?;
        let attrs = raw_termios(get_termios(&slave)?);
        if unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &attrs) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((master, slave))
    }

    fn nonblocking_pipe() -> io::Result<(File, File)> {
        let mut fds = [-1; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = unsafe { libc::fcntl(fds[0], libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) })
    }

    #[test]
    fn decodes_utf8_prompt_characters() {
        let mut hebrew = Cursor::new("ב".as_bytes()[1..].to_vec());
        assert_eq!(
            read_utf8_char(&mut hebrew, "ב".as_bytes()[0]).unwrap(),
            Some('ב')
        );

        let mut emoji = Cursor::new("🙂".as_bytes()[1..].to_vec());
        assert_eq!(
            read_utf8_char(&mut emoji, "🙂".as_bytes()[0]).unwrap(),
            Some('🙂')
        );
    }

    #[test]
    fn rejects_invalid_utf8_leads_and_oversized_select_fds() {
        assert_eq!(
            read_utf8_char(&mut Cursor::new(Vec::<u8>::new()), 0x80).unwrap(),
            None
        );
        assert!(validate_select_fd(libc::FD_SETSIZE as i32).is_err());
    }

    #[test]
    fn resize_pipe_wakes_event_loop() -> Result<()> {
        let (tty_read, _tty_write) = nonblocking_pipe()?;
        let (resize_read, mut resize_write) = nonblocking_pipe()?;
        resize_write.write_all(&[1])?;
        assert_eq!(
            wait_for_event(tty_read.as_raw_fd(), resize_read.as_raw_fd())?,
            WaitEvent::Resize
        );
        Ok(())
    }

    #[test]
    fn signal_wakes_loop_and_drop_restores_pty_mode() -> Result<()> {
        let (_master, tty) = pty_pair()?;
        let observer = tty.try_clone()?;
        let original = get_termios(&tty)?;
        let raw = raw_termios(original);
        if unsafe { libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        let tty_fd = tty.as_raw_fd();
        let (resize_read_fd, resize_write_fd, old_sigwinch) = install_resize_signal()?;
        let terminal = Terminal {
            original,
            tty,
            resize_read_fd,
            resize_write_fd,
            old_sigwinch: Some(old_sigwinch),
        };

        unsafe {
            libc::raise(libc::SIGWINCH);
        }
        assert_eq!(
            wait_for_event(tty_fd, terminal.resize_fd())?,
            WaitEvent::Resize
        );
        drop(terminal);

        let restored = get_termios(&observer)?;
        assert_eq!(restored.c_iflag, original.c_iflag);
        assert_eq!(restored.c_oflag, original.c_oflag);
        assert_eq!(restored.c_cflag, original.c_cflag);
        assert_eq!(restored.c_lflag, original.c_lflag);
        assert_eq!(restored.c_cc, original.c_cc);
        Ok(())
    }

    #[test]
    fn pty_prompt_reads_utf8() -> Result<()> {
        let (mut master, mut slave) = raw_pty()?;
        master.write_all("ב".as_bytes())?;
        assert_eq!(read_prompt_input(&mut slave)?, Input::Char('ב'));
        Ok(())
    }

    #[test]
    fn pty_accepts_fragmented_arrow_sequence() -> Result<()> {
        let (mut master, mut slave) = raw_pty()?;
        let writer = thread::spawn(move || -> io::Result<()> {
            master.write_all(b"\x1b")?;
            master.flush()?;
            thread::sleep(Duration::from_millis(5));
            master.write_all(b"[A")?;
            master.flush()?;
            thread::sleep(Duration::from_millis(50));
            Ok(())
        });
        assert_eq!(read_input(&mut slave)?, Input::Up);
        match writer.join() {
            Ok(result) => result?,
            Err(_) => return Err(anyhow::anyhow!("PTY writer thread panicked")),
        }
        Ok(())
    }
}

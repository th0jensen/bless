use std::{
    cell::{Cell, RefCell},
    env,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::{CommandExt, ExitStatusExt},
    },
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicI32, Ordering},
};

use anyhow::{Context, Result, bail};

use crate::{
    cli::human_bytes,
    shell::spawning_shell,
    text::{Line, snapshot_lines, vt_terminal},
};

static CAPTURE_SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn interrupt_capture(signal: libc::c_int) {
    CAPTURE_SIGNAL.store(signal, Ordering::Relaxed);
}

pub(crate) struct Captured {
    pub(crate) lines: Vec<Line>,
    pub(crate) notice: Option<String>,
    pub(crate) code: u8,
    pub(crate) interrupted: bool,
}

pub(crate) fn capture(args: &[String], cols: u16, rows: u16, limit: u64) -> Result<Captured> {
    let (program, arguments) = args.split_first().context("missing command")?;
    let shell = spawning_shell();
    let cols = cols.max(1);
    let _signals = CaptureSignals::install()?;
    let (mut master, slave) = open_pty(cols, rows.max(1))?;
    let command = if arguments.is_empty() {
        // Only an explicitly supplied command string is shell source. Joining
        // argv would lose quoting and reinterpret literal arguments as code.
        let mut command = Command::new(&shell);
        // A leading newline keeps command text such as --help from being
        // interpreted as another option by the shell itself.
        command.arg("-c").arg(format!("\n{program}"));
        command
    } else {
        let mut command = Command::new(program);
        command.args(arguments);
        command
    };
    let child = spawn_command(command, &slave, &shell, cols, rows);
    let child = match child {
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && !arguments.is_empty()
                && shell.file_name().is_some_and(|name| name == "pwsh") =>
        {
            // Keep native executables on the lossless argv path. PowerShell
            // aliases/cmdlets need its binder, including named-parameter metadata
            // preserved by CommandWithArgs. Never interpolate argv into source.
            let mut command = Command::new(&shell);
            command
                .arg("-CommandWithArgs")
                .arg("$command = $args[0]; $tail = @(); if ($args.Count -gt 1) { $tail = $args[1..($args.Count - 1)] }; & $command @tail")
                .arg(program)
                .args(arguments);
            spawn_command(command, &slave, &shell, cols, rows)
        }
        result => result,
    };
    let mut child = CapturedChild {
        child: child.with_context(|| format!("running {program}"))?,
        reaped: false,
    };
    drop(slave);

    let replies = RefCell::new(Vec::new());
    let reply_overflow = Cell::new(false);
    let mut terminal = vt_terminal(cols, rows)?;
    terminal.on_pty_write(|_, bytes| {
        let mut pending = replies.borrow_mut();
        if pending.len().saturating_add(bytes.len()) > 64 * 1024 {
            reply_overflow.set(true);
        } else {
            pending.extend_from_slice(bytes);
        }
    })?;
    let mut loaded = 0;
    let mut capped = false;
    let mut buf = [0; 16 * 1024];
    loop {
        if CAPTURE_SIGNAL.load(Ordering::Relaxed) != 0 {
            break;
        }
        let mut fd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN
                | if replies.borrow().is_empty() {
                    0
                } else {
                    libc::POLLOUT
                },
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, 100) } < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::last_os_error()).context("polling command PTY");
        }
        if fd.revents & libc::POLLNVAL != 0 {
            bail!("command PTY closed unexpectedly");
        }
        if !child.reaped && fd.revents & libc::POLLHUP == 0 && child.exited()? {
            // A finished command's descendants can retain slave descriptors.
            // Stop them, then drain the PTY instead of waiting indefinitely.
            child.stop(libc::SIGHUP)?;
        }
        if fd.revents & libc::POLLOUT != 0 {
            let mut pending = replies.borrow_mut();
            match master.write(&pending) {
                Ok(n) => {
                    pending.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if matches!(e.raw_os_error(), Some(libc::EIO | libc::EPIPE)) => {
                    pending.clear();
                }
                Err(e) => return Err(e).context("answering command terminal query"),
            }
        }
        if fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
            continue;
        }
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let keep = (limit - loaded).min(n as u64) as usize;
                terminal.vt_write(&buf[..keep]);
                loaded += keep as u64;
                if reply_overflow.get() {
                    bail!("command produced too many unanswered terminal queries");
                }
                if keep < n {
                    capped = true;
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            // Linux PTY masters report EIO after the final slave closes;
            // other Unix systems may return zero instead.
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) => return Err(e).context("reading command PTY"),
        }
    }
    let status = if capped {
        child.stop(libc::SIGTERM)?
    } else {
        child.wait()?
    };
    let lines = if CAPTURE_SIGNAL.load(Ordering::Relaxed) != 0 {
        Vec::new()
    } else {
        snapshot_lines(&mut terminal)?
    };
    let signal = CAPTURE_SIGNAL.load(Ordering::Relaxed);
    let code = if signal != 0 {
        128 + signal
    } else {
        status.code().unwrap_or(128 + status.signal().unwrap_or(0))
    } as u8;
    let notice = if capped {
        Some(format!(
            "output capped at {}; command stopped",
            human_bytes(limit)
        ))
    } else if !status.success() {
        Some(format!("command exited with status {code}"))
    } else {
        None
    };
    Ok(Captured {
        lines,
        notice,
        code,
        interrupted: signal != 0,
    })
}

fn spawn_command(
    mut command: Command,
    slave: &File,
    shell: &Path,
    cols: u16,
    rows: u16,
) -> io::Result<Child> {
    command
        .stdin(if unsafe { libc::isatty(0) } == 1 {
            Stdio::from(slave.try_clone()?)
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave.try_clone()?))
        .env("SHELL", shell)
        .env("COLUMNS", cols.to_string())
        .env("LINES", rows.max(1).to_string())
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat");
    if env::var_os("TERM").is_none() {
        command.env("TERM", "xterm-256color");
    }
    // Only async-signal-safe calls belong between fork and exec. Command has
    // already attached stdout to the slave before running this closure.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(1, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // Drop Command's configured slave descriptors on return, before reading the
    // master. Keeping any parent-owned slave descriptor would prevent PTY EOF.
    command.spawn()
}

fn open_pty(cols: u16, rows: u16) -> Result<(File, File)> {
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_col: cols,
        ws_row: rows,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error()).context("opening command PTY");
    }
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error()).context("configuring command PTY descriptor");
        }
    }
    let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error()).context("configuring command PTY reads");
    }
    let mut attrs = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut attrs) } < 0 {
        return Err(io::Error::last_os_error()).context("reading command PTY attributes");
    }
    // Query responses must reach readers without echo or newline buffering.
    // Keep normal terminal output processing (including LF -> CRLF).
    let output_flags = attrs.c_oflag;
    unsafe { libc::cfmakeraw(&mut attrs) };
    attrs.c_oflag = output_flags;
    if unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &attrs) } < 0 {
        return Err(io::Error::last_os_error()).context("setting command PTY attributes");
    }
    Ok((master, slave))
}

struct CapturedChild {
    child: Child,
    reaped: bool,
}

impl CapturedChild {
    fn exited(&self) -> Result<bool> {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        // Keep the child unreaped so its process-group ID cannot be reused
        // before cleanup signals have been delivered.
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } < 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(error).context("checking command exit");
        }
        Ok(info.si_signo != 0)
    }

    fn stop(&mut self, signal: i32) -> Result<ExitStatus> {
        if self.reaped {
            return self.child.wait().context("reading command exit status");
        }
        unsafe {
            libc::kill(-(self.child.id() as i32), signal);
            libc::kill(-(self.child.id() as i32), libc::SIGCONT);
            libc::poll(std::ptr::null_mut(), 0, 100);
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let status = self.child.wait().context("waiting for stopped command")?;
        self.reaped = true;
        Ok(status)
    }

    fn wait(&mut self) -> Result<ExitStatus> {
        loop {
            let signal = CAPTURE_SIGNAL.load(Ordering::Relaxed);
            if signal != 0 {
                return self.stop(signal);
            }
            if let Some(status) = self.child.try_wait().context("waiting for command")? {
                self.reaped = true;
                return Ok(status);
            }
            unsafe { libc::poll(std::ptr::null_mut(), 0, 20) };
        }
    }
}

impl Drop for CapturedChild {
    fn drop(&mut self) {
        if !self.reaped {
            unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
            let _ = self.child.wait();
        }
    }
}

struct CaptureSignals(Vec<(libc::c_int, libc::sigaction)>);

impl CaptureSignals {
    fn install() -> Result<Self> {
        CAPTURE_SIGNAL.store(0, Ordering::Relaxed);
        let mut guard = Self(Vec::new());
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
            action.sa_sigaction = interrupt_capture as *const () as usize;
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            let mut old = unsafe { std::mem::zeroed::<libc::sigaction>() };
            if unsafe { libc::sigaction(signal, &action, &mut old) } < 0 {
                return Err(io::Error::last_os_error())
                    .context("installing command signal handler");
            }
            guard.0.push((signal, old));
        }
        Ok(guard)
    }
}

impl Drop for CaptureSignals {
    fn drop(&mut self) {
        for (signal, old) in &self.0 {
            unsafe { libc::sigaction(*signal, old, std::ptr::null_mut()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TermColor;

    fn plain(captured: &Captured) -> Vec<String> {
        captured
            .lines
            .iter()
            .map(|line| line.iter().map(|cell| cell.ch).collect())
            .collect()
    }

    #[test]
    fn captures_tty_colors_arguments_queries_and_exit_status() -> Result<()> {
        let args = [
            "sh",
            "-c",
            r#"[ -t 1 ] && [ -t 2 ] || exit 11
printf '\033[32m%s\033[0m\n' "$1"
printf 'stderr\n' >&2
stty size </dev/tty
printf '%s|%s\n' "$PAGER" "$GIT_PAGER"
exit 7"#,
            "bless-test",
            "$literal; space",
        ]
        .map(String::from);
        let captured = capture(&args, 160, 12, 1024)?;
        assert_eq!(
            plain(&captured),
            ["$literal; space", "stderr", "12 160", "cat|cat"]
        );
        assert_eq!(captured.code, 7);
        assert_eq!(
            captured.notice.as_deref(),
            Some("command exited with status 7")
        );
        assert!(
            captured.lines[0]
                .iter()
                .all(|cell| cell.style.fg == Some(TermColor::Ansi(2)))
        );

        let args = [
            "sh",
            "-c",
            r#"exec 3<>/dev/tty
stty -icanon -echo min 0 time 5 <&3
printf 'abc\033[6n'
reply=$(dd bs=1 count=6 <&3 2>/dev/null)
[ "$reply" = "$(printf '\033[1;4R')" ] || exit 13
printf '|reply-ok'"#,
        ]
        .map(String::from);
        let captured = capture(&args, 80, 12, 1024)?;
        assert_eq!(captured.code, 0);
        assert_eq!(plain(&captured), ["abc|reply-ok"]);

        let args = ["sh", "-c", "printf abcdef; sleep 10"].map(String::from);
        let captured = capture(&args, 80, 12, 4)?;
        assert_eq!(plain(&captured), ["abcd"]);
        assert_ne!(captured.code, 0);
        assert!(captured.notice.as_deref().unwrap().contains("capped"));

        // An exited command must not leave capture waiting for background jobs.
        let args = ["sh", "-c", "trap '' HUP; sleep 10 & printf done"].map(String::from);
        let started = std::time::Instant::now();
        let captured = capture(&args, 80, 12, 1024)?;
        assert_eq!(plain(&captured), ["done"]);
        assert_eq!(captured.code, 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        Ok(())
    }
}

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Result, bail};

struct ShellChild(Child);

impl Drop for ShellChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            unsafe { libc::kill(-(self.0.id() as i32), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }
}

#[test]
fn library_pages_supplied_content_and_restores_terminal() -> Result<()> {
    if let Ok(mode) = std::env::var("BLESS_TEST_LIBRARY_INPUT") {
        let mut before = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(0, &mut before) }, 0);
        let text = "abc\x1b[31mRED\x1b[0m\n";
        if mode == "text" {
            bless::page_text("Log", text)?;
        } else {
            bless::page_reader("Log", io::Cursor::new(text.as_bytes()))?;
        }
        let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(0, &mut after) }, 0);
        assert_eq!(after.c_iflag, before.c_iflag);
        assert_eq!(after.c_oflag, before.c_oflag);
        assert_eq!(after.c_cflag, before.c_cflag);
        assert_eq!(after.c_lflag, before.c_lflag);
        assert_eq!(after.c_cc, before.c_cc);
        return Ok(());
    }

    for mode in ["text", "reader", "capped"] {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "library_pages_supplied_content_and_restores_terminal",
                "--nocapture",
            ])
            .env("BLESS_TEST_LIBRARY_INPUT", mode)
            .env_remove("BLESS_MAX_BYTES");
        if mode == "capped" {
            command.env("BLESS_MAX_BYTES", "3B");
        }
        let output = run_in_pty(command, 0)?;
        assert!(output.contains("Log"), "{output}");
        assert!(output.contains("abc"), "{output}");
        if mode == "capped" {
            assert!(output.contains("capped at 3 B"), "{output}");
            assert!(!output.contains("RED"), "{output}");
        } else {
            assert!(output.contains("\x1b[0;31mRED"), "{output}");
        }
        assert!(output.contains("\x1b[?1049l"), "{output}");
    }
    Ok(())
}

#[test]
fn library_returns_reader_errors_before_opening_the_pager() {
    struct FailingReader;
    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("log reader failed"))
        }
    }
    let error = bless::page_reader("Broken log", FailingReader).unwrap_err();
    assert_eq!(error.to_string(), "log reader failed");
}

#[test]
fn uses_the_spawning_shell_instead_of_a_stale_shell_variable() -> Result<()> {
    let mut command = Command::new("/bin/sh");
    command
        // Keep the parent shell alive; a final exec would erase its identity.
        .args(["-c", "\"$1\" \"$2\"; exit", "bless-test"])
        .arg(env!("CARGO_BIN_EXE_bless"))
        .arg("printf 'SPAWNING=<%s>\\n' \"$SHELL\"")
        .env("SHELL", "/not/a/shell");
    let output = run_in_pty(command, 0)?;
    assert!(
        output.contains("SPAWNING=</bin/sh>") || output.contains("SPAWNING=</usr/bin/sh>"),
        "{output}"
    );
    Ok(())
}

#[test]
fn quoted_pipeline_preserves_output_and_exit_status() -> Result<()> {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "\"$1\" \"$2\"; exit", "bless-test"])
        .arg(env!("CARGO_BIN_EXE_bless"))
        .arg("printf 'zebra,alpha\\n' | tr ',' ':'; printf 'after\\n' | tr a-z A-Z; exit 9")
        .env("SHELL", "/not/a/shell");
    let output = run_in_pty(command, 9)?;
    assert!(
        output.contains("zebra:alpha") && output.contains("AFTER"),
        "{output}"
    );
    Ok(())
}

#[test]
#[ignore = "requires PowerShell 7.5 or later"]
fn powershell_aliases_and_named_parameters_reach_the_pager() -> Result<()> {
    for invocation in ["gci", "gci -Name -LiteralPath ."] {
        let mut command = Command::new("pwsh");
        command
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "& $env:BLESS_TEST_BINARY {invocation}; exit $LASTEXITCODE"
            ))
            .env("BLESS_TEST_BINARY", env!("CARGO_BIN_EXE_bless"))
            .env("SHELL", "/not/a/shell")
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests"));
        let output = run_in_pty(command, 0)?;
        assert!(output.contains("spawning_shell.rs"), "{output}");
    }
    Ok(())
}

struct CallerExecutable(PathBuf);

impl Drop for CallerExecutable {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn accepts_a_parent_executable_without_a_recognized_shell_name() -> Result<()> {
    let path = std::env::temp_dir().join(format!("bless-caller-{}", std::process::id()));
    let mut file = File::options().write(true).create_new(true).open(&path)?;
    let caller = CallerExecutable(path);
    io::copy(&mut File::open("/bin/sh")?, &mut file)?;
    file.set_permissions(fs::metadata("/bin/sh")?.permissions())?;
    drop(file);

    let mut command = Command::new(&caller.0);
    command
        .args(["-c", "\"$1\" \"$2\"; exit", "bless-test"])
        .arg(env!("CARGO_BIN_EXE_bless"))
        .arg("printf 'CALLER=<%s>\\n' \"$SHELL\"")
        .env("SHELL", "/not/a/shell");
    let output = run_in_pty(command, 0)?;
    assert!(
        output.contains(&format!("CALLER=<{}>", caller.0.display())),
        "{output}"
    );
    Ok(())
}

fn run_in_pty(mut command: Command, expected_code: i32) -> Result<String> {
    let (mut master_fd, mut slave_fd) = (-1, -1);
    let mut size = libc::winsize {
        ws_col: 80,
        ws_row: 8,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let (mut master, slave) =
        unsafe { (File::from_raw_fd(master_fd), File::from_raw_fd(slave_fd)) };
    for fd in [master_fd, slave_fd] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    let flags = unsafe { libc::fcntl(master_fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave.try_clone()?));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = ShellChild(command.spawn()?);
    drop(command);
    drop(slave);
    let mut output = Vec::new();
    let mut quit = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() >= deadline {
            bail!("pager timed out: {}", String::from_utf8_lossy(&output));
        }
        let mut fd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut fd, 1, 50) } < 0
            && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
        {
            return Err(io::Error::last_os_error().into());
        }
        let mut buf = [0; 4096];
        match master.read(&mut buf) {
            Ok(n) => output.extend_from_slice(&buf[..n]),
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted
                    || e.raw_os_error() == Some(libc::EIO) => {}
            Err(e) => return Err(e.into()),
        }
        if !quit && output.windows(5).any(|bytes| bytes == b"cols ") {
            master.write_all(b"q")?;
            quit = true;
        }
        if let Some(status) = child.0.try_wait()? {
            assert_eq!(
                status.code(),
                Some(expected_code),
                "{}",
                String::from_utf8_lossy(&output)
            );
            break;
        }
    }
    let output = String::from_utf8_lossy(&output);
    assert!(quit, "pager never opened: {output}");
    Ok(output.into_owned())
}

//! A synchronous ANSI/VT-aware terminal pager.
//!
//! Use [`page_text`] for log text or [`page_reader`] for a file or other reader.
//! Suspend any existing TUI and leave its raw/alternate-screen mode before
//! calling; resume it after the call returns. Paging uses stdout and `/dev/tty`
//! and must not run concurrently with another terminal UI.
use std::{env, io::Read, process::ExitCode};

use anyhow::Result;

mod app;
mod cli;
mod command;
mod render;
mod shell;
mod terminal;
mod text;

use app::App;
use cli::{command_columns, configured_max_bytes, print_help, read_limited, stdin_is_tty};
use text::{Line, parse_vt};

pub(crate) const MAX_LOAD_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_LINES: usize = 1_000_000;

/// Page supplied UTF-8 text, interpreting ANSI/VT sequences.
///
/// `title` labels the status line. This blocks until the user quits and restores
/// the terminal before returning, including on an I/O error. See [`page_reader`]
/// for input limits and terminal requirements.
///
/// ```no_run
/// bless::page_text("Application logs", "\x1b[31merror\x1b[0m\n")?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn page_text(title: impl Into<String>, text: &str) -> Result<()> {
    page_reader(title, text.as_bytes())
}

/// Read ANSI/VT bytes to completion, then page them interactively.
///
/// Input is bounded by `BLESS_MAX_BYTES` (64 MiB by default), with a truncation
/// notice in the pager. At most 1,000,000 decoded lines are retained. Input is
/// read before entering raw mode, so reader errors leave the terminal untouched.
/// This is not a streaming/live-log viewer; readers must eventually reach EOF.
///
/// Requires stdout to be the terminal being paged and a controlling `/dev/tty`
/// for keyboard input. Suspend other terminal UIs first. Quitting with `q`, Esc,
/// or Ctrl-C returns `Ok(())`; read, parse, and terminal I/O errors are returned.
///
/// ```no_run
/// bless::page_reader("Application logs", std::fs::File::open("app.log")?)?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn page_reader(title: impl Into<String>, reader: impl Read) -> Result<()> {
    let title = title.into();
    let (input, notice) = read_limited(reader, &title)?;
    let (cols, rows) = terminal::Terminal::size();
    page_lines(parse_vt(&input, cols, rows)?, title, notice)
}

/// Run the command-line pager using the current process's arguments and stdin.
pub fn run_cli() -> Result<ExitCode> {
    let mut argv = env::args();
    let command_name = argv
        .next()
        .and_then(|arg| {
            std::path::PathBuf::from(arg)
                .file_name()
                .map(|name| name.to_owned())
        })
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| "bless".into());
    let args: Vec<String> = argv.collect();
    if args.is_empty() && stdin_is_tty() {
        print_help(&command_name);
        return Ok(ExitCode::SUCCESS);
    }

    let (cols, rows) = terminal::Terminal::size();
    let (name, lines, notice, code) = if !args.is_empty() {
        let captured =
            command::capture(&args, command_columns(cols), rows, configured_max_bytes())?;
        if captured.interrupted {
            return Ok(ExitCode::from(captured.code));
        }
        (
            args.join(" "),
            captured.lines,
            captured.notice,
            captured.code,
        )
    } else {
        let (input, notice) = read_limited(std::io::stdin(), "stdin")?;
        ("stdin".into(), parse_vt(&input, cols, rows)?, notice, 0)
    };

    page_lines(lines, name, notice)?;
    Ok(ExitCode::from(code))
}

fn page_lines(lines: Vec<Line>, name: String, mut notice: Option<String>) -> Result<()> {
    if lines.len() >= MAX_LINES {
        let msg = format!("line cap reached ({MAX_LINES})");
        notice = Some(notice.map(|n| format!("{n}; {msg}")).unwrap_or(msg));
    }

    App::new(lines, name, notice).run()
}

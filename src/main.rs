/** bless: a blessed less. */
use std::{env, process::ExitCode};

use anyhow::Result;

mod app;
mod cli;
mod command;
mod render;
mod shell;
mod terminal;
mod text;

use app::App;
use cli::{command_columns, configured_max_bytes, print_help, read_stdin_limited, stdin_is_tty};
use text::parse_vt;

pub(crate) const MAX_LOAD_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_LINES: usize = 1_000_000;

fn main() -> Result<ExitCode> {
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
    let (name, lines, mut notice, code) = if !args.is_empty() {
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
        let (name, input, notice) = read_stdin_limited()?;
        (name, parse_vt(&input, cols, rows)?, notice, 0)
    };

    if lines.len() >= MAX_LINES {
        let msg = format!("line cap reached ({MAX_LINES})");
        notice = Some(notice.map(|n| format!("{n}; {msg}")).unwrap_or(msg));
    }

    App::new(lines, name, notice).run()?;
    Ok(ExitCode::from(code))
}

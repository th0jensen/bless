/** bless: a blessed less. */
use std::env;

use anyhow::Result;

mod app;
mod cli;
mod render;
mod terminal;
mod text;

use app::App;
use cli::{print_help, read_limited};
use text::{highlight_text, parse_ansi};

pub(crate) const MAX_LOAD_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const MAX_LINES: usize = 1_000_000;

fn main() -> Result<()> {
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
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_help(&command_name);
        return Ok(());
    }

    let (name, path, input, mut notice) = read_limited(&args, &command_name)?;
    let (lines, raw_lines) = if input.contains('\x1b') {
        let (lines, raw) = parse_ansi(&input)?;
        (lines, Some(raw))
    } else {
        (highlight_text(&input, path.as_ref()), None)
    };

    if lines.len() >= MAX_LINES {
        let msg = format!("line cap reached ({MAX_LINES})");
        notice = Some(notice.map(|n| format!("{n}; {msg}")).unwrap_or(msg));
    }

    App::new(lines, raw_lines, name, notice).run()
}

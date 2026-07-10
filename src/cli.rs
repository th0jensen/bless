use std::{
    env, fs,
    io::{self, Read},
    os::fd::AsRawFd,
    path::PathBuf,
};

use anyhow::{bail, Context, Result};

use crate::MAX_LOAD_BYTES;

pub(crate) fn print_help(command_name: &str) {
    println!(
        "Usage: {command_name} [OPTION]... [FILE]

bless: a small Ghostty-oriented pager with libghostty-vt SGR parsing.
The installed command is a shorthand to avoid conflicting with macOS bless(8).

Arguments:
  FILE                  File to view. Use '-' to read standard input.
                        If standard input is a pipe, FILE may be omitted.

Options:
  -h, --help            Display this help and exit.

Environment:
  BLESS_MAX_BYTES=SIZE  Maximum bytes to load (default: 256MiB).
                        SIZE accepts B, KiB, MiB, GiB suffixes."
    );
}

pub(crate) fn read_limited(
    args: &[String],
    command_name: &str,
) -> Result<(String, Option<PathBuf>, String, Option<String>)> {
    let Some(file) = args.first() else {
        if stdin_is_tty() {
            bail!("missing operand\nTry '{command_name} --help' for more information.");
        }
        return read_stdin_limited();
    };
    if file == "-" {
        return read_stdin_limited();
    }

    let p = PathBuf::from(file);
    let meta = fs::metadata(&p).with_context(|| format!("stat {file}"))?;
    let limit = configured_max_bytes();
    let mut f = fs::File::open(&p).with_context(|| format!("reading {file}"))?;
    let mut bytes = Vec::with_capacity(meta.len().min(limit) as usize);
    std::io::Read::by_ref(&mut f)
        .take(limit)
        .read_to_end(&mut bytes)?;
    let notice = if meta.len() > limit {
        Some(format!(
            "loaded first {} of {}",
            human_bytes(limit),
            human_bytes(meta.len())
        ))
    } else {
        None
    };
    Ok((
        file.clone(),
        Some(p),
        String::from_utf8_lossy(&bytes).into_owned(),
        notice,
    ))
}

fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(io::stdin().as_raw_fd()) == 1 }
}

fn read_stdin_limited() -> Result<(String, Option<PathBuf>, String, Option<String>)> {
    let limit = configured_max_bytes();
    let mut stdin = io::stdin();
    let mut bytes = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    let mut truncated = false;
    loop {
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if total + n as u64 > limit {
            let keep = (limit - total) as usize;
            bytes.extend_from_slice(&buf[..keep]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        total += n as u64;
    }
    let notice = truncated.then(|| format!("stdin capped at {}", human_bytes(limit)));
    Ok((
        "stdin".into(),
        None,
        String::from_utf8_lossy(&bytes).into_owned(),
        notice,
    ))
}

fn configured_max_bytes() -> u64 {
    env::var("BLESS_MAX_BYTES")
        .ok()
        .and_then(|v| parse_bytes(&v))
        .unwrap_or(MAX_LOAD_BYTES)
}

fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..split].parse().ok()?;
    let unit = s[split..].trim().to_ascii_lowercase();
    Some(
        n * match unit.as_str() {
            "" | "b" => 1,
            "k" | "kb" | "kib" => 1024,
            "m" | "mb" | "mib" => 1024 * 1024,
            "g" | "gb" | "gib" => 1024 * 1024 * 1024,
            _ => return None,
        },
    )
}

fn human_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

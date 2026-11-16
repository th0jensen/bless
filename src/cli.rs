use std::{
    env,
    io::{self, Read},
    os::fd::AsRawFd,
};

use anyhow::Result;
use indoc::printdoc;

use crate::MAX_LOAD_BYTES;

pub(crate) fn print_help(command_name: &str) {
    printdoc! {"
        Usage: {command_name} COMMAND [ARG]...
               {command_name} 'SHELL COMMAND'
               COMMAND | {command_name}

        bless: a small Ghostty-oriented pager with automatic ANSI/VT parsing.
        The installed command is a shorthand to avoid conflicting with macOS bless(8).

        Arguments:
          COMMAND [ARG]...      Run in a PTY, then page its terminal output.
                                All arguments belong to the command, never files
                                or pager options. Multiple arguments stay literal.
          'SHELL COMMAND'       One argument runs through the spawning shell.
                                Fallback: $SHELL, then /bin/sh.
                                Quote pipelines and other shell syntax as one string;
                                unquoted operators belong to the calling shell.
                                No arguments: page stdin, or show help at a terminal.

        Environment:
          BLESS_MAX_BYTES=SIZE  Maximum bytes to load (default: 64MiB).
                                SIZE accepts B, KiB, MiB, GiB suffixes.
          BLESS_COLUMNS=COLS    Command PTY width (default: at least 160 columns).
        "
    };
}

pub(crate) fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(io::stdin().as_raw_fd()) == 1 }
}

pub(crate) fn read_limited(mut reader: impl Read, name: &str) -> Result<(Vec<u8>, Option<String>)> {
    let limit = configured_max_bytes();
    let mut bytes = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buf)?;
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
    let notice = truncated.then(|| format!("{name} capped at {}", human_bytes(limit)));
    Ok((bytes, notice))
}

pub(crate) fn configured_max_bytes() -> u64 {
    env::var("BLESS_MAX_BYTES")
        .ok()
        .and_then(|v| parse_bytes(&v))
        .unwrap_or(MAX_LOAD_BYTES)
}

pub(crate) fn command_columns(viewport: u16) -> u16 {
    env::var("BLESS_COLUMNS")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(viewport.max(160))
}

fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..split].parse().ok()?;
    let unit = s[split..].trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(multiplier)
}

pub(crate) fn human_bytes(n: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_byte_limits_without_overflow() {
        assert_eq!(parse_bytes("64MiB"), Some(64 * 1024 * 1024));
        assert_eq!(parse_bytes("2 gb"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_bytes("bogus"), None);
        assert_eq!(parse_bytes("18446744073709551615GiB"), None);
    }
}

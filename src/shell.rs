use std::{env, path::PathBuf};

pub(crate) fn spawning_shell() -> PathBuf {
    parent_executable()
        .or_else(|| {
            env::var_os("SHELL")
                .filter(|shell| !shell.is_empty())
                .map(Into::into)
        })
        .unwrap_or_else(|| "/bin/sh".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn parent_executable() -> Option<PathBuf> {
    None
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn shell_path(executable: PathBuf, invocation: Option<&std::ffi::OsStr>) -> PathBuf {
    use std::{fs, os::unix::fs::MetadataExt, path::Path};

    if let Some(invocation) = invocation
        && let Ok(actual) = fs::metadata(&executable)
    {
        use std::os::unix::ffi::OsStrExt;

        // A login-shell prefix is a name hint, never an option to forward.
        let bytes = invocation.as_bytes();
        let invocation = Path::new(std::ffi::OsStr::from_bytes(
            bytes.strip_prefix(b"-").unwrap_or(bytes),
        ));
        let candidate = if invocation.is_absolute() {
            Some(invocation.to_path_buf())
        } else if invocation.components().count() == 1 {
            executable.parent().map(|dir| dir.join(invocation))
        } else {
            None
        };
        if let Some(candidate) = candidate
            && let Ok(alias) = fs::metadata(&candidate)
            && actual.dev() == alias.dev()
            && actual.ino() == alias.ino()
        {
            // Preserve sh versus bash (and other shell aliases) only when the
            // hint names the very same executable. Never search our own PATH.
            return candidate;
        }
    }
    executable
}

#[cfg(target_os = "linux")]
fn parent_executable() -> Option<PathBuf> {
    use std::{ffi::OsStr, fs, io::Read, os::unix::ffi::OsStrExt};

    let pid = unsafe { libc::getppid() };
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let executable = fs::read_link(proc.join("exe")).ok()?;
    // cmdline is mutable and can be large. Read at most one pathname's worth,
    // accepting only a complete NUL-terminated argv[0] as an alias hint.
    let mut cmdline = Vec::new();
    let read = fs::File::open(proc.join("cmdline"))
        .and_then(|file| file.take(4096).read_to_end(&mut cmdline));
    let invocation = read.ok().and_then(|_| {
        cmdline
            .iter()
            .position(|&byte| byte == 0)
            .map(|end| OsStr::from_bytes(&cmdline[..end]))
    });
    Some(shell_path(executable, invocation))
}

#[cfg(target_os = "macos")]
fn parent_executable() -> Option<PathBuf> {
    use std::{ffi::OsStr, mem, os::unix::ffi::OsStrExt};

    let pid = unsafe { libc::getppid() };
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if len <= 0 || len as usize >= path.len() {
        return None;
    }
    let executable = PathBuf::from(OsStr::from_bytes(&path[..len as usize]));
    let mut info = unsafe { mem::zeroed::<libc::proc_bsdinfo>() };
    let size = mem::size_of_val(&info) as libc::c_int;
    // libproc returns zero on failure, not -1. Reject short results as well.
    if unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) }
        != size
    {
        return Some(executable);
    }
    // pbi_comm is only an alias hint, subject to the same device/inode check as
    // Linux's argv[0]. It never overrides the executable with another program.
    let name: Vec<u8> = info
        .pbi_comm
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    Some(shell_path(executable, Some(OsStr::from_bytes(&name))))
}

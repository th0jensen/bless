prefix := if os() == "macos" { env("PREFIX", env("HOME") / "opt") } else { env("PREFIX", env("HOME") / ".local") }

install_name := env("INSTALL_NAME", "bls")
bindir := env("BINDIR", prefix / "bin")
mandir := env("MANDIR", prefix / "share/man")
destdir := env("DESTDIR", "")

cargo := env("CARGO", "cargo")
install_cmd := env("INSTALL", "install")
gzip := env("GZIP", "gzip")

# build release binary and man page
all: release man

# print resolved platform/prefix/install-name
platform:
    @printf 'os=%s\n' '{{ os() }}'
    @printf 'prefix=%s\n' '{{ prefix }}'
    @printf 'install_name=%s\n' '{{ install_name }}'

# build the release binary
release:
    {{ cargo }} build --release

# install binary and man page
install: release man
    {{ install_cmd }} -d "{{ destdir }}{{ bindir }}"
    {{ install_cmd }} -m 0755 target/release/bless "{{ destdir }}{{ bindir }}/{{ install_name }}"
    {{ install_cmd }} -d "{{ destdir }}{{ mandir }}/man1"
    {{ gzip }} -cn doc/{{ install_name }}.1 > "{{ destdir }}{{ mandir }}/man1/{{ install_name }}.1.gz"
    chmod 0644 "{{ destdir }}{{ mandir }}/man1/{{ install_name }}.1.gz"

# remove installed binary and man page
uninstall:
    rm -f "{{ destdir }}{{ bindir }}/{{ install_name }}" "{{ destdir }}{{ mandir }}/man1/{{ install_name }}.1.gz"

# clean build artifacts
clean:
    {{ cargo }} clean

# fast-forward the vendored ghostty submodule to the tip of its tracked branch
update-ghostty:
    git submodule update --remote --merge vendor/ghostty
    @echo "vendor/ghostty now at $(git -C vendor/ghostty rev-parse --short HEAD) — review and commit the submodule bump."

# regenerate the man page from the crate version and current date
man:
    #!/usr/bin/env bash
    set -euo pipefail
    command_name='{{ install_name }}'
    version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
    date_str="$(date +'%B %Y')"
    upper_name="$(printf '%s' "$command_name" | tr '[:lower:]-' '[:upper:]_')"
    mkdir -p doc
    cat > "doc/${command_name}.1" <<MAN
    .TH ${upper_name} 1 "${date_str}" "bless ${version}" "User Commands"
    .SH NAME
    ${command_name} \- command shorthand for bless, a small Ghostty-oriented pager
    .SH SYNOPSIS
    .B ${command_name}
    .I COMMAND
    .RI [ ARG ]...
    .br
    .B ${command_name}
    .I 'SHELL COMMAND'
    .br
    .I COMMAND
    .B | ${command_name}
    .SH DESCRIPTION
    .B bless
    is a terminal pager inspired by
    .BR less (1).
    It renders directly to a modern Ghostty-compatible terminal and uses
    .B libghostty-vt
    to interpret all input as ANSI/VT terminal output, without raw/color flags
    or synthetic syntax highlighting. Cursor motion, erasure, character sets,
    colours, decorations, and hyperlinks are decoded into pageable text cells.
    .PP
    Long rows are clipped, never wrapped. Horizontal scrolling preserves
    text styling and OSC 8 hyperlinks. Tabs use eight-column terminal stops.
    The status line shows the visible columns and the widest row.
    Configure the producing command for wide, unwrapped output if it wraps
    or truncates its own output before writing to the pipe.
    The emulator uses the initial terminal dimensions and joins soft wraps
    for horizontal paging. Resizing the pager does not replay the input.
    Cursor-addressed output depends on that initial geometry.
    .PP
    Prefix a command with the pager, without a separator, to capture its normal
    terminal output through a PTY:
    .B ${command_name} git diff
    .PP
    Everything after the pager is executed, never treated as a file or pager
    option. Multiple arguments run directly, preserving literal arguments.
    A single argument is evaluated with -c by the spawning process's executable.
    There is no shell-name allowlist and wrappers are not skipped: the caller
    is responsible for supplying a shell-compatible executable. If the parent
    executable cannot be read, the fallback is nonempty SHELL, then /bin/sh.
    The selected path is exported
    as SHELL to the child. Other exported variables and the working directory
    are inherited, apart from the PTY and pager settings described below.
    Quote a whole pipeline, redirection, or command list as one argument to put
    it inside capture. Unquoted shell operators belong to the calling shell
    and cannot be captured by the executable. The fresh noninteractive shell
    does not copy the calling session's unexported variables, aliases, functions,
    options, or jobs. The shell's own startup and exported-function rules apply.
    Intermediate pipeline commands write to pipes and may suppress colours.
    .PP
    PowerShell aliases and cmdlets such as gci work without quoting. When a
    command with arguments is not a native executable, PowerShell 7.5 or later
    resolves it using -CommandWithArgs. Native executables retain exact argv.
    Quote the whole command for colon-attached parameter values or explicit
    switch booleans, which -CommandWithArgs otherwise converts.
    .PP
    The PTY is at least 160 columns wide, or the current viewport width if larger.
    BLESS_COLUMNS overrides its width. Ghostty decodes output incrementally and
    answers terminal queries back to the child. Both stdout and stderr are captured.
    Capture finishes before paging; interactive prompts and live TUIs are not
    supported. Piped stdin is passed to the child; host keyboard input is not.
    Ctrl-C cancels capture and stops the child process group. Reaching the output
    cap also stops the command. The command's exit status is returned after paging.
    PAGER and GIT_PAGER are set to cat to prevent nested pagers.
    .PP
    The view is the final active text screen and retained scrollback, not an
    animation. Graphics, clipboard writes, title changes, and terminal queries
    are not forwarded to the host terminal.
    .PP
    The installed command is
    .B ${command_name}
    to avoid conflicting with Apple macOS' built-in
    .BR bless (8)
    utility while keeping the program name
    .BR bless .
    .PP
    .B bless
    reads standard input when piped input is available and no arguments are
    given. With no arguments at a terminal, it displays usage.
    To view a file, use cat as the command or redirect the file to standard input.
    .SH OPERANDS
    .TP
    .I COMMAND [ARG]...
    Command and arguments to capture through a PTY, without a separator.
    .TP
    .I 'SHELL COMMAND'
    Shell source to execute through a PTY, including quoted pipelines.
    .SH KEYS
    .TP
    .BR q ,\  Esc ,\  Ctrl-C
    Quit.
    .TP
    .BR j ,\  Down
    Scroll down one line.
    .TP
    .BR k ,\  Up
    Scroll up one line.
    .TP
    .BR Space ,\  d ,\  PageDown
    Scroll down half a page.
    .TP
    .BR u ,\  PageUp
    Scroll up half a page.
    .TP
    .BR g
    Go to the first page.
    .TP
    .BR G
    Go to the last page.
    .TP
    .BR h ,\  Left
    Scroll left.
    .TP
    .BR l ,\  Right
    Scroll right.
    .TP
    .BR H ,\  L
    Scroll left or right by half a viewport.
    .TP
    .BR 0 ,\  $
    Go to the first or last horizontal viewport.
    .TP
    .B /
    Search.
    .TP
    .BR n ,\  N
    Go to the next or previous search match.
    .SH ENVIRONMENT
    .TP
    .B BLESS_MAX_BYTES
    Maximum number of bytes to load. The default is 64MiB. Values may use
    .BR B ,
    .BR KiB ,
    .BR MiB ,
    or
    .BR GiB
    suffixes.
    .TP
    .B BLESS_COLUMNS
    Command PTY width, from 1 to 65535 columns. Defaults to at least 160 columns.
    .TP
    .B SHELL
    Fallback shell when the spawning shell cannot be identified. An unset or
    empty value falls back to /bin/sh. Children receive the selected shell path.
    .SH EXAMPLES
    .TP
    View a file:
    .B ${command_name} cat Cargo.toml
    .TP
    View coloured command output:
    .B ${command_name} fastfetch
    .TP
    Capture a pipeline:
    .B ${command_name} 'printf zebra | tr a-z A-Z'
    .TP
    Raise the load cap:
    .B BLESS_MAX_BYTES=1GiB ${command_name} cat huge.log
    .SH SEE ALSO
    .BR less (1),
    .BR ghostty (1)
    MAN

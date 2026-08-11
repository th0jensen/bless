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
    .RI [ OPTION ]...
    .I FILE
    .br
    .I COMMAND
    .B | ${command_name} -
    .SH DESCRIPTION
    .B bless
    is a terminal pager inspired by
    .BR less (1).
    It renders directly to a modern Ghostty-compatible terminal and uses
    .B libghostty-vt
    for SGR parsing so coloured command output can be preserved.
    .PP
    The installed command is
    .B ${command_name}
    to avoid conflicting with Apple macOS' built-in
    .BR bless (8)
    utility while keeping the program name
    .BR bless .
    .PP
    .B bless
    does not read standard input implicitly when no file is given. Use
    .B -
    as the file operand to read from standard input.
    .SH OPTIONS
    .TP
    .BR -h ,\  --help
    Display help and exit.
    .SH OPERANDS
    .TP
    .I FILE
    The file to view. Use
    .B -
    to read from standard input.
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
    .B /
    Search.
    .TP
    .BR n ,\  N
    Go to the next or previous search match.
    .SH ENVIRONMENT
    .TP
    .B BLESS_MAX_BYTES
    Maximum number of bytes to load. The default is 256MiB. Values may use
    .BR B ,
    .BR KiB ,
    .BR MiB ,
    or
    .BR GiB
    suffixes.
    .SH EXAMPLES
    .TP
    View a file:
    .B ${command_name} Cargo.toml
    .TP
    View coloured command output:
    .B fastfetch --pipe false | ${command_name} -
    .TP
    Raise the load cap:
    .B BLESS_MAX_BYTES=1GiB ${command_name} huge.log
    .SH SEE ALSO
    .BR less (1),
    .BR ghostty (1)
    MAN

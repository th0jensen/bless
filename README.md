# bless

`bless` is a small Rust pager: a deliberately blessed `less` clone that interprets all input through Ghostty's ANSI/VT terminal emulator.

## Features

- execute everything after `bls`; with no arguments, page piped stdin
- prefix commands with `bls`, e.g. `bls git diff`: capture their normal terminal output through a PTY, without a separator or colour flags
- run quoted shell commands, including pipelines, redirections, and command lists: `bls 'git diff | cat'`
- raw terminal alternate-screen pager
- vim/less-style movement: `j/k`, arrows, `d/u`, space/page up/down, `g/G`, `q`
- wide output stays unwrapped: `h/l` or left/right pan four columns, `H/L` pan half a viewport, `0/$` jump to the first/last columns
- preserves colors, text decorations, and OSC 8 links while panning; tabs align to eight-column terminal stops
- `/` search; cycle matches with `n/N`, arrow keys, or `h/j/k/l`
- automatic VT decoding, including cursor movement, erasure, character sets, SGR styling, and OSC palette changes; no raw/color flags
- renders resolved terminal cells, not arbitrary source terminal commands; no synthetic syntax highlighting

## Build

Requires Rust 1.96+ and [Zig](https://ziglang.org) 0.17.0. The Rust bindings are pinned to upstream revision `483ea3d5247b214060da58ef6037a95f30ae89fb`, paired with their Ghostty `vt-1.0.1` revision `108cacb17ed8990840cd14abfce43d8ed8503ca1`. `libghostty-vt` is built from source and statically linked: `.cargo/config.toml` points its build script at the matching `vendor/ghostty` submodule. Kitty graphics are disabled; this pager renders text cells.

The resulting binary has no runtime dependency on a system or bundled `libghostty-vt` — it's statically linked in. Runtime dependencies are intentionally tiny: just `anyhow` and `libc`. Cargo source packages exclude the large upstream submodule. Registry publication is currently gated on a matching upstream bindings release; see [Publication](#publication).

When updating the Rust bindings, update `vendor/ghostty` to the exact revision pinned by their build script. Updating Ghostty alone to an unrelated tip can break the C ABI; `just update-ghostty` is only useful for deliberately testing such an upgrade.

Large-file safeguards: `bless` loads at most 64 MiB by default, stores styled text as compact runs, and caps parsed lines at 1,000,000. Override bytes with `BLESS_MAX_BYTES`, e.g. `BLESS_MAX_BYTES=256MiB bls cat huge.log`.

Use the installed command shorthand, `bls`, for pipelines. Running it without arguments reads piped stdin, or prints usage when stdin is a terminal. There are no pager options or file operands: to view a file, use `bls cat FILE` or `bls < FILE`.

```sh
git clone --recurse-submodules --shallow-submodules https://github.com/th0jensen/bless.git
cd bless
cargo build --release
```

If you already cloned without `--recurse-submodules`, run `git submodule update --init` first.

## Use as a library

The Cargo package is `bless-pager`; the Rust library is `bless`. Git dependencies do not require `publish = true` or a registry release:

```toml
[dependencies]
bless = { package = "bless-pager", git = "https://github.com/th0jensen/bless" }
```

Pin `rev` to the Bless commit you have tested for reproducible builds. A git consumer still needs Rust and Zig as above. Cargo does not inherit this repository's `.cargo/config.toml` when Bless is a dependency; the bindings' build script fetches its own exact pinned Ghostty source instead.

```rust
// Suspend your own terminal UI and leave its raw/alternate-screen mode first.
bless::page_text("Application logs", "\x1b[31merror\x1b[0m\n")?;
// Or supply any std::io::Read, without redirecting process stdin:
bless::page_reader("Application logs", std::fs::File::open("app.log")?)?;
// Resume your terminal UI after the result has been handled, even on error.
```

Both entry points block until the user quits, preserve ANSI/VT styling, and restore terminal state before returning. They use stdout for rendering and `/dev/tty` for keys; stdout must be the terminal being paged. They must not overlap another terminal UI. Input is read before entering raw mode; reader errors are returned without opening the pager. Readers must reach EOF: this is not a live-log viewer. The same `BLESS_MAX_BYTES` cap (64 MiB by default), line cap, and truncation notices as the CLI apply.

## Publication

`bless-pager` is the intended registry name because crates.io's `bless` is an unrelated project. The executable remains `bless`, installed as `bls`. No registry release has been made, and `publish = false` deliberately prevents publishing an unusable package.

A crates.io consumer cannot publish with a git-only dependency, even when that dependency is itself publishable. The matching `libghostty-vt` and `libghostty-vt-sys` sources must first be released to the registry. The registry's current `0.2.2` releases are **not** the sources pinned above: they use bindings commit `c9876b2902d5bb3e1f3d61b6395059f1e5e7b721`, Ghostty `a887df42c56f6de86c0fe6da9c4eeca37931e083`, and the older Rust API. Adding their version to the git dependency would make Cargo package different, incompatible sources.

The release path is to obtain compatible upstream registry releases for the pinned pair, replace the git dependency with that verified registry version, remove `publish = false`, and run `cargo package` plus `cargo publish --dry-run` before an approved publication. After `bless-pager` itself is released, a downstream crates.io package can use `bless = { package = "bless-pager", version = "0.1" }`. Until then, use the git dependency for development only; publishing downstream is blocked. Maintaining and publishing renamed forks of both bindings crates would be a separate alternative, not an implicit side effect of this library extraction.

## Install from source

```sh
just install
```

`just install` (or bare `just`, which builds the release binary and man page) detects the host platform without an explicit Cargo `--target`. Source installs avoid system/package-manager prefixes by default: `~/.local` on Linux/other Unix and `~/opt` on macOS.

macOS already ships Apple's `/usr/sbin/bless` utility. To avoid shadowing a system administration command while keeping one command name across platforms, the installed command shorthand is `bls` everywhere. The program, project, and Rust library are still named `bless`; the Cargo package is `bless-pager`. Override `INSTALL_NAME`, `PREFIX`, `BINDIR`, or `MANDIR` if needed; packaging recipes should set their own prefix explicitly.

## Run

```sh
bls cat src/main.rs
bls git diff
bls fastfetch
bls 'printf "zebra\nalpha\n" | sort'
rg TODO --color=always | bls
```

## Command capture

Everything after `bls` is a command, never a file operand or pager option. `bls git --help` forwards `--help` to Git. Multiple arguments execute directly and remain literal: `bls printf '%s\n' 'a b;$HOME'` does not expand the last argument again.

A single argument is shell source, evaluated with `-c` by the spawning process's executable. There is no shell-name allowlist and wrappers are not skipped: the caller is responsible for using an executable that supports shell invocation. This follows a nested shell, including PowerShell, even when `$SHELL` names a different login shell. If the parent executable cannot be read, `bls` falls back to a nonempty `$SHELL`, then `/bin/sh`. The child receives the selected path as `SHELL`; its other exported environment and working directory are inherited, apart from the PTY/pager settings described below.

In PowerShell, `bls gci` works directly. When a command with arguments is not a native executable, PowerShell 7.5+ resolves it as an alias, function, or cmdlet using `-CommandWithArgs`, e.g. `bls gci -Name`. Native executables retain their exact arguments. For PowerShell's colon-attached parameter values or explicit switch booleans, quote the whole command instead, e.g. `bls 'Get-ChildItem -Name -Force:$false'`; `-CommandWithArgs` otherwise converts those tokens.

Use `bls 'cmd | other'` for pipelines, redirections, variable assignments, builtins, or command lists. This starts a fresh noninteractive process of the detected shell, not a clone of your live shell session: unexported variables, aliases, functions, options, and jobs are not copied. The shell's own startup rules still apply, including any exported-function support it provides.

Quote the whole pipeline to put it inside capture. In `bls cmd | other`, the calling shell consumes the pipe before `bls` starts: only `cmd` belongs to `bls`, and the pager's own output goes to `other`. Capturing that unquoted remainder requires shell integration; an executable cannot recover shell syntax from its arguments. Within a captured pipeline, intermediate commands still write to pipes and may suppress their own automatic colours; the final command's stdout and the pipeline's inherited stderr use the PTY.

The child writes stdout and stderr to a PTY, so commands retain their ordinary automatic colours. Ghostty interprets output as it arrives and answers terminal queries back to the child. The PTY is at least 160 columns wide (or the current viewport width if larger); override it with `BLESS_COLUMNS=240 bls git diff`. Its geometry stays fixed while paging and resizing.

Capture finishes before the pager opens. Piped stdin is passed to the command. Host keyboard input is not forwarded: commands requiring interactive prompts or a live TUI are not supported. Ctrl-C cancels capture and stops the child process group. The output-byte cap also stops the command and displays a notice. When the direct command exits, background descendants retaining its PTY are stopped rather than delaying paging. After paging, `bls` returns the command's exit status; failed-command output remains readable. `PAGER` and `GIT_PAGER` are set to `cat` in the child to avoid nested pagers.

## Wide output

`bless` clips long rows instead of wrapping them. Use `h/l` or left/right arrows for small horizontal steps, `H/L` for half-viewport steps, and `0/$` to jump to either horizontal edge. The status line shows the visible column range and the widest row. Vertical paging keeps the same horizontal position; resizing changes the viewport, not the stored rows.

All captured commands and piped input follow the same byte-preserving VT decoding path, even without escape bytes. Carriage return and backspace have terminal semantics: overwriting does not erase the rest of the row or invent bold/underline styling. LF starts a new line by default, including after a terminal reset. The emulator uses the terminal dimensions at load time, then joins its soft-wrapped rows for horizontal paging; resizing the pager does not replay or reflow the input. Cursor-addressed output depends on that initial geometry, which may differ from the original capture.

The pager cannot recover bytes the producer never emitted. Some commands disable color or change their layout when stdout is a pipe; prefix the command with `bls` instead. A producer's own hard wrapping or truncation cannot be undone.

The stored view is the final active text screen plus retained scrollback, not an animation or byte-for-byte replay. Overwritten/erased cells and discarded alternate screens are not preserved. Hyperlink destinations and text styling survive; terminal graphics, clipboard writes, title changes, and query responses are not forwarded to your terminal. Unspecified colors use your terminal's theme; explicit OSC palette/default-color changes are resolved into the displayed cells.

## Test

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

With PowerShell 7.5+ installed, include its real-PTY alias/cmdlet regression:

```sh
cargo test --test spawning_shell -- --include-ignored
```

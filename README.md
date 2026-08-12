# bless

`bless` is a small Rust pager: a deliberately blessed `less` clone with syntax highlighting and ANSI/SGR output parsed through `libghostty-vt`.

## Features

- open files; read stdin explicitly with `-`
- raw terminal alternate-screen pager
- vim/less-style movement: `j/k`, arrows, `d/u`, space/page up/down, `g/G`, `q`
- `/` search; cycle matches with `n/N`, arrow keys, or `h/j/k/l`
- tiny built-in syntax highlighting for common text/code; no highlighting mega-crate
- preserves colored command output by parsing SGR with Ghostty's `libghostty-vt`
- renders ANSI directly back to the terminal, assuming a modern Ghostty-compatible terminal

## Build

Requires Rust and [Zig](https://ziglang.org) 0.16+. `libghostty-vt` is built from source and statically linked: the repository pins a shallow `vendor/ghostty` git submodule checkout, and `.cargo/config.toml` points the `libghostty-vt-sys` build script at it via `GHOSTTY_SOURCE_DIR`. The Rust integration uses the safe `libghostty-vt` crate.

The resulting binary has no runtime dependency on a system or bundled `libghostty-vt` — it's statically linked in. Runtime dependencies are intentionally tiny: just `anyhow` and `libc`. Cargo source packages exclude the large upstream submodule, and this source-coupled crate is intentionally not published to crates.io.

Run `just update-ghostty` to fast-forward the submodule to its tracked `main` branch, then rebuild and test before committing the new pinned gitlink: upstream C API changes can drift from the Rust crate's checked-in bindings.

Large-file safeguards: `bless` loads at most 64 MiB by default, stores styled text as compact runs, and caps parsed lines at 1,000,000. Override bytes with `BLESS_MAX_BYTES`, e.g. `BLESS_MAX_BYTES=256MiB bls huge.log`.

`bless` follows Unix operand style: it does not read stdin implicitly. Use the installed command shorthand, `bls -`, for pipelines.

```sh
git clone --recurse-submodules --shallow-submodules https://github.com/th0jensen/bless.git
cd bless
cargo build --release
```

If you already cloned without `--recurse-submodules`, run `git submodule update --init` first.

## Install from source

```sh
just install
```

`just install` (or bare `just`, which builds the release binary and man page) detects the host platform without an explicit Cargo `--target`. Source installs avoid system/package-manager prefixes by default: `~/.local` on Linux/other Unix and `~/opt` on macOS.

macOS already ships Apple's `/usr/sbin/bless` utility. To avoid shadowing a system administration command while keeping one command name across platforms, the installed command shorthand is `bls` everywhere. The program, project, and package are still named `bless`. Override `INSTALL_NAME`, `PREFIX`, `BINDIR`, or `MANDIR` if needed; packaging recipes should set their own prefix explicitly.

## Run

```sh
bls src/main.rs
rg TODO --color=always | bls -
```

## Test

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

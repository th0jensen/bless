# bless

`bless` is a small Rust pager: a deliberately blessed `less` clone with syntax highlighting and ANSI/SGR output parsed through `libghostty-vt`.

## Features

- open files; read stdin explicitly with `-`
- raw terminal alternate-screen pager
- vim/less-style movement: `j/k`, arrows, `d/u`, space/page up/down, `g/G`, `q`
- `/` search and `n/N` navigation
- tiny built-in syntax highlighting for common text/code; no highlighting mega-crate
- preserves colored command output by parsing SGR with Ghostty's `libghostty-vt`
- renders ANSI directly back to the terminal, assuming a modern Ghostty-compatible terminal

## Build

Requires Rust and [Zig](https://ziglang.org) 0.16+. `libghostty-vt` is built from source and statically linked: the `vendor/ghostty` git submodule tracks the tip of upstream [ghostty](https://github.com/ghostty-org/ghostty)'s `main` branch, and `.cargo/config.toml` points the `libghostty-vt-sys` build script at it via `GHOSTTY_SOURCE_DIR`. The Rust integration uses the safe `libghostty-vt` crate.

The resulting binary has no runtime dependency on a system or bundled `libghostty-vt` — it's statically linked in. Runtime dependencies are intentionally tiny: just `anyhow` and `libc`.

Because it tracks ghostty's tip rather than a pinned release, the vendored C API can drift from what the `libghostty-vt` crate's checked-in bindings expect. Run `just update-ghostty` to fast-forward the submodule and rebuild to catch breakage early.

Large-file safeguards: `bless` loads at most 256 MiB by default and caps parsed lines at 1,000,000. Override bytes with `BLESS_MAX_BYTES`, e.g. `BLESS_MAX_BYTES=1GiB bls huge.log`.

`bless` follows Unix operand style: it does not read stdin implicitly. Use the installed command shorthand, `bls -`, for pipelines.

```sh
git clone --recurse-submodules https://github.com/th0jensen/bless.git
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

## Arch Linux / AUR

AUR packaging files live in `packaging/aur`. Release tags should use the `vX.Y.Z` format expected by the `PKGBUILD`, for example `v0.1.0`.

## Run

```sh
bls src/main.rs
rg TODO --color=always | bls -
```

A sparse 10 GiB test file is available at `samples/10gb.sparse.log`.

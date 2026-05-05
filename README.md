# dtach-rs

A small Rust clone of [`dtach`](https://github.com/crigler/dtach): it runs one
program under a pseudo-terminal, lets terminals attach and detach from it, and
keeps the program alive after the original terminal goes away.

## Installation

### Pre-built binaries

Download the latest binary for your platform from the
[GitHub Releases](https://github.com/nicoavanzdev/dtach-rs/releases) page.

### Windows (WinGet)

```powershell
winget install nicoavanzdev.dtach-rs
```

### macOS / Linux (Homebrew)

```sh
brew install nicoavanzdev/tap/dtach-rs
```

### From source (cargo)

```sh
cargo install --git https://github.com/nicoavanzdev/dtach-rs
```

This implementation keeps the upstream `dtach` command-line shape:

```text
dtach-rs -a <socket> <options>
dtach-rs -A <socket> <options> <command...>
dtach-rs -c <socket> <options> <command...>
dtach-rs -n <socket> <options> <command...>
dtach-rs -N <socket> <options> <command...>
dtach-rs -p <socket>
```

## Features

- `-a`: attach to an existing session.
- `-A`: attach, or create the session if it does not exist.
- `-c`: create a session and attach to it.
- `-n`: create a detached session and exit.
- `-N`: create a session and keep the master in the foreground.
- `-p`: copy standard input to a session without scanning for detach keys.
- `-l`, `--list`: list active sessions known to the per-user registry.
- `-e <char>` and `-E`: customize or disable the detach character.
- `-r none|ctrl_l|winch`: choose the redraw behavior.
- `-z`: disable local suspend handling.

The default detach key is `^\` (`Ctrl-\`), matching upstream `dtach`.

## Cross-Platform Model

The original `dtach` uses Unix-domain sockets as session handles. To work on
Windows and Unix with the same code path, `dtach-rs` stores a small descriptor
file at the requested `<socket>` path. That descriptor points to an
authenticated loopback control port owned by the master process.

The program PTY is provided by `portable-pty`, so Unix uses the platform PTY
implementation and Windows uses ConPTY. Windows therefore requires a ConPTY
capable system, normally Windows 10 October 2018 or newer.
Interactive attach/detach behavior should be tested from a real terminal;
headless Windows shells do not always exercise ConPTY like an attached console.

`dtach-rs` is not wire-compatible with the original C `dtach`; existing C
`dtach` clients cannot attach to `dtach-rs` sessions, and vice versa.

## Examples

Create or attach to a shell:

```sh
dtach-rs -A /tmp/work-shell bash
```

Create a detached session:

```sh
dtach-rs -n /tmp/work-shell bash
```

Send commands to a session:

```sh
printf 'cd /var/log\nls -l\n' | dtach-rs -p /tmp/work-shell
```

List active sessions:

```sh
dtach-rs -l
dtach-rs -l /tmp
```

On Windows, use a normal filesystem path for the session descriptor:

```powershell
dtach-rs -A $env:TEMP\work-shell powershell
```

## Development

This repository uses Rust 2024.

```sh
cargo build
cargo test
```


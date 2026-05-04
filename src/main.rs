mod args;
mod attach;
mod list;
mod master;
mod protocol;
mod session;
mod trace;

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process;

use args::{Cli, Mode};
use master::{run_master_foreground, spawn_master_background};

type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let code = match real_main() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("dtach-rs: {err}");
            1
        }
    };
    process::exit(code);
}

fn real_main() -> AppResult<i32> {
    let mut args = env::args_os();
    let _program = args.next();
    let args: Vec<OsString> = args.collect();

    if args.first().is_some_and(|arg| arg == "--dtach-rs-master") {
        return run_internal_master(&args[1..]);
    }

    let cli = match Cli::parse(args) {
        Ok(cli) => cli,
        Err(args::ParseOutcome::Help) => {
            print_usage();
            return Ok(0);
        }
        Err(args::ParseOutcome::Version) => {
            println!("dtach-rs {VERSION}");
            return Ok(0);
        }
        Err(args::ParseOutcome::Error(message)) => {
            eprintln!("dtach-rs: {message}");
            eprintln!("Try 'dtach-rs --help' for more information.");
            return Ok(1);
        }
    };

    dispatch(cli)
}

fn dispatch(cli: Cli) -> AppResult<i32> {
    match cli.mode {
        Mode::Attach => {
            attach::attach_path(&cli.socket, cli.options, false)?;
            Ok(0)
        }
        Mode::AttachOrCreate => match attach::open_session(&cli.socket) {
            Ok(session) => {
                attach::attach_connected(session, cli.options)?;
                Ok(0)
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    || err.kind() == std::io::ErrorKind::ConnectionRefused =>
            {
                if err.kind() == std::io::ErrorKind::ConnectionRefused {
                    let _ = std::fs::remove_file(&cli.socket);
                }
                spawn_master_background(&cli.socket, &cli.command, cli.options.redraw, true)?;
                attach::attach_path(&cli.socket, cli.options, false)?;
                Ok(0)
            }
            Err(err) => Err(Box::new(err)),
        },
        Mode::Create => {
            spawn_master_background(&cli.socket, &cli.command, cli.options.redraw, true)?;
            attach::attach_path(&cli.socket, cli.options, false)?;
            Ok(0)
        }
        Mode::NewDetached => {
            spawn_master_background(&cli.socket, &cli.command, cli.options.redraw, false)?;
            Ok(0)
        }
        Mode::NewForeground => run_master_foreground(
            cli.socket,
            cli.command,
            cli.options.redraw.defaulted(),
            false,
            None,
        ),
        Mode::Push => {
            attach::push_path(&cli.socket)?;
            Ok(0)
        }
        Mode::List => list::run(&cli.command),
    }
}

fn run_internal_master(args: &[OsString]) -> AppResult<i32> {
    if args.len() < 6 {
        return Err("invalid internal master invocation".into());
    }

    let socket = PathBuf::from(&args[0]);
    let wait_attach = match args[1].to_string_lossy().as_ref() {
        "0" => false,
        "1" => true,
        _ => return Err("invalid internal wait flag".into()),
    };
    let redraw = args::RedrawMethod::from_wire(args[2].to_string_lossy().parse()?)
        .ok_or("invalid internal redraw method")?;
    let ready_file = PathBuf::from(&args[3]);

    if args[4] != "--" {
        return Err("invalid internal command separator".into());
    }
    let command = args[5..].to_vec();

    #[cfg(unix)]
    unsafe {
        libc::setsid();
    }

    run_master_foreground(
        socket,
        command,
        redraw.defaulted(),
        wait_attach,
        Some(ready_file),
    )
}

fn print_usage() {
    println!(
        "dtach-rs {VERSION}
Usage: dtach-rs -a <socket> <options>
       dtach-rs -A <socket> <options> <command...>
       dtach-rs -c <socket> <options> <command...>
       dtach-rs -n <socket> <options> <command...>
       dtach-rs -N <socket> <options> <command...>
       dtach-rs -p <socket>
       dtach-rs -l [path...]

Modes:
  -a            Attach to the specified session.
  -A            Attach, or create the session if it does not exist.
  -c            Create a new session and attach to it.
  -n            Create a new detached session and exit.
  -N            Create a new session and keep the master in the foreground.
  -p            Copy standard input to an existing session.
  -l, --list    List active sessions. With paths, scan those files/directories.

Options:
  -e <char>     Set the detach character, defaults to ^\\.
  -E            Disable the detach character.
  -r <method>   Redraw method: none, ctrl_l, or winch.
  -z            Disable local suspend processing."
    );
}

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Cli {
    pub mode: Mode,
    pub socket: PathBuf,
    pub options: AttachOptions,
    pub command: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Attach,
    AttachOrCreate,
    Create,
    NewDetached,
    NewForeground,
    Push,
    List,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachOptions {
    pub detach_char: Option<u8>,
    pub no_suspend: bool,
    pub redraw: RedrawMethod,
}

impl Default for AttachOptions {
    fn default() -> Self {
        Self {
            detach_char: Some(b'\\' & 0x1f),
            no_suspend: false,
            redraw: RedrawMethod::Unspecified,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedrawMethod {
    Unspecified,
    None,
    CtrlL,
    Winch,
}

impl RedrawMethod {
    pub fn defaulted(self) -> Self {
        match self {
            Self::Unspecified => Self::CtrlL,
            method => method,
        }
    }

    pub fn to_wire(self) -> u8 {
        match self {
            Self::Unspecified => 0,
            Self::None => 1,
            Self::CtrlL => 2,
            Self::Winch => 3,
        }
    }

    pub fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unspecified),
            1 => Some(Self::None),
            2 => Some(Self::CtrlL),
            3 => Some(Self::Winch),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum ParseOutcome {
    Help,
    Version,
    Error(String),
}

impl Cli {
    pub fn parse(args: Vec<OsString>) -> Result<Self, ParseOutcome> {
        if args.is_empty() {
            return Err(ParseOutcome::Error("No mode was specified.".to_string()));
        }

        if args[0] == "--help" || args[0] == "-?" {
            return Err(ParseOutcome::Help);
        }
        if args[0] == "--version" {
            return Err(ParseOutcome::Version);
        }

        let mode = parse_mode(&args[0])?;
        if mode == Mode::List {
            return Ok(Self {
                mode,
                socket: PathBuf::new(),
                options: AttachOptions::default(),
                command: args[1..].to_vec(),
            });
        }

        if args.len() < 2 {
            return Err(ParseOutcome::Error("No socket was specified.".to_string()));
        }
        let socket = PathBuf::from(&args[1]);

        if mode == Mode::Push {
            if args.len() != 2 {
                return Err(ParseOutcome::Error(
                    "Invalid number of arguments for -p.".to_string(),
                ));
            }
            return Ok(Self {
                mode,
                socket,
                options: AttachOptions::default(),
                command: Vec::new(),
            });
        }

        let mut options = AttachOptions::default();
        let mut index = 2;
        while index < args.len() {
            let text = args[index].to_string_lossy();
            if text == "--" {
                index += 1;
                break;
            }
            if !text.starts_with('-') || text == "-" {
                break;
            }
            parse_option(&args, &mut index, &mut options)?;
            index += 1;
        }

        let command = args[index..].to_vec();
        if mode == Mode::Attach {
            if !command.is_empty() {
                return Err(ParseOutcome::Error(
                    "Invalid number of arguments for -a.".to_string(),
                ));
            }
        } else if command.is_empty() {
            return Err(ParseOutcome::Error("No command was specified.".to_string()));
        }

        Ok(Self {
            mode,
            socket,
            options,
            command,
        })
    }
}

fn parse_mode(arg: &OsStr) -> Result<Mode, ParseOutcome> {
    match arg.to_string_lossy().as_ref() {
        "-a" => Ok(Mode::Attach),
        "-A" => Ok(Mode::AttachOrCreate),
        "-c" => Ok(Mode::Create),
        "-n" => Ok(Mode::NewDetached),
        "-N" => Ok(Mode::NewForeground),
        "-p" => Ok(Mode::Push),
        "-l" | "--list" => Ok(Mode::List),
        other if other.starts_with('-') && other.len() >= 2 => {
            Err(ParseOutcome::Error(format!("Invalid mode '{}'.", other)))
        }
        _ => Err(ParseOutcome::Error("No mode was specified.".to_string())),
    }
}

fn parse_option(
    args: &[OsString],
    index: &mut usize,
    options: &mut AttachOptions,
) -> Result<(), ParseOutcome> {
    let text = args[*index].to_string_lossy();
    let bytes = text.as_bytes();
    let mut pos = 1;

    while pos < bytes.len() {
        match bytes[pos] {
            b'E' => options.detach_char = None,
            b'z' => options.no_suspend = true,
            b'e' => {
                *index += 1;
                if *index >= args.len() {
                    return Err(ParseOutcome::Error(
                        "No escape character specified.".to_string(),
                    ));
                }
                options.detach_char = Some(parse_detach_char(&args[*index]));
                break;
            }
            b'r' => {
                *index += 1;
                if *index >= args.len() {
                    return Err(ParseOutcome::Error(
                        "No redraw method specified.".to_string(),
                    ));
                }
                options.redraw = parse_redraw(&args[*index])?;
                break;
            }
            other => {
                return Err(ParseOutcome::Error(format!(
                    "Invalid option '-{}'.",
                    other as char
                )));
            }
        }
        pos += 1;
    }
    Ok(())
}

fn parse_detach_char(value: &OsStr) -> u8 {
    let text = value.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'^' {
        if bytes[1] == b'?' {
            0x7f
        } else {
            bytes[1] & 0x1f
        }
    } else {
        bytes.first().copied().unwrap_or_default()
    }
}

fn parse_redraw(value: &OsStr) -> Result<RedrawMethod, ParseOutcome> {
    match value.to_string_lossy().as_ref() {
        "none" => Ok(RedrawMethod::None),
        "ctrl_l" => Ok(RedrawMethod::CtrlL),
        "winch" => Ok(RedrawMethod::Winch),
        _ => Err(ParseOutcome::Error(
            "Invalid redraw method specified.".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_create_with_options() {
        let cli = Cli::parse(os(&["-c", "sock", "-Ez", "-r", "winch", "bash"])).unwrap();
        assert_eq!(cli.mode, Mode::Create);
        assert_eq!(cli.options.detach_char, None);
        assert!(cli.options.no_suspend);
        assert_eq!(cli.options.redraw, RedrawMethod::Winch);
        assert_eq!(cli.command, os(&["bash"]));
    }

    #[test]
    fn parses_control_escape() {
        let cli = Cli::parse(os(&["-a", "sock", "-e", "^A"])).unwrap();
        assert_eq!(cli.options.detach_char, Some(1));
    }

    #[test]
    fn parses_list_without_socket() {
        let cli = Cli::parse(os(&["--list"])).unwrap();
        assert_eq!(cli.mode, Mode::List);
        assert!(cli.command.is_empty());
    }
}

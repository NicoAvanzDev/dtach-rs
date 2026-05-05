use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};

use crate::args::{AttachOptions, RedrawMethod};
use crate::protocol::{write_message, Message, TerminalSize};
use crate::session;
use crate::AppResult;

const EOS: &str = "\x1b[999H";

pub struct ConnectedSession {
    stream: TcpStream,
}

struct RawTerminal;

impl RawTerminal {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        restore_terminal_modes();
    }
}

fn restore_terminal_modes() {
    let _ = disable_raw_mode();
    print!("\x1b[?25h");
    let _ = io::stdout().flush();
}

pub fn open_session(path: &Path) -> io::Result<ConnectedSession> {
    let descriptor = session::read_descriptor(path)?;
    let mut stream = TcpStream::connect(("127.0.0.1", descriptor.port))?;
    stream.set_nodelay(true)?;
    write_message(&mut stream, &Message::Hello(descriptor.token))?;
    Ok(ConnectedSession { stream })
}

pub fn attach_path(path: &Path, options: AttachOptions, quiet: bool) -> AppResult<()> {
    match open_session(path) {
        Ok(session) => attach_connected(session, options),
        Err(err) if quiet => Err(Box::new(err)),
        Err(err) => Err(io::Error::new(err.kind(), format!("{}: {err}", path.display())).into()),
    }
}

pub fn attach_connected(session: ConnectedSession, options: AttachOptions) -> AppResult<()> {
    crate::trace::event("attach: connected");
    let _terminal = RawTerminal::enter()?;
    let initial_size = current_terminal_size();

    let output_stream = session.stream.try_clone()?;
    let writer = Arc::new(Mutex::new(session.stream));
    let running = Arc::new(AtomicBool::new(true));
    let detached = Arc::new(AtomicBool::new(false));

    {
        let mut stream = writer.lock().expect("poisoned mutex");
        write_message(&mut *stream, &Message::Attach)?;
        crate::trace::event("attach: sent attach");
        write_message(
            &mut *stream,
            &Message::Redraw {
                method: options.redraw,
                size: initial_size,
            },
        )?;
        crate::trace::event("attach: sent redraw");
    }

    print!("\x1b[H\x1b[J");
    io::stdout().flush()?;

    let output_running = Arc::clone(&running);
    let output_detached = Arc::clone(&detached);
    let output_thread = thread::spawn(move || {
        read_session_output(output_stream, output_running, output_detached);
    });

    let resize_running = Arc::clone(&running);
    let resize_writer = Arc::clone(&writer);
    let resize_thread = thread::spawn(move || {
        poll_resize(resize_writer, resize_running, initial_size);
    });

    let input_running = Arc::clone(&running);
    let input_detached = Arc::clone(&detached);
    let input_writer = Arc::clone(&writer);
    thread::spawn(move || {
        read_keyboard(input_writer, input_running, input_detached, options);
    });

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }

    let _ = writer
        .lock()
        .expect("poisoned mutex")
        .shutdown(Shutdown::Both);
    let _ = output_thread.join();
    let _ = resize_thread.join();
    Ok(())
}

pub fn push_path(path: &Path) -> AppResult<()> {
    let session = open_session(path)?;
    let mut stream = session.stream;
    let mut stdin = io::stdin().lock();
    let mut buf = [0u8; 8192];

    loop {
        let len = stdin.read(&mut buf)?;
        if len == 0 {
            return Ok(());
        }
        write_message(&mut stream, &Message::Push(buf[..len].to_vec()))?;
    }
}

fn read_session_output(mut stream: TcpStream, running: Arc<AtomicBool>, detached: Arc<AtomicBool>) {
    let mut stdout = io::stdout().lock();
    let mut buf = [0u8; 8192];
    while running.load(Ordering::SeqCst) {
        match stream.read(&mut buf) {
            Ok(0) => {
                crate::trace::event("attach: output socket EOF");
                if !detached.load(Ordering::SeqCst) {
                    let _ = write!(stdout, "{EOS}\r\n[EOF - dtach terminating]\r\n");
                    let _ = stdout.flush();
                }
                running.store(false, Ordering::SeqCst);
                return;
            }
            Ok(len) => {
                if detached.load(Ordering::SeqCst) || !running.load(Ordering::SeqCst) {
                    return;
                }
                if stdout.write_all(&buf[..len]).is_err() || stdout.flush().is_err() {
                    running.store(false, Ordering::SeqCst);
                    return;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                crate::trace::event("attach: output socket error");
                running.store(false, Ordering::SeqCst);
                return;
            }
        }
    }
}

fn read_keyboard(
    writer: Arc<Mutex<TcpStream>>,
    running: Arc<AtomicBool>,
    detached: Arc<AtomicBool>,
    options: AttachOptions,
) {
    #[cfg(windows)]
    read_keyboard_events(writer, running, detached, options);

    #[cfg(not(windows))]
    read_keyboard_bytes(writer, running, detached, options);
}

#[cfg(not(windows))]
fn read_keyboard_bytes(
    writer: Arc<Mutex<TcpStream>>,
    running: Arc<AtomicBool>,
    detached: Arc<AtomicBool>,
    options: AttachOptions,
) {
    let mut stdin = io::stdin().lock();
    let mut byte = [0u8; 1];

    while running.load(Ordering::SeqCst) {
        match stdin.read(&mut byte) {
            Ok(0) => {
                crate::trace::event("attach: stdin EOF");
                return;
            }
            Ok(_) => {
                if !handle_input_bytes(&writer, &running, &detached, options, &byte) {
                    return;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                crate::trace::event("attach: stdin error");
                return;
            }
        }
    }
}

#[cfg(windows)]
fn read_keyboard_events(
    writer: Arc<Mutex<TcpStream>>,
    running: Arc<AtomicBool>,
    detached: Arc<AtomicBool>,
    options: AttachOptions,
) {
    while running.load(Ordering::SeqCst) {
        match event::poll(Duration::from_millis(50)) {
            Ok(false) => {}
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) => {
                    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    let Some(bytes) = encode_key_event(key) else {
                        continue;
                    };
                    if !handle_input_bytes(&writer, &running, &detached, options, &bytes) {
                        return;
                    }
                }
                Ok(Event::Resize(cols, rows)) => {
                    let _ = send_message(&writer, &Message::Resize(TerminalSize { rows, cols }));
                }
                Ok(_) => {}
                Err(_) => {
                    crate::trace::event("attach: event read error");
                    return;
                }
            },
            Err(_) => {
                crate::trace::event("attach: event poll error");
                return;
            }
        }
    }
}

fn handle_input_bytes(
    writer: &Arc<Mutex<TcpStream>>,
    running: &Arc<AtomicBool>,
    detached: &Arc<AtomicBool>,
    options: AttachOptions,
    bytes: &[u8],
) -> bool {
    if bytes.is_empty() {
        return true;
    }

    if options.detach_char == Some(bytes[0]) {
        detached.store(true, Ordering::SeqCst);
        let _ = send_message(writer, &Message::Detach);
        running.store(false, Ordering::SeqCst);
        let _ = writer
            .lock()
            .expect("poisoned mutex")
            .shutdown(Shutdown::Both);
        print!("{EOS}\r\n[detached]\r\n");
        let _ = io::stdout().flush();
        return false;
    }

    if !options.no_suspend && bytes[0] == 0x1a {
        suspend_attach(writer, options.redraw);
        return true;
    }

    if send_message(writer, &Message::Push(bytes.to_vec())).is_err() {
        running.store(false, Ordering::SeqCst);
        return false;
    }

    if bytes[0] == b'\x0c' {
        let _ = send_message(writer, &Message::Resize(current_terminal_size()));
    }

    true
}

#[cfg(windows)]
fn encode_key_event(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Backspace => Some(vec![0x08]),
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            control_char(ch).map(|byte| vec![byte])
        }
        KeyCode::Char(ch) => {
            let mut buf = [0u8; 4];
            Some(ch.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        KeyCode::F(n) => function_key(n),
        KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => None,
    }
}

#[cfg(windows)]
fn control_char(ch: char) -> Option<u8> {
    match ch {
        'a'..='z' => Some((ch as u8) - b'a' + 1),
        'A'..='Z' => Some((ch as u8) - b'A' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

#[cfg(windows)]
fn function_key(n: u8) -> Option<Vec<u8>> {
    let sequence = match n {
        1 => "\x1bOP",
        2 => "\x1bOQ",
        3 => "\x1bOR",
        4 => "\x1bOS",
        5 => "\x1b[15~",
        6 => "\x1b[17~",
        7 => "\x1b[18~",
        8 => "\x1b[19~",
        9 => "\x1b[20~",
        10 => "\x1b[21~",
        11 => "\x1b[23~",
        12 => "\x1b[24~",
        _ => return None,
    };
    Some(sequence.as_bytes().to_vec())
}

fn poll_resize(
    writer: Arc<Mutex<TcpStream>>,
    running: Arc<AtomicBool>,
    mut last_size: TerminalSize,
) {
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(250));
        let size = current_terminal_size();
        if size != last_size {
            last_size = size;
            if send_message(&writer, &Message::Resize(size)).is_err() {
                running.store(false, Ordering::SeqCst);
                return;
            }
        }
    }
}

#[cfg(unix)]
fn suspend_attach(writer: &Arc<Mutex<TcpStream>>, redraw: RedrawMethod) {
    let _ = send_message(writer, &Message::Detach);
    let _ = disable_raw_mode();
    print!("\x1b[H\x1b[2J");
    let _ = io::stdout().flush();
    unsafe {
        libc::raise(libc::SIGTSTP);
    }
    let _ = enable_raw_mode();
    let _ = send_message(writer, &Message::Attach);
    let _ = send_message(
        writer,
        &Message::Redraw {
            method: redraw,
            size: current_terminal_size(),
        },
    );
}

#[cfg(not(unix))]
fn suspend_attach(writer: &Arc<Mutex<TcpStream>>, _redraw: RedrawMethod) {
    let _ = send_message(writer, &Message::Push(vec![0x1a]));
}

fn send_message(writer: &Arc<Mutex<TcpStream>>, message: &Message) -> io::Result<()> {
    let mut stream = writer.lock().expect("poisoned mutex");
    write_message(&mut *stream, message)
}

fn current_terminal_size() -> TerminalSize {
    match terminal_size() {
        Ok((cols, rows)) => TerminalSize { rows, cols },
        Err(_) => TerminalSize::fallback(),
    }
}

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

use crate::args::RedrawMethod;
use crate::protocol::{read_message, Message, TerminalSize};
use crate::session::{self, SessionDescriptor};
use crate::AppResult;

type PtyWriter = Box<dyn Write + Send>;
type SharedPtyWriter = Arc<Mutex<PtyWriter>>;
type SharedMasterPty = Arc<Mutex<Box<dyn MasterPty + Send>>>;

struct ClientEntry {
    id: u64,
    attached: bool,
    stream: TcpStream,
}

struct SharedState {
    clients: Mutex<Vec<ClientEntry>>,
    first_attached: Mutex<bool>,
    first_attached_cv: Condvar,
    shutdown: AtomicBool,
}

struct ReadyReporter {
    path: Option<PathBuf>,
    reported: bool,
}

impl ReadyReporter {
    fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            reported: false,
        }
    }

    fn ok(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::write(path, "ok\n");
        }
        self.reported = true;
    }

    fn error(&mut self, message: &str) {
        if self.reported {
            return;
        }
        if let Some(path) = &self.path {
            let _ = fs::write(path, format!("err\n{message}\n"));
        }
        self.reported = true;
    }
}

pub fn spawn_master_background(
    socket: &Path,
    command: &[OsString],
    redraw: RedrawMethod,
    wait_attach: bool,
) -> AppResult<()> {
    crate::trace::event("parent: spawning master");
    let ready_file = std::env::temp_dir().join(format!(
        "dtach-rs-ready-{}-{}",
        std::process::id(),
        session::random_token()?
    ));

    let mut child = {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("--dtach-rs-master")
            .arg(socket)
            .arg(if wait_attach { "1" } else { "0" })
            .arg(redraw.defaulted().to_wire().to_string())
            .arg(&ready_file)
            .arg("--")
            .args(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }

        cmd.spawn()?
    };

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(contents) = fs::read_to_string(&ready_file) {
            let _ = fs::remove_file(&ready_file);
            if contents.starts_with("ok\n") {
                return Ok(());
            }
            let message = contents.lines().skip(1).collect::<Vec<_>>().join("\n");
            return Err(io::Error::other(if message.is_empty() {
                "master failed to start".to_string()
            } else {
                message
            })
            .into());
        }

        if let Some(status) = child.try_wait()? {
            let _ = fs::remove_file(&ready_file);
            return Err(io::Error::other(format!(
                "master exited before becoming ready ({status})"
            ))
            .into());
        }

        if Instant::now() >= deadline {
            let _ = fs::remove_file(&ready_file);
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for master to start",
            )
            .into());
        }

        thread::sleep(Duration::from_millis(25));
    }
}

pub fn run_master_foreground(
    socket: PathBuf,
    command: Vec<OsString>,
    default_redraw: RedrawMethod,
    wait_attach: bool,
    ready_file: Option<PathBuf>,
) -> AppResult<i32> {
    let mut ready = ReadyReporter::new(ready_file);
    match run_master_inner(socket, command, default_redraw, wait_attach, &mut ready) {
        Ok(code) => Ok(code),
        Err(err) => {
            ready.error(&err.to_string());
            Err(err)
        }
    }
}

fn run_master_inner(
    socket: PathBuf,
    command: Vec<OsString>,
    default_redraw: RedrawMethod,
    wait_attach: bool,
    ready: &mut ReadyReporter,
) -> AppResult<i32> {
    crate::trace::event("master: starting");
    if command.is_empty() {
        return Err("No command was specified.".into());
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let token = session::random_token()?;

    let pty_pair = native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let command_builder = CommandBuilder::from_argv(command);
    let mut child = pty_pair.slave.spawn_command(command_builder)?;
    crate::trace::event(format!(
        "master: child spawned pid={:?}",
        child.process_id()
    ));
    drop(pty_pair.slave);

    let mut reader = pty_pair.master.try_clone_reader()?;
    let writer = pty_pair.master.take_writer()?;
    let master_pty = Arc::new(Mutex::new(pty_pair.master));
    let pty_writer = Arc::new(Mutex::new(writer));
    let mut child_killer = child.clone_killer();

    let descriptor = SessionDescriptor {
        port,
        token: token.clone(),
        pid: std::process::id(),
    };
    if let Err(err) = session::write_descriptor(&socket, &descriptor) {
        let _ = child.kill();
        return Err(err.into());
    }
    let _ = session::register_session(&socket);

    ready.ok();
    crate::trace::event("master: ready");

    let shared = Arc::new(SharedState {
        clients: Mutex::new(Vec::new()),
        first_attached: Mutex::new(false),
        first_attached_cv: Condvar::new(),
        shutdown: AtomicBool::new(false),
    });

    let reader_shared = Arc::clone(&shared);
    let reader_writer = Arc::clone(&pty_writer);
    let _reader_thread = thread::spawn(move || {
        if wait_attach {
            let mut attached = reader_shared.first_attached.lock().expect("poisoned mutex");
            while !*attached && !reader_shared.shutdown.load(Ordering::SeqCst) {
                attached = reader_shared
                    .first_attached_cv
                    .wait(attached)
                    .expect("poisoned mutex");
            }
        }

        let mut buf = [0u8; 8192];
        let mut output_filter = TerminalOutputFilter::default();
        while !reader_shared.shutdown.load(Ordering::SeqCst) {
            match reader.read(&mut buf) {
                Ok(0) => {
                    crate::trace::event("master: pty reader EOF");
                    reader_shared.shutdown.store(true, Ordering::SeqCst);
                    let _ = child_killer.kill();
                    break;
                }
                Ok(len) => {
                    let output = output_filter.filter(&buf[..len], &reader_writer);
                    if !output.is_empty() {
                        broadcast(&reader_shared, &output);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    crate::trace::event("master: pty reader error");
                    reader_shared.shutdown.store(true, Ordering::SeqCst);
                    let _ = child_killer.kill();
                    break;
                }
            }
        }
    });

    let accept_shared = Arc::clone(&shared);
    let accept_writer = Arc::clone(&pty_writer);
    let accept_master = Arc::clone(&master_pty);
    let accept_token = token.clone();
    let accept_thread = thread::spawn(move || {
        let mut next_client_id = 1u64;
        while !accept_shared.shutdown.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    crate::trace::event("master: accepted client");
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let _ = stream.set_nodelay(true);
                    let id = next_client_id;
                    next_client_id = next_client_id.wrapping_add(1);
                    let shared = Arc::clone(&accept_shared);
                    let writer = Arc::clone(&accept_writer);
                    let master = Arc::clone(&accept_master);
                    let token = accept_token.clone();
                    thread::spawn(move || {
                        handle_client(id, stream, token, shared, writer, master, default_redraw);
                    });
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => {
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
    });

    let status = child.wait()?;
    crate::trace::event(format!("master: child exited {status}"));
    shared.shutdown.store(true, Ordering::SeqCst);
    shared.first_attached_cv.notify_all();
    shutdown_clients(&shared);
    session::remove_descriptor(&socket);
    let _ = session::unregister_session(&socket);

    let _ = accept_thread.join();

    Ok(if status.success() {
        0
    } else {
        status.exit_code() as i32
    })
}

fn handle_client(
    id: u64,
    mut stream: TcpStream,
    token: String,
    shared: Arc<SharedState>,
    pty_writer: SharedPtyWriter,
    master_pty: SharedMasterPty,
    default_redraw: RedrawMethod,
) {
    let Ok(Message::Hello(client_token)) = read_message(&mut stream) else {
        crate::trace::event("master: client hello read failed");
        return;
    };
    if client_token != token {
        crate::trace::event("master: client token mismatch");
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }

    let Ok(write_stream) = stream.try_clone() else {
        return;
    };

    {
        let mut clients = shared.clients.lock().expect("poisoned mutex");
        clients.push(ClientEntry {
            id,
            attached: false,
            stream: write_stream,
        });
    }
    crate::trace::event(format!("master: client {id} registered"));

    loop {
        match read_message(&mut stream) {
            Ok(Message::Attach) => {
                crate::trace::event(format!("master: client {id} attach"));
                set_attached(&shared, id, true);
                let mut first_attached = shared.first_attached.lock().expect("poisoned mutex");
                *first_attached = true;
                shared.first_attached_cv.notify_all();
            }
            Ok(Message::Detach) => set_attached(&shared, id, false),
            Ok(Message::Push(bytes)) => {
                crate::trace::event(format!("master: client {id} push {}", bytes.len()));
                if pty_writer
                    .lock()
                    .expect("poisoned mutex")
                    .write_all(&bytes)
                    .is_err()
                {
                    break;
                }
            }
            Ok(Message::Resize(size)) => {
                crate::trace::event(format!(
                    "master: client {id} resize {}x{}",
                    size.cols, size.rows
                ));
                let _ = resize(&master_pty, size);
            }
            Ok(Message::Redraw { method, size }) => {
                crate::trace::event(format!(
                    "master: client {id} redraw {:?} {}x{}",
                    method, size.cols, size.rows
                ));
                redraw(
                    &master_pty,
                    &pty_writer,
                    method.defaulted_to(default_redraw),
                    size,
                );
            }
            Ok(Message::Hello(_)) => {}
            Err(err)
                if err.kind() == io::ErrorKind::UnexpectedEof
                    || err.kind() == io::ErrorKind::ConnectionReset =>
            {
                crate::trace::event(format!("master: client {id} disconnected: {err}"));
                break;
            }
            Err(err) => {
                crate::trace::event(format!("master: client {id} protocol error: {err}"));
                break;
            }
        }
    }

    remove_client(&shared, id);
    crate::trace::event(format!("master: client {id} removed"));
}

#[derive(Default)]
struct TerminalOutputFilter {
    pending: Vec<u8>,
}

impl TerminalOutputFilter {
    fn filter(&mut self, input: &[u8], pty_writer: &SharedPtyWriter) -> Vec<u8> {
        const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
        const CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";
        const STRIPPED_SEQUENCES: &[&[u8]] = &[b"\x1b[?9001h", b"\x1b[?9001l"];

        let mut data = Vec::with_capacity(self.pending.len() + input.len());
        data.extend_from_slice(&self.pending);
        data.extend_from_slice(input);
        self.pending.clear();

        let mut output = Vec::with_capacity(data.len());
        let mut index = 0;
        while index < data.len() {
            let remaining = &data[index..];

            if remaining.starts_with(CURSOR_POSITION_QUERY) {
                crate::trace::event("master: answered cursor position query");
                let _ = pty_writer
                    .lock()
                    .expect("poisoned mutex")
                    .write_all(CURSOR_POSITION_REPLY);
                index += CURSOR_POSITION_QUERY.len();
                continue;
            }

            if let Some(sequence) = STRIPPED_SEQUENCES
                .iter()
                .find(|sequence| remaining.starts_with(sequence))
            {
                crate::trace::event("master: stripped terminal-private mode sequence");
                index += sequence.len();
                continue;
            }

            if CURSOR_POSITION_QUERY.starts_with(remaining)
                || STRIPPED_SEQUENCES
                    .iter()
                    .any(|sequence| sequence.starts_with(remaining))
            {
                self.pending.extend_from_slice(remaining);
                break;
            }

            output.push(data[index]);
            index += 1;
        }

        output
    }
}

fn broadcast(shared: &SharedState, data: &[u8]) {
    crate::trace::event(format!("master: broadcast {}", data.len()));
    let mut clients = shared.clients.lock().expect("poisoned mutex");
    clients.retain_mut(|client| {
        if !client.attached {
            return true;
        }
        let ok = client.stream.write_all(data).is_ok();
        if !ok {
            crate::trace::event(format!(
                "master: client {} broadcast write failed",
                client.id
            ));
        }
        ok
    });
}

fn set_attached(shared: &SharedState, id: u64, attached: bool) {
    let mut clients = shared.clients.lock().expect("poisoned mutex");
    if let Some(client) = clients.iter_mut().find(|client| client.id == id) {
        client.attached = attached;
    }
}

fn remove_client(shared: &SharedState, id: u64) {
    let mut clients = shared.clients.lock().expect("poisoned mutex");
    clients.retain(|client| client.id != id);
}

fn shutdown_clients(shared: &SharedState) {
    let mut clients = shared.clients.lock().expect("poisoned mutex");
    for client in clients.iter_mut() {
        let _ = client.stream.shutdown(Shutdown::Both);
    }
    clients.clear();
}

fn redraw(
    master_pty: &SharedMasterPty,
    pty_writer: &SharedPtyWriter,
    method: RedrawMethod,
    size: TerminalSize,
) {
    #[cfg(windows)]
    let _ = pty_writer;

    match method {
        RedrawMethod::None => {}
        RedrawMethod::CtrlL => {
            let _ = resize(master_pty, size);
            #[cfg(not(windows))]
            let _ = pty_writer
                .lock()
                .expect("poisoned mutex")
                .write_all(b"\x0c");
        }
        RedrawMethod::Winch => {
            let _ = resize(master_pty, size);
        }
        RedrawMethod::Unspecified => unreachable!("redraw methods are defaulted before use"),
    }
}

fn resize(master_pty: &SharedMasterPty, size: TerminalSize) -> AppResult<()> {
    master_pty.lock().expect("poisoned mutex").resize(PtySize {
        rows: size.rows.max(1),
        cols: size.cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    })?;
    Ok(())
}

trait RedrawDefault {
    fn defaulted_to(self, default: RedrawMethod) -> RedrawMethod;
}

impl RedrawDefault for RedrawMethod {
    fn defaulted_to(self, default: RedrawMethod) -> RedrawMethod {
        match self {
            RedrawMethod::Unspecified => default.defaulted(),
            method => method,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};

    struct RecordingWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes
                .lock()
                .expect("poisoned mutex")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn recording_writer() -> (SharedPtyWriter, Arc<Mutex<Vec<u8>>>) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = RecordingWriter {
            bytes: Arc::clone(&bytes),
        };
        (Arc::new(Mutex::new(Box::new(writer))), bytes)
    }

    #[test]
    fn output_filter_answers_cursor_query_without_forwarding_it() {
        let (writer, replies) = recording_writer();
        let mut filter = TerminalOutputFilter::default();

        let output = filter.filter(b"pre\x1b[6npost", &writer);

        assert_eq!(output, b"prepost");
        assert_eq!(
            replies.lock().expect("poisoned mutex").as_slice(),
            b"\x1b[1;1R"
        );
    }

    #[test]
    fn output_filter_strips_win32_input_mode_sequences() {
        let (writer, replies) = recording_writer();
        let mut filter = TerminalOutputFilter::default();

        let output = filter.filter(b"a\x1b[?9001hb\x1b[?9001lc", &writer);

        assert_eq!(output, b"abc");
        assert!(replies.lock().expect("poisoned mutex").is_empty());
    }

    #[test]
    fn output_filter_handles_split_sequences() {
        let (writer, replies) = recording_writer();
        let mut filter = TerminalOutputFilter::default();

        assert_eq!(filter.filter(b"a\x1b[?", &writer), b"a");
        assert_eq!(filter.filter(b"9001hb\x1b[", &writer), b"b");
        assert_eq!(filter.filter(b"6nc", &writer), b"c");
        assert_eq!(
            replies.lock().expect("poisoned mutex").as_slice(),
            b"\x1b[1;1R"
        );
    }
}

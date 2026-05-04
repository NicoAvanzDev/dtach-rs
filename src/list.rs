use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use crate::protocol::{write_message, Message};
use crate::session::{self, SessionDescriptor};
use crate::AppResult;

struct ActiveSession {
    path: PathBuf,
    descriptor: SessionDescriptor,
}

pub fn run(paths: &[OsString]) -> AppResult<i32> {
    let explicit_paths = paths.iter().map(PathBuf::from).collect::<Vec<_>>();
    let candidates = session::descriptor_candidates(&explicit_paths)?;
    let mut seen = BTreeSet::new();
    let mut active = Vec::new();

    for candidate in candidates {
        let path = match session::normalize_path(&candidate) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !seen.insert(path.clone()) {
            continue;
        }

        let Ok(descriptor) = session::read_descriptor(&path) else {
            continue;
        };
        if probe_session(&descriptor).is_ok() {
            active.push(ActiveSession { path, descriptor });
        }
    }

    if explicit_paths.is_empty() {
        let paths = active
            .iter()
            .map(|session| session.path.clone())
            .collect::<Vec<_>>();
        let _ = session::write_registry_entries(&paths);
    }

    print_sessions(&active);
    Ok(0)
}

fn probe_session(descriptor: &SessionDescriptor) -> io::Result<()> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, descriptor.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(300))?;
    stream.set_write_timeout(Some(Duration::from_millis(300)))?;
    write_message(&mut stream, &Message::Hello(descriptor.token.clone()))
}

fn print_sessions(active: &[ActiveSession]) {
    if active.is_empty() {
        println!("No active dtach-rs sessions.");
        return;
    }

    println!("{:<7} {:<7} PATH", "PID", "PORT");
    for session in active {
        println!(
            "{:<7} {:<7} {}",
            session.descriptor.pid,
            session.descriptor.port,
            session.path.display()
        );
    }
}

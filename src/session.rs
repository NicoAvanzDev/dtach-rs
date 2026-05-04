use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const MAGIC: &str = "dtach-rs-session-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionDescriptor {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

pub fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|err| io::Error::other(format!("getrandom failed: {err:?}")))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

pub fn write_descriptor(path: &Path, descriptor: &SessionDescriptor) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options.open(path)?;
    writeln!(file, "{MAGIC}")?;
    writeln!(file, "host=127.0.0.1")?;
    writeln!(file, "port={}", descriptor.port)?;
    writeln!(file, "token={}", descriptor.token)?;
    writeln!(file, "pid={}", descriptor.pid)?;
    file.flush()
}

pub fn read_descriptor(path: &Path) -> io::Result<SessionDescriptor> {
    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines();
    if lines.next() != Some(MAGIC) {
        return Err(invalid("not a dtach-rs session descriptor"));
    }

    let mut port = None;
    let mut token = None;
    let mut pid = None;

    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "port" => port = Some(value.parse::<u16>().map_err(|_| invalid("invalid port"))?),
            "token" => token = Some(value.to_string()),
            "pid" => pid = Some(value.parse::<u32>().map_err(|_| invalid("invalid pid"))?),
            _ => {}
        }
    }

    Ok(SessionDescriptor {
        port: port.ok_or_else(|| invalid("missing port"))?,
        token: token.ok_or_else(|| invalid("missing token"))?,
        pid: pid.ok_or_else(|| invalid("missing pid"))?,
    })
}

pub fn remove_descriptor(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn register_session(path: &Path) -> io::Result<()> {
    let path = normalize_path(path)?;
    let registry = registry_path();
    if let Some(parent) = registry.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(registry)?;
    writeln!(file, "{}", path.display())
}

pub fn unregister_session(path: &Path) -> io::Result<()> {
    let path = normalize_path(path)?;
    let entries = registry_entries()?;
    let retained = entries
        .into_iter()
        .filter(|entry| entry != &path)
        .collect::<Vec<_>>();
    write_registry_entries(&retained)
}

pub fn registry_entries() -> io::Result<Vec<PathBuf>> {
    let registry = registry_path();
    let contents = match fs::read_to_string(registry) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    Ok(contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

pub fn write_registry_entries(entries: &[PathBuf]) -> io::Result<()> {
    let registry = registry_path();
    if let Some(parent) = registry.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut body = String::new();
    for entry in entries {
        body.push_str(&entry.display().to_string());
        body.push('\n');
    }
    fs::write(registry, body)
}

pub fn descriptor_candidates(paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return registry_entries();
    }

    let mut candidates = Vec::new();
    for path in paths {
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    candidates.push(entry.path());
                }
            }
        } else {
            candidates.push(path.clone());
        }
    }
    Ok(candidates)
}

pub fn normalize_path(path: &Path) -> io::Result<PathBuf> {
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()?.join(path)
            }
        }
        Err(err) => return Err(err),
    };
    Ok(clean_display_path(path))
}

#[cfg(windows)]
fn clean_display_path(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix("\\\\?\\") {
        return PathBuf::from(rest);
    }
    path
}

#[cfg(not(windows))]
fn clean_display_path(path: PathBuf) -> PathBuf {
    path
}

fn registry_path() -> PathBuf {
    if let Some(path) = std::env::var_os("DTACH_RS_REGISTRY") {
        return PathBuf::from(path);
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data)
                .join("dtach-rs")
                .join("sessions.txt");
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
            return PathBuf::from(state_home).join("dtach-rs").join("sessions");
        }
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("dtach-rs")
                .join("sessions");
        }
    }

    std::env::temp_dir().join("dtach-rs").join("sessions.txt")
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wrong_magic() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("dtach-rs-test-{}", std::process::id()));
        fs::write(&path, "wrong\n").unwrap();
        let err = read_descriptor(&path).unwrap_err();
        remove_descriptor(&path);
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}

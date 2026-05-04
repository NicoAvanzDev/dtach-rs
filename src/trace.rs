use std::fs::OpenOptions;
use std::io::Write;

pub fn event(message: impl AsRef<str>) {
    let Ok(path) = std::env::var("DTACH_RS_LOG") else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(
        file,
        "[{}:{}] {}",
        std::process::id(),
        std::thread::current().name().unwrap_or("thread"),
        message.as_ref()
    );
}

//! Single-instance handling and the control socket used by `rlauncher-toggle`.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Toggle,
    Show,
    Hide,
    Settings,
    Reload,
    Quit,
}

impl Command {
    pub fn parse(s: &str) -> Option<Command> {
        Some(match s.trim() {
            "toggle" => Command::Toggle,
            "show" => Command::Show,
            "hide" => Command::Hide,
            "settings" => Command::Settings,
            "reload" => Command::Reload,
            "quit" => Command::Quit,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Command::Toggle => "toggle",
            Command::Show => "show",
            Command::Hide => "hide",
            Command::Settings => "settings",
            Command::Reload => "reload",
            Command::Quit => "quit",
        }
    }
}

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("rlauncher-{}", unsafe { libc::getuid() })));
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    // WAYLAND_DISPLAY may be an absolute path
    let display = display.rsplit('/').next().unwrap_or("wayland-0").to_string();
    dir.join(format!("rlauncher-{display}.sock"))
}

/// Sends a command (with an optional query for `show`) to the running instance.
pub fn send(cmd: Command, query: Option<&str>) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut msg = cmd.as_str().to_string();
    if let Some(q) = query {
        msg.push('\n');
        msg.push_str(q);
    }
    stream.write_all(msg.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    Ok(())
}

/// Binds the control socket. Returns `None` if another instance already owns it.
pub fn bind() -> std::io::Result<Option<UnixListener>> {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match UnixListener::bind(&path) {
        Ok(l) => Ok(Some(l)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                return Ok(None);
            }
            // stale socket from a crashed instance
            std::fs::remove_file(&path)?;
            UnixListener::bind(&path).map(Some)
        }
        Err(e) => Err(e),
    }
}

/// Reads a single command (and optional query) from an accepted connection.
pub fn read_command(stream: UnixStream) -> Option<(Command, Option<String>)> {
    stream.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
    let mut buf = String::new();
    stream.take(4096).read_to_string(&mut buf).ok()?;
    let (cmd, query) = match buf.split_once('\n') {
        Some((c, q)) => (c, Some(q.to_string())),
        None => (buf.as_str(), None),
    };
    Some((Command::parse(cmd)?, query))
}

pub fn cleanup() {
    let _ = std::fs::remove_file(socket_path());
}

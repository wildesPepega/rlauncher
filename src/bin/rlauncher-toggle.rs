//! Tiny client to bind to a hotkey: toggles the launcher, starting it if needed.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

#[path = "../ipc.rs"]
#[allow(dead_code)]
mod ipc;

fn main() {
    let cmd = match std::env::args().nth(1).as_deref() {
        Some("show") => ipc::Command::Show,
        Some("hide") => ipc::Command::Hide,
        _ => ipc::Command::Toggle,
    };
    if let Ok(mut s) = UnixStream::connect(ipc::socket_path())
        && s.write_all(cmd.as_str().as_bytes()).is_ok()
    {
        return;
    }
    if cmd == ipc::Command::Hide {
        return;
    }
    // Not running: start the daemon and show it right away.
    let exe = std::env::current_exe()
        .ok()
        .map(|e| e.with_file_name("rlauncher"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("rlauncher"));
    let mut child = std::process::Command::new(exe);
    child.arg("--show");
    unsafe {
        child.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Err(e) = child.spawn() {
        eprintln!("rlauncher-toggle: cannot start rlauncher: {e}");
        std::process::exit(1);
    }
}

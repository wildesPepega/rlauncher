mod apps;
mod calc;
mod config;
mod fuzzy;
mod hotkey;
mod icons;
mod ipc;
mod search;
mod settings;
mod theme;
mod ui;
mod wayland;

use ipc::Command;

const USAGE: &str = "\
rlauncher - application launcher for Wayland

USAGE:
    rlauncher [--show]            start the launcher daemon (hidden, or shown with --show)
    rlauncher toggle|show|hide    control the running daemon (starts it if needed)
    rlauncher show <query>        show with a prefilled query, e.g. `rlauncher show ~/`
    rlauncher settings [tab]      open the settings (general|hotkey|queries|appearance)
    rlauncher reload|quit         reload config / stop the daemon
    rlauncher themes              list available themes (Ulauncher themes included)
    rlauncher shortcuts           list custom queries
    rlauncher shortcuts add <keyword> <name> <command> [--no-arg] [--default-search] [--icon <icon>]
    rlauncher shortcuts remove <keyword>

Config:    ~/.config/rlauncher/config.toml
Queries:   ~/.config/rlauncher/shortcuts.json (Ulauncher format)
Themes:    ~/.config/rlauncher/themes/, ~/.config/ulauncher/user-themes/

Commands support %s (replaced by the argument), scripts starting with #! and http(s) URLs.
";

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn,zbus=warn,tracing=warn,usvg=error"))
        .format_timestamp(None)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str);
    let result = match first {
        None => run_daemon(false),
        Some("--show") => run_daemon(true),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("themes") => {
            let current = config::Config::load().theme;
            for t in theme::list_themes() {
                let mark = if t.name == current { "*" } else { " " };
                let dir = t.dir.map(|d| d.display().to_string()).unwrap_or_else(|| "builtin".into());
                println!("{mark} {:<28} {:<32} {dir}", t.name, t.display_name);
            }
            Ok(())
        }
        Some("shortcuts") => shortcuts_cli(&args[1..]),
        Some(cmd) => match Command::parse(cmd) {
            Some(c) => control(c, (args.len() > 1).then(|| args[1..].join(" "))),
            None => {
                eprint!("unknown command '{cmd}'\n\n{USAGE}");
                std::process::exit(2);
            }
        },
    };
    if let Err(e) = result {
        eprintln!("rlauncher: {e:#}");
        std::process::exit(1);
    }
}

fn run_daemon(show: bool) -> anyhow::Result<()> {
    // Overlay layers like MangoHud would draw their HUD into the launcher. The variable is
    // removed again for launched apps (see apps::run_detached).
    if std::env::var_os("DISABLE_MANGOHUD").is_none() {
        // SAFETY: set before any other thread is started.
        unsafe { std::env::set_var("DISABLE_MANGOHUD", "1") };
        apps::STRIP_DISABLE_MANGOHUD.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let Some(listener) = ipc::bind()? else {
        // Already running: just forward the request.
        if show {
            ipc::send(Command::Show, None)?;
        } else {
            log::info!("rlauncher is already running");
        }
        return Ok(());
    };
    unsafe {
        // Children are reaped by their waiter threads; ignore SIGPIPE from closed sockets.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let result = wayland::run(listener, show);
    ipc::cleanup();
    result
}

/// Sends a command to the daemon, starting it when it isn't running yet.
fn control(cmd: Command, query: Option<String>) -> anyhow::Result<()> {
    if ipc::send(cmd, query.as_deref()).is_ok() {
        return Ok(());
    }
    match cmd {
        Command::Toggle | Command::Show | Command::Settings => {
            start_daemon_detached()?;
            if query.is_some() || cmd == Command::Settings {
                // give the new instance a moment to bind its socket
                for _ in 0..50 {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                    if ipc::send(cmd, query.as_deref()).is_ok() {
                        break;
                    }
                }
            }
            Ok(())
        }
        _ => anyhow::bail!("rlauncher is not running"),
    }
}

pub fn start_daemon_detached() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    // rlauncher-toggle lives next to rlauncher
    let exe = if exe.file_name().is_some_and(|n| n != "rlauncher") {
        exe.with_file_name("rlauncher")
    } else {
        exe
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--show").stdin(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}

fn shortcuts_cli(args: &[String]) -> anyhow::Result<()> {
    let mut list = config::load_shortcuts();
    match args.first().map(String::as_str) {
        None | Some("list") => {
            for s in &list {
                let mut flags = Vec::new();
                if s.run_without_argument {
                    flags.push("no-arg");
                }
                if s.is_default_search {
                    flags.push("default-search");
                }
                println!("{:<10} {:<28} {:<16} {}", s.keyword, s.name, flags.join(","), s.cmd.replace('\n', "⏎"));
            }
            println!("\nEdit {} for scripts and icons.", config::shortcuts_path().display());
        }
        Some("add") => {
            let mut pos = Vec::new();
            let mut no_arg = false;
            let mut default_search = false;
            let mut icon = None;
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--no-arg" => no_arg = true,
                    "--default-search" => default_search = true,
                    "--icon" => icon = it.next().cloned(),
                    _ => pos.push(a.clone()),
                }
            }
            let [keyword, name, cmd] = pos.as_slice() else {
                anyhow::bail!("usage: rlauncher shortcuts add <keyword> <name> <command> [--no-arg] [--default-search] [--icon <icon>]");
            };
            list.retain(|s| &s.keyword != keyword);
            list.push(config::Shortcut {
                id: config::new_id(),
                name: name.clone(),
                keyword: keyword.clone(),
                cmd: cmd.clone(),
                icon,
                is_default_search: default_search,
                run_without_argument: no_arg,
                added: config::now_secs(),
            });
            config::save_shortcuts(&list)?;
            println!("added '{keyword}'");
        }
        Some("remove") => {
            let Some(keyword) = args.get(1) else { anyhow::bail!("usage: rlauncher shortcuts remove <keyword>") };
            let before = list.len();
            list.retain(|s| &s.keyword != keyword);
            if list.len() == before {
                anyhow::bail!("no shortcut with keyword '{keyword}'");
            }
            config::save_shortcuts(&list)?;
            println!("removed '{keyword}'");
        }
        Some(other) => anyhow::bail!("unknown shortcuts command '{other}'"),
    }
    Ok(())
}

//! Desktop entry discovery, parsing and process launching.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct App {
    /// Desktop file id, e.g. `org.kde.dolphin.desktop`.
    pub id: String,
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub keywords: Vec<String>,
    pub exec: String,
    pub icon: Option<String>,
    pub terminal: bool,
    pub workdir: Option<String>,
    pub file: PathBuf,
    /// Lowercase binary name from `Exec`, used as an extra search term.
    pub exec_name: String,
}

impl App {
    pub fn key(&self) -> String {
        format!("app:{}", self.id)
    }
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = dirs::data_dir() {
        dirs.push(d.join("applications"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        let p = Path::new(d).join("applications");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    // Flatpak exports are not always part of XDG_DATA_DIRS when started from a service.
    for p in [
        dirs::data_dir().map(|d| d.join("flatpak/exports/share/applications")),
        Some(PathBuf::from("/var/lib/flatpak/exports/share/applications")),
    ]
    .into_iter()
    .flatten()
    {
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    dirs
}

/// Scans all application directories. Earlier directories override later ones.
pub fn scan(blacklist: &[String]) -> Vec<App> {
    let desktops = current_desktops();
    let locales = locale_variants();
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut apps = Vec::new();
    for dir in application_dirs() {
        let mut files = Vec::new();
        collect_desktop_files(&dir, &dir, blacklist, &mut files, 0);
        for (id, path) in files {
            if seen.insert(id.clone(), ()).is_some() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if let Some(app) = parse_desktop_entry(&id, &path, &text, &desktops, &locales) {
                apps.push(app);
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

fn collect_desktop_files(
    root: &Path,
    dir: &Path,
    blacklist: &[String],
    out: &mut Vec<(String, PathBuf)>,
    depth: usize,
) {
    if depth > 4 || blacklist.iter().any(|b| dir.starts_with(b)) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(root, &path, blacklist, out, depth + 1);
        } else if path.extension().is_some_and(|e| e == "desktop") {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let id = rel.to_string_lossy().replace('/', "-");
            out.push((id, path));
        }
    }
}

fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// `de_DE.UTF-8@euro` -> ["de_DE@euro", "de_DE", "de@euro", "de"]
fn locale_variants() -> Vec<String> {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default();
    let (base, modifier) = match lang.split_once('@') {
        Some((b, m)) => (b, Some(m)),
        None => (lang.as_str(), None),
    };
    let base = base.split('.').next().unwrap_or("");
    let lang_only = base.split('_').next().unwrap_or("");
    let mut out = Vec::new();
    if let Some(m) = modifier {
        out.push(format!("{base}@{m}"));
    }
    out.push(base.to_string());
    if let Some(m) = modifier {
        out.push(format!("{lang_only}@{m}"));
    }
    out.push(lang_only.to_string());
    out.retain(|s| !s.is_empty() && s != "C" && s != "POSIX");
    out.dedup();
    out
}

/// Unescapes a desktop entry string value (`\s`, `\n`, `\t`, `\r`, `\\`).
fn unescape_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_desktop_entry(
    id: &str,
    path: &Path,
    text: &str,
    desktops: &[String],
    locales: &[String],
) -> Option<App> {
    let mut in_entry = false;
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if in_entry {
                break;
            }
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            fields.entry(k.trim()).or_insert(v.trim());
        }
    }
    let get = |k: &str| fields.get(k).map(|v| unescape_value(v));
    let localized = |k: &str| {
        locales
            .iter()
            .find_map(|l| fields.get(format!("{k}[{l}]").as_str()).map(|v| unescape_value(v)))
            .or_else(|| get(k))
    };
    let is_true = |k: &str| fields.get(k).is_some_and(|v| *v == "true");

    if fields.get("Type").copied().unwrap_or("Application") != "Application" {
        return None;
    }
    if is_true("NoDisplay") || is_true("Hidden") {
        return None;
    }
    let list = |k: &str| -> Vec<String> {
        get(k)
            .unwrap_or_default()
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    };
    let only = list("OnlyShowIn");
    if !only.is_empty() && !only.iter().any(|d| desktops.contains(&d.to_lowercase())) {
        return None;
    }
    if list("NotShowIn").iter().any(|d| desktops.contains(&d.to_lowercase())) {
        return None;
    }
    if let Some(try_exec) = get("TryExec")
        && !try_exec.is_empty()
        && find_in_path(&try_exec).is_none()
    {
        return None;
    }
    let exec = get("Exec")?;
    let name = localized("Name")?;
    let exec_name = split_exec(&exec)
        .first()
        .and_then(|a| Path::new(a).file_name().map(|f| f.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let keywords = localized("Keywords")
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    Some(App {
        id: id.to_string(),
        name,
        generic_name: localized("GenericName"),
        comment: localized("Comment"),
        keywords,
        exec,
        icon: get("Icon").filter(|s| !s.is_empty()),
        terminal: is_true("Terminal"),
        workdir: get("Path").filter(|s| !s.is_empty()),
        file: path.to_path_buf(),
        exec_name,
    })
}

pub fn find_in_path(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return p.exists().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// Splits an `Exec` value into arguments, following the desktop entry quoting rules.
fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            '\\' if in_quotes => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(cur);
    }
    args
}

/// Builds the argv for an app, expanding/removing field codes.
pub fn app_argv(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    for arg in split_exec(&app.exec) {
        match arg.as_str() {
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => continue,
            "%i" => {
                if let Some(icon) = &app.icon {
                    out.push("--icon".into());
                    out.push(icon.clone());
                }
                continue;
            }
            _ => {}
        }
        let mut s = String::new();
        let mut chars = arg.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                s.push(c);
                continue;
            }
            match chars.next() {
                Some('%') => s.push('%'),
                Some('c') => s.push_str(&app.name),
                Some('k') => s.push_str(&app.file.to_string_lossy()),
                Some(_) | None => {}
            }
        }
        out.push(s);
    }
    out
}

/// The terminal command (as argv prefix) used to run terminal apps.
pub fn terminal_prefix(configured: &str) -> Vec<String> {
    if !configured.trim().is_empty() {
        return split_exec(configured);
    }
    let candidates: [(&str, &[&str]); 9] = [
        ("kitty", &[]),
        ("foot", &[]),
        ("alacritty", &["-e"]),
        ("wezterm", &["start", "--"]),
        ("ghostty", &["-e"]),
        ("konsole", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("xfce4-terminal", &["-x"]),
        ("xterm", &["-e"]),
    ];
    if let Ok(t) = std::env::var("TERMINAL")
        && find_in_path(&t).is_some()
    {
        let base = Path::new(&t).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let extra = candidates.iter().find(|(n, _)| *n == base).map(|(_, e)| *e).unwrap_or(&["-e"]);
        return std::iter::once(t.clone()).chain(extra.iter().map(|s| s.to_string())).collect();
    }
    for (bin, extra) in candidates {
        if find_in_path(bin).is_some() {
            return std::iter::once(bin.to_string()).chain(extra.iter().map(|s| s.to_string())).collect();
        }
    }
    vec!["xterm".into(), "-e".into()]
}

/// What to start once the launcher has an activation token.
#[derive(Debug, Clone)]
pub enum LaunchSpec {
    Argv { argv: Vec<String>, cwd: Option<String>, terminal: bool },
    /// A shell snippet, run like Ulauncher does: written to a temp file and executed with
    /// the argument appended.
    Script { script: String, args: Option<String> },
    Open(String),
}

/// Spawns a fully detached process. `token` is an XDG activation token, which allows the
/// started app to take focus on Wayland. `terminal` is the configured terminal command.
pub fn spawn(spec: &LaunchSpec, token: Option<&str>, terminal: &str) {
    let result = match spec {
        LaunchSpec::Argv { argv, cwd, terminal: in_terminal } => {
            let argv: Vec<String> = if *in_terminal {
                terminal_prefix(terminal).into_iter().chain(argv.iter().cloned()).collect()
            } else {
                argv.clone()
            };
            let Some((prog, args)) = argv.split_first() else { return };
            let mut cmd = Command::new(prog);
            cmd.args(args);
            let cwd = cwd.as_ref().map(PathBuf::from).or_else(dirs::home_dir);
            if let Some(cwd) = cwd.filter(|c| c.is_dir()) {
                cmd.current_dir(cwd);
            }
            run_detached(cmd, token)
        }
        LaunchSpec::Open(target) => {
            let mut cmd = Command::new("xdg-open");
            cmd.arg(target);
            if let Some(home) = dirs::home_dir() {
                cmd.current_dir(home);
            }
            run_detached(cmd, token)
        }
        LaunchSpec::Script { script, args } => run_script(script, args.as_deref(), token),
    };
    if let Err(e) = result {
        log::error!("failed to launch {spec:?}: {e}");
    }
}

/// Set when rlauncher disabled MangoHud for itself; launched apps get the user's setting.
pub static STRIP_DISABLE_MANGOHUD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn run_detached(mut cmd: Command, token: Option<&str>) -> std::io::Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if STRIP_DISABLE_MANGOHUD.load(std::sync::atomic::Ordering::Relaxed) {
        cmd.env_remove("DISABLE_MANGOHUD");
    }
    cmd.env_remove("XDG_ACTIVATION_TOKEN").env_remove("DESKTOP_STARTUP_ID");
    if let Some(t) = token {
        cmd.env("XDG_ACTIVATION_TOKEN", t).env("DESKTOP_STARTUP_ID", t);
    }
    // New session: the app must survive rlauncher and not receive its signals.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            // Restore default signal handling for the child.
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // Reap the child when it exits so no zombies are left behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn run_script(script: &str, args: Option<&str>, token: Option<&str>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = dir.join(format!("rlauncher_script_{}", crate::config::new_id()));
    std::fs::write(&file, script)?;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700))?;
    let mut line = shell_quote(&file.to_string_lossy());
    if let Some(a) = args.filter(|a| !a.is_empty()) {
        // Arguments are passed "as written", exactly like Ulauncher does.
        line.push(' ');
        line.push_str(a);
    }
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(line);
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    run_detached(cmd, token)?;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = std::fs::remove_file(file);
    });
    Ok(())
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_parsing() {
        assert_eq!(split_exec(r#"foo "bar baz" %U"#), vec!["foo", "bar baz", "%U"]);
        assert_eq!(split_exec(r#"sh -c "echo \"hi\"""#), vec!["sh", "-c", "echo \"hi\""]);
        let app = App {
            id: "x.desktop".into(),
            name: "X".into(),
            generic_name: None,
            comment: None,
            keywords: vec![],
            exec: "x --name=%c %F --pct 100%%".into(),
            icon: Some("x".into()),
            terminal: false,
            workdir: None,
            file: "/a/x.desktop".into(),
            exec_name: "x".into(),
        };
        assert_eq!(app_argv(&app), vec!["x", "--name=X", "--pct", "100%"]);
    }

    #[test]
    fn desktop_entry() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\nExec=nautilus %U\nIcon=folder\nKeywords=folder;manager;\n[Desktop Action new]\nName=New\n";
        let app = parse_desktop_entry("f.desktop", Path::new("/f.desktop"), text, &[], &["de".into()]).unwrap();
        assert_eq!(app.name, "Dateien");
        assert_eq!(app.keywords, vec!["folder", "manager"]);
        assert_eq!(app.exec_name, "nautilus");
    }
}

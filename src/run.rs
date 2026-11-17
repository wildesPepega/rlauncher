//! Logic of the "Run" dialog layout: the run history (most recently used first) and
//! turning a typed line into something to launch, like the Windows "Run" dialog does.

use std::path::{Path, PathBuf};

use crate::apps::{self, App, LaunchSpec};
use crate::config::{self, Shortcut};

/// Windows keeps 26 entries (a–z) in its RunMRU list.
const MAX_HISTORY: usize = 26;

fn history_path() -> PathBuf {
    config::data_dir().join("run_history.json")
}

pub fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default()
}

fn save_history(history: &[String]) {
    let path = history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(history)
        && let Err(e) = std::fs::write(&path, text)
    {
        log::warn!("cannot save run history: {e}");
    }
}

/// Moves `entry` to the front of the history (case-insensitively deduplicated).
pub fn push_history(history: &mut Vec<String>, entry: &str) {
    let entry = entry.trim();
    if entry.is_empty() {
        return;
    }
    history.retain(|h| !h.eq_ignore_ascii_case(entry));
    history.insert(0, entry.to_string());
    history.truncate(MAX_HISTORY);
    save_history(history);
}

/// History entries that start with `typed` (case-insensitive), in MRU order.
pub fn matches<'a>(history: &'a [String], typed: &str) -> Vec<&'a str> {
    let t = typed.trim_start().to_lowercase();
    history
        .iter()
        .filter(|h| t.is_empty() || (h.to_lowercase().starts_with(&t) && h.len() > t.len()))
        .map(String::as_str)
        .collect()
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest.trim_start_matches('/')),
        None => PathBuf::from(p),
    }
}

fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn shell(text: &str, app_id: &str) -> LaunchSpec {
    LaunchSpec::Argv {
        argv: vec!["/bin/sh".into(), "-c".into(), text.to_string()],
        cwd: None,
        terminal: false,
        app_id: Some(app_id.to_string()),
    }
}

/// What to do with a line typed into the Run dialog. `None` means nothing was found.
pub fn resolve(text: &str, apps: &[App], shortcuts: &[Shortcut]) -> Option<LaunchSpec> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (first, rest) = match text.split_once(char::is_whitespace) {
        Some((f, r)) => (f, r.trim()),
        None => (text, ""),
    };

    // Custom queries: "reboot", "g cats", …
    if let Some(s) = shortcuts.iter().find(|s| !s.keyword.is_empty() && s.keyword.eq_ignore_ascii_case(first)) {
        let arg = (!rest.is_empty()).then_some(rest);
        return Some(crate::search::shortcut_spec(s, arg));
    }

    // URLs
    if text.contains("://") || text.starts_with("mailto:") {
        return Some(LaunchSpec::Open(text.to_string()));
    }
    if text.starts_with("www.") && !text.contains(char::is_whitespace) {
        return Some(LaunchSpec::Open(format!("https://{text}")));
    }

    // Files and folders, also "quoted paths" as inserted by Browse…
    let unquoted = text.trim_matches('"');
    if unquoted.starts_with('/') || unquoted.starts_with('~') || unquoted.starts_with("./") {
        let path = expand_tilde(unquoted);
        if is_executable_file(&path) {
            return Some(LaunchSpec::Argv {
                argv: vec![path.to_string_lossy().into_owned()],
                cwd: path.parent().map(|p| p.to_string_lossy().into_owned()),
                terminal: false,
                app_id: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            });
        }
        if path.exists() {
            return Some(LaunchSpec::Open(path.to_string_lossy().into_owned()));
        }
        // "/path/to/script --flag": an executable followed by arguments
        let bin = expand_tilde(first);
        if is_executable_file(&bin) {
            return Some(shell(text, &bin.file_name().unwrap_or_default().to_string_lossy()));
        }
        return None;
    }

    // Programs in $PATH, run through the shell so arguments, pipes etc. work
    if !first.contains('/') && apps::find_in_path(first).is_some() {
        return Some(shell(text, first));
    }

    // Apps by name, desktop id or binary ("Steam", "org.kde.dolphin", …)
    let lower = text.to_lowercase();
    let app = apps.iter().find(|a| a.name.to_lowercase() == lower).or_else(|| {
        apps.iter().find(|a| a.id.trim_end_matches(".desktop").to_lowercase() == lower || a.exec_name == lower)
    });
    app.map(|a| LaunchSpec::Argv {
        argv: apps::app_argv(a),
        cwd: a.workdir.clone(),
        terminal: a.terminal,
        app_id: Some(a.id.trim_end_matches(".desktop").to_string()),
    })
}

/// Turns a `file://` URI from the file chooser into a path.
pub fn uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// How a chosen file is put into the input field: quoted when it contains spaces.
pub fn quote_path(path: &str) -> String {
    if path.contains(char::is_whitespace) { format!("\"{path}\"") } else { path.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(keyword: &str, cmd: &str) -> Shortcut {
        Shortcut {
            id: keyword.into(),
            name: keyword.into(),
            keyword: keyword.into(),
            cmd: cmd.into(),
            icon: None,
            is_default_search: false,
            run_without_argument: true,
            added: 0.0,
        }
    }

    #[test]
    fn resolves_like_the_windows_run_dialog() {
        let shortcuts = vec![shortcut("reboot", "systemctl reboot"), shortcut("g", "https://google.com/search?q=%s")];
        assert!(matches!(resolve("reboot", &[], &shortcuts), Some(LaunchSpec::Script { .. })));
        assert!(
            matches!(resolve("g rust egui", &[], &shortcuts), Some(LaunchSpec::Open(u)) if u == "https://google.com/search?q=rust+egui")
        );
        assert!(matches!(resolve("www.example.org", &[], &[]), Some(LaunchSpec::Open(u)) if u == "https://www.example.org"));
        assert!(matches!(resolve("/", &[], &[]), Some(LaunchSpec::Open(p)) if p == "/"));
        assert!(matches!(resolve("sh -c true", &[], &[]), Some(LaunchSpec::Argv { argv, .. }) if argv[2] == "sh -c true"));
        assert!(resolve("definitely-not-a-program-xyz", &[], &[]).is_none());
        assert!(resolve("/does/not/exist", &[], &[]).is_none());
        assert!(resolve("   ", &[], &[]).is_none());
    }

    #[test]
    fn history_matches_are_prefixes() {
        let h: Vec<String> = ["firefox", "Fish", "kate", "f"].iter().map(|s| s.to_string()).collect();
        assert_eq!(matches(&h, "f"), vec!["firefox", "Fish"]);
        assert_eq!(matches(&h, "").len(), 4);
        assert!(matches(&h, "firefox").is_empty());
    }

    #[test]
    fn file_uris_are_decoded() {
        assert_eq!(uri_to_path("file:///home/a%20b/x.sh").as_deref(), Some("/home/a b/x.sh"));
        assert_eq!(quote_path("/home/a b/x.sh"), "\"/home/a b/x.sh\"");
        assert_eq!(uri_to_path("https://x"), None);
    }
}

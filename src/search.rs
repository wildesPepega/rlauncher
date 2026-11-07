//! Turns a query into a ranked list of result items.

use std::path::{Path, PathBuf};

use crate::apps::{self, App, LaunchSpec};
use crate::calc;
use crate::config::{Config, Shortcut, Stats};
use crate::fuzzy;

/// Results below this score are dropped (same threshold as Ulauncher).
const MIN_SCORE: f32 = 50.0;
const MAX_ITEMS: usize = 40;

#[derive(Debug, Clone)]
pub enum Action {
    Launch(LaunchSpec),
    /// Replace the query, e.g. to enter a shortcut keyword or a directory.
    SetQuery(String),
    Copy(String),
}

#[derive(Debug, Clone)]
pub struct ResultItem {
    /// Stable key used for statistics and icon caching.
    pub key: String,
    pub title: String,
    /// Char indices into `title` to highlight.
    pub highlight: Vec<usize>,
    pub description: String,
    /// Icon name or absolute path.
    pub icon: Option<String>,
    pub action: Action,
    /// Alternative action (Alt+Enter).
    pub alt_action: Option<Action>,
    /// Whether choosing this item is recorded in the statistics.
    pub record: bool,
}

pub struct Context<'a> {
    pub apps: &'a [App],
    pub shortcuts: &'a [Shortcut],
    pub stats: &'a Stats,
    pub config: &'a Config,
}

pub fn search(query: &str, ctx: &Context) -> Vec<ResultItem> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    // A shortcut keyword followed by a space (or alone, for argument-less shortcuts)
    // switches to that shortcut only.
    for s in ctx.shortcuts {
        if s.keyword.is_empty() {
            continue;
        }
        if query == s.keyword && s.run_without_argument {
            return vec![shortcut_item(s, query, None, false, Vec::new())];
        }
        if let Some(arg) = query.strip_prefix(&format!("{} ", s.keyword)) {
            return vec![shortcut_item(s, query, Some(arg), false, Vec::new())];
        }
    }

    if ctx.config.file_browser && (query.starts_with('/') || query.starts_with('~')) {
        return file_results(query);
    }

    if ctx.config.calculator
        && calc::looks_like_math(query)
        && let Some(v) = calc::eval(query)
    {
        let result = calc::format_number(v);
        return vec![ResultItem {
            key: "calc".into(),
            title: result.clone(),
            highlight: Vec::new(),
            description: "Enter to copy to the clipboard".into(),
            icon: Some("accessories-calculator".into()),
            action: Action::Copy(result),
            alt_action: None,
            record: false,
        }];
    }

    let q = query.trim();
    let q_lower = q.to_lowercase();
    let preferred = ctx.stats.history.get(&q_lower);
    let mut scored: Vec<(f32, ResultItem)> = Vec::new();

    for app in ctx.apps {
        // The name is matched fuzzily (like Ulauncher); secondary fields only count when
        // the query is a prefix of one of their words, so they don't add noise.
        let name_score = if fuzzy::plausible(q, &app.name) { fuzzy::score(q, &app.name) } else { 0.0 };
        let mut s = name_score;
        if let Some(g) = &app.generic_name
            && fuzzy::plausible(q, g)
            && word_prefix(g, &q_lower)
        {
            s = s.max(fuzzy::score(q, g) * 0.85);
        }
        if app.exec_name.starts_with(&q_lower) {
            s = s.max(fuzzy::score(q, &app.exec_name) * 0.9);
        }
        if app.keywords.iter().any(|k| word_prefix(k, &q_lower)) {
            s = s.max(70.0);
        }
        if s < MIN_SCORE {
            continue;
        }
        let mut item = app_item(app, q);
        if name_score < s {
            // matched through another field: highlighting the name would be misleading
            item.highlight.clear();
        }
        scored.push((rank(s, &item.key, preferred, ctx.stats), item));
    }

    for sc in ctx.shortcuts {
        let mut s = if fuzzy::plausible(q, &sc.name) { fuzzy::score(q, &sc.name) } else { 0.0 };
        if !sc.keyword.is_empty() {
            if fuzzy::plausible(q, &sc.keyword) {
                s = s.max(fuzzy::score(q, &sc.keyword));
            }
            if sc.keyword.eq_ignore_ascii_case(q) {
                s = 100.0;
            }
        }
        if s < MIN_SCORE {
            continue;
        }
        let item = shortcut_item(sc, query, None, false, fuzzy::matching_indexes(q, &sc.name));
        scored.push((rank(s, &item.key, preferred, ctx.stats), item));
    }

    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    let mut items: Vec<ResultItem> = scored.into_iter().map(|(_, i)| i).take(MAX_ITEMS).collect();

    // Default searches ("Google Search for …") are always offered at the end.
    for sc in ctx.shortcuts.iter().filter(|s| s.is_default_search) {
        let key = format!("default:{}", sc.id);
        if !items.iter().any(|i| i.key == key) {
            items.push(shortcut_item(sc, query, None, true, Vec::new()));
        }
    }
    items
}

fn word_prefix(text: &str, query_lower: &str) -> bool {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(query_lower))
}

fn rank(score: f32, key: &str, preferred: Option<&String>, stats: &Stats) -> f32 {
    let mut r = score;
    if preferred.is_some_and(|p| p == key) {
        r += 100.0;
    }
    let launches = stats.launches.get(key).copied().unwrap_or(0).min(20);
    r + launches as f32 * 0.5
}

fn app_item(app: &App, query: &str) -> ResultItem {
    ResultItem {
        key: app.key(),
        title: app.name.clone(),
        highlight: fuzzy::matching_indexes(query, &app.name),
        description: app
            .comment
            .clone()
            .or_else(|| app.generic_name.clone())
            .unwrap_or_else(|| app.exec.clone()),
        icon: app.icon.clone().or_else(|| Some("application-x-executable".into())),
        action: Action::Launch(LaunchSpec::Argv {
            argv: apps::app_argv(app),
            cwd: app.workdir.clone(),
            terminal: app.terminal,
            app_id: Some(app.id.trim_end_matches(".desktop").to_string()),
        }),
        alt_action: Some(Action::Launch(LaunchSpec::Open(
            app.file.parent().unwrap_or(Path::new("/")).to_string_lossy().into_owned(),
        ))),
        record: true,
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Builds a shortcut item. `argument` is set when the shortcut keyword is active,
/// `default_search` when it is offered as a fallback for the whole query.
fn shortcut_item(
    s: &Shortcut,
    query: &str,
    argument: Option<&str>,
    default_search: bool,
    highlight: Vec<usize>,
) -> ResultItem {
    let argument = argument.filter(|a| !a.is_empty());
    let keyword_active = argument.is_some() || (query == s.keyword);
    let base_descr = if s.cmd.starts_with("#!") { String::new() } else { s.cmd.clone() };
    let description = if default_search {
        base_descr.replace("%s", query)
    } else if let Some(arg) = argument {
        base_descr.replace("%s", arg)
    } else if keyword_active && s.run_without_argument {
        "Press Enter to run the shortcut".into()
    } else if keyword_active {
        "Type in your query and press Enter…".into()
    } else {
        base_descr.replace("%s", "…")
    };
    let description = if description.is_empty() { format!("Keyword: {}", s.keyword) } else { description };

    let arg = if default_search { Some(query) } else { argument };
    let action = match arg {
        Some(arg) => Action::Launch(shortcut_spec(s, Some(arg))),
        None if s.run_without_argument => Action::Launch(shortcut_spec(s, None)),
        None => Action::SetQuery(format!("{} ", s.keyword)),
    };
    let title = if default_search || argument.is_none() {
        s.name.clone()
    } else {
        format!("{}: {}", s.name, argument.unwrap_or_default())
    };
    ResultItem {
        key: if default_search { format!("default:{}", s.id) } else { format!("shortcut:{}", s.id) },
        title,
        highlight: if keyword_active { Vec::new() } else { highlight },
        description,
        icon: s.icon.clone().filter(|i| !i.is_empty()).or_else(|| {
            Some(if s.is_url() { "web-browser".into() } else { "utilities-terminal".into() })
        }),
        action,
        alt_action: None,
        record: true,
    }
}

fn shortcut_spec(s: &Shortcut, arg: Option<&str>) -> LaunchSpec {
    let cmd = s.cmd.trim();
    if s.is_url() {
        return LaunchSpec::Open(match arg {
            Some(a) => cmd.replace("%s", &url_encode(a)),
            None => cmd.replace("%s", ""),
        });
    }
    if !s.cmd.starts_with("#!") && s.cmd.contains("%s") {
        let quoted = arg.map(apps::shell_quote).unwrap_or_default();
        return LaunchSpec::Script { script: s.cmd.replace("%s", &quoted), args: None };
    }
    LaunchSpec::Script { script: s.cmd.clone(), args: arg.map(String::from) }
}

fn expand_tilde(q: &str) -> PathBuf {
    match q.strip_prefix('~') {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest.trim_start_matches('/')),
        None => PathBuf::from(q),
    }
}

fn file_results(query: &str) -> Vec<ResultItem> {
    let (dir_part, partial) = match query.rfind('/') {
        Some(i) => (&query[..=i], &query[i + 1..]),
        None => (query, ""), // "~" alone
    };
    let dir_part = if dir_part == "~" { "~/" } else { dir_part };
    let dir = expand_tilde(dir_part);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };

    let show_hidden = partial.starts_with('.');
    let mut list: Vec<(f32, String, bool)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && !show_hidden {
                return None;
            }
            let is_dir = e.path().is_dir();
            let score = if partial.is_empty() {
                100.0
            } else if name.to_lowercase().starts_with(&partial.to_lowercase()) {
                100.0 + 1.0 / (name.len() as f32)
            } else if fuzzy::plausible(partial, &name) {
                fuzzy::score(partial, &name)
            } else {
                0.0
            };
            (score >= MIN_SCORE).then_some((score, name, is_dir))
        })
        .collect();
    list.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });

    let mut items: Vec<ResultItem> = list
        .into_iter()
        .take(MAX_ITEMS)
        .map(|(_, name, is_dir)| {
            let path = dir.join(&name);
            let path_str = path.to_string_lossy().into_owned();
            let display_query = format!("{dir_part}{name}");
            ResultItem {
                key: format!("file:{path_str}"),
                highlight: fuzzy::matching_indexes(partial, &name),
                title: name,
                description: path_str.clone(),
                icon: Some(if is_dir { "folder".into() } else { file_icon(&path) }),
                action: if is_dir {
                    Action::SetQuery(format!("{display_query}/"))
                } else {
                    Action::Launch(LaunchSpec::Open(path_str.clone()))
                },
                alt_action: Some(Action::Launch(LaunchSpec::Open(if is_dir {
                    path_str
                } else {
                    dir.to_string_lossy().into_owned()
                }))),
                record: false,
            }
        })
        .collect();

    // Entering a directory path itself offers to open it.
    if partial.is_empty() {
        let p = dir.to_string_lossy().into_owned();
        items.insert(
            0,
            ResultItem {
                key: format!("file:{p}"),
                title: dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone()),
                highlight: Vec::new(),
                description: format!("Open {p}"),
                icon: Some("folder-open".into()),
                action: Action::Launch(LaunchSpec::Open(p)),
                alt_action: None,
                record: false,
            },
        );
    }
    items
}

fn file_icon(path: &Path) -> String {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" => "image-x-generic",
        "mp3" | "flac" | "ogg" | "wav" | "opus" | "m4a" => "audio-x-generic",
        "mp4" | "mkv" | "webm" | "avi" | "mov" => "video-x-generic",
        "pdf" => "application-pdf",
        "zip" | "tar" | "gz" | "xz" | "zst" | "7z" | "rar" => "package-x-generic",
        "sh" | "py" | "rs" | "js" | "ts" | "c" | "cpp" | "h" | "toml" | "json" => "text-x-script",
        _ => "text-x-generic",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(keyword: &str, cmd: &str, default: bool, no_arg: bool) -> Shortcut {
        Shortcut {
            id: keyword.into(),
            name: format!("Short {keyword}"),
            keyword: keyword.into(),
            cmd: cmd.into(),
            icon: None,
            is_default_search: default,
            run_without_argument: no_arg,
            added: 0.0,
        }
    }

    #[test]
    fn shortcut_keyword_activation() {
        let shortcuts = vec![sc("g", "https://google.com/search?q=%s", true, false), sc("c", "kitty", false, true)];
        let stats = Stats::default();
        let config = Config::default();
        let ctx = Context { apps: &[], shortcuts: &shortcuts, stats: &stats, config: &config };

        let r = search("g hello world", &ctx);
        assert_eq!(r.len(), 1);
        match &r[0].action {
            Action::Launch(LaunchSpec::Open(u)) => assert_eq!(u, "https://google.com/search?q=hello+world"),
            a => panic!("{a:?}"),
        }
        let r = search("c", &ctx);
        assert!(matches!(&r[0].action, Action::Launch(LaunchSpec::Script { script, .. }) if script == "kitty"));

        // unknown text falls back to the default search
        let r = search("zzzz", &ctx);
        assert_eq!(r.last().unwrap().key, "default:g");
    }

    #[test]
    fn keyword_prefix_runs_shortcut() {
        let shortcuts = vec![sc("reboot", "systemctl reboot", false, true)];
        let stats = Stats::default();
        let config = Config::default();
        let ctx = Context { apps: &[], shortcuts: &shortcuts, stats: &stats, config: &config };
        let r = search("re", &ctx);
        assert_eq!(r[0].key, "shortcut:reboot");
        assert!(matches!(&r[0].action, Action::Launch(LaunchSpec::Script { script, args: None }) if script == "systemctl reboot"));
    }

    #[test]
    fn calculator() {
        let stats = Stats::default();
        let config = Config::default();
        let ctx = Context { apps: &[], shortcuts: &[], stats: &stats, config: &config };
        let r = search("2*21", &ctx);
        assert_eq!(r[0].title, "42");
    }
}

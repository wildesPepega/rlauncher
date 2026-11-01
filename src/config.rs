//! Settings, custom shortcuts ("queries") and usage statistics.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "io.github.rlauncher";

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("rlauncher")
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("~/.local/share")).join("rlauncher")
}

fn ulauncher_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ulauncher"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Theme name (Ulauncher themes are supported, see `rlauncher themes`).
    pub theme: String,
    /// Extra folders to search for themes, in addition to ~/.config/rlauncher/themes,
    /// ~/.config/ulauncher/user-themes and /usr/share/ulauncher/themes.
    pub theme_dirs: Vec<String>,
    /// Width of the launcher window in logical pixels.
    pub width: f32,
    /// Number of results that are visible at once.
    pub max_results: usize,
    /// Vertical position of the window's top edge, as a fraction of the screen height.
    pub top_offset: f32,
    /// Terminal used for `Terminal=true` apps, e.g. "kitty" or "alacritty -e".
    /// Empty = auto-detect.
    pub terminal: String,
    /// Hide the launcher when it loses keyboard focus or when clicking outside of it.
    pub close_on_focus_out: bool,
    /// Characters used for Alt+<key> quick selection.
    pub jump_keys: String,
    /// Register a global hotkey (KDE kglobalaccel or the XDG GlobalShortcuts portal).
    #[serde(alias = "portal_hotkey")]
    pub hotkey_enabled: bool,
    /// Global hotkey (XDG shortcuts syntax, e.g. "LOGO+space").
    pub hotkey: String,
    /// Directories whose .desktop files are ignored.
    pub blacklisted_dirs: Vec<String>,
    /// Enable the inline calculator.
    pub calculator: bool,
    /// Enable the file browser for queries starting with `/` or `~`.
    pub file_browser: bool,
    /// Draw a drop shadow around the window.
    pub shadow: bool,
    /// Optional font file (ttf/otf). Empty = system "sans-serif" via fontconfig.
    pub font: String,
    /// Placeholder text of the input field.
    pub placeholder: String,
    /// The monitor the launcher opens on.
    pub monitor: Monitor,
}

/// Which monitor the launcher opens on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Monitor {
    /// The monitor under the mouse pointer.
    #[default]
    Mouse,
    /// The primary monitor (KDE's primary display, elsewhere the top-left one).
    Primary,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "dark".into(),
            theme_dirs: Vec::new(),
            width: 600.0,
            max_results: 8,
            top_offset: 0.22,
            terminal: String::new(),
            close_on_focus_out: true,
            jump_keys: "1234567890abcdefghijklmnopqrstuvwxyz".into(),
            hotkey_enabled: true,
            hotkey: "LOGO+space".into(),
            blacklisted_dirs: vec![
                "/usr/share/locale".into(),
                "/usr/share/app-install".into(),
                "/usr/share/kservices5".into(),
                "/usr/share/fk5".into(),
                "/usr/share/kservicetypes5".into(),
                "/usr/share/applications/screensavers".into(),
                "/usr/share/kde4".into(),
                "/usr/share/mimelnk".into(),
            ],
            calculator: true,
            file_browser: true,
            shadow: true,
            font: String::new(),
            placeholder: "Search apps, commands…".into(),
            monitor: Monitor::Mouse,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Loads the config. On first start, settings are imported from Ulauncher.
    pub fn load() -> Config {
        let cfg = Self::load_file();
        crate::theme::set_extra_dirs(&cfg.theme_dirs);
        cfg
    }

    fn load_file() -> Config {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    log::error!("invalid {}: {e}", path.display());
                    Config::default()
                }
            },
            Err(_) => {
                let cfg = Self::import_ulauncher().unwrap_or_default();
                if let Err(e) = cfg.save() {
                    log::warn!("cannot write {}: {e}", path.display());
                }
                cfg
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        let text = format!(
            "# rlauncher configuration\n# Changes are picked up the next time the launcher is shown.\n\n{}",
            toml::to_string_pretty(self)?
        );
        std::fs::write(Self::path(), text)?;
        Ok(())
    }

    fn import_ulauncher() -> Option<Config> {
        let text = std::fs::read_to_string(ulauncher_dir()?.join("settings.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        let mut cfg = Config::default();
        if let Some(t) = v["theme-name"].as_str() {
            cfg.theme = t.to_string();
        }
        if let Some(t) = v["terminal-command"].as_str() {
            cfg.terminal = t.to_string();
        }
        if let Some(j) = v["jump-keys"].as_str() {
            cfg.jump_keys = j.to_string();
        }
        if let Some(b) = v["close-on-focus-out"].as_bool() {
            cfg.close_on_focus_out = b;
        }
        if let Some(d) = v["blacklisted-desktop-dirs"].as_str() {
            cfg.blacklisted_dirs = d.split(':').filter(|s| !s.is_empty()).map(String::from).collect();
        }
        if let Some(h) = v["hotkey-show-app"].as_str() {
            cfg.hotkey = gtk_accel_to_xdg(h);
        }
        log::info!("imported settings from Ulauncher");
        Some(cfg)
    }
}

fn autostart_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("~/.config")).join("autostart/rlauncher.desktop")
}

pub fn autostart_enabled() -> bool {
    std::fs::read_to_string(autostart_path())
        .is_ok_and(|t| !t.lines().any(|l| l.trim() == "Hidden=true" || l.trim() == "X-GNOME-Autostart-enabled=false"))
}

/// Creates or removes the XDG autostart entry for the daemon.
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let path = autostart_path();
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let exe = exe.with_file_name("rlauncher");
    std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
    std::fs::write(
        &path,
        format!(
            "[Desktop Entry]\nType=Application\nName=rlauncher\nComment=Start the rlauncher daemon\nExec={}\nIcon=system-search\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        ),
    )?;
    Ok(())
}

/// Converts a GTK accelerator like `<Super>space` to the XDG shortcuts format `LOGO+space`.
fn gtk_accel_to_xdg(accel: &str) -> String {
    let mut parts = Vec::new();
    let mut rest = accel;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else { break };
        let m = &rest[start + 1..start + end];
        parts.push(match m.to_ascii_lowercase().as_str() {
            "super" | "meta" | "mod4" => "LOGO",
            "control" | "ctrl" | "primary" => "CTRL",
            "alt" | "mod1" => "ALT",
            "shift" => "SHIFT",
            _ => continue,
        });
        rest = &rest[start + end + 1..];
    }
    let key = rest.trim();
    let mut s = parts.join("+");
    if !key.is_empty() {
        if !s.is_empty() {
            s.push('+');
        }
        s.push_str(key);
    }
    s
}

/// A custom query. Same format as Ulauncher's `shortcuts.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shortcut {
    pub id: String,
    pub name: String,
    pub keyword: String,
    /// A command, a script (starting with `#!`) or a URL (`http(s)://`).
    /// `%s` is replaced by the query argument.
    pub cmd: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub is_default_search: bool,
    #[serde(default)]
    pub run_without_argument: bool,
    #[serde(default)]
    pub added: f64,
}

impl Shortcut {
    pub fn is_url(&self) -> bool {
        let c = self.cmd.trim_start();
        c.starts_with("http://") || c.starts_with("https://")
    }
}

pub fn shortcuts_path() -> PathBuf {
    config_dir().join("shortcuts.json")
}

pub fn load_shortcuts() -> Vec<Shortcut> {
    let path = shortcuts_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            // First start: take over Ulauncher's shortcuts, or install some defaults.
            let text = ulauncher_dir()
                .and_then(|d| std::fs::read_to_string(d.join("shortcuts.json")).ok())
                .unwrap_or_else(default_shortcuts_json);
            let _ = std::fs::create_dir_all(config_dir());
            let _ = std::fs::write(&path, &text);
            text
        }
    };
    let map: BTreeMap<String, Shortcut> = match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            log::error!("invalid {}: {e}", path.display());
            return Vec::new();
        }
    };
    let mut list: Vec<Shortcut> = map.into_values().collect();
    list.sort_by(|a, b| a.added.total_cmp(&b.added));
    list
}

pub fn save_shortcuts(list: &[Shortcut]) -> anyhow::Result<()> {
    let map: BTreeMap<&str, &Shortcut> = list.iter().map(|s| (s.id.as_str(), s)).collect();
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(shortcuts_path(), serde_json::to_string_pretty(&map)?)?;
    Ok(())
}

fn default_shortcuts_json() -> String {
    let now = now_secs();
    let list = [
        Shortcut {
            id: new_id(),
            name: "Google Search".into(),
            keyword: "g".into(),
            cmd: "https://google.com/search?q=%s".into(),
            icon: Some("web-browser".into()),
            is_default_search: true,
            run_without_argument: false,
            added: now,
        },
        Shortcut {
            id: new_id(),
            name: "Wikipedia".into(),
            keyword: "wiki".into(),
            cmd: "https://en.wikipedia.org/wiki/%s".into(),
            icon: Some("accessories-dictionary".into()),
            is_default_search: false,
            run_without_argument: false,
            added: now + 1.0,
        },
    ];
    let map: BTreeMap<&str, &Shortcut> = list.iter().map(|s| (s.id.as_str(), s)).collect();
    serde_json::to_string_pretty(&map).unwrap_or_else(|_| "{}".into())
}

pub fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A random UUID v4 (formatted like Ulauncher's ids).
pub fn new_id() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut b);
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Launch counts and per-query selection history, used to rank results.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub launches: HashMap<String, u32>,
    /// lowercase query -> key of the item that was chosen for it last time
    pub history: HashMap<String, String>,
}

impl Stats {
    fn path() -> PathBuf {
        data_dir().join("stats.json")
    }

    pub fn load() -> Stats {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn record(&mut self, query: &str, key: &str) {
        *self.launches.entry(key.to_string()).or_default() += 1;
        let q = query.trim().to_lowercase();
        if !q.is_empty() {
            self.history.insert(q, key.to_string());
        }
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(Self::path(), text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accel_conversion() {
        assert_eq!(gtk_accel_to_xdg("<Super>space"), "LOGO+space");
        assert_eq!(gtk_accel_to_xdg("<Primary><Alt>r"), "CTRL+ALT+r");
    }
}

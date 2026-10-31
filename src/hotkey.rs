//! Global hotkey.
//!
//! On KDE Plasma the shortcut is registered natively with kglobalaccel, which lets the
//! settings dialog set the key directly and detect conflicts. Elsewhere the XDG
//! GlobalShortcuts portal is used (GNOME 48+, Hyprland, …). Compositors without either can
//! bind `rlauncher-toggle`.
//!
//! Triggers are stored in the XDG shortcuts format, e.g. `LOGO+space` or `CTRL+ALT+r`.

use std::sync::{Arc, Mutex};

/// Status shown in the settings dialog.
#[derive(Debug, Clone, Default)]
pub struct HotkeyStatus {
    pub backend: &'static str,
    /// Human readable key that is currently active, if any.
    pub active: Option<String>,
    /// Another action that already uses the requested key.
    pub conflict: Option<String>,
    pub error: Option<String>,
    /// The backend can set keys itself (otherwise the system dialog has to be used).
    pub can_set: bool,
}

#[derive(Debug)]
pub enum HotkeyCmd {
    /// Use this trigger. `force` takes the key away from other actions using it.
    Set { trigger: String, force: bool },
    /// Temporarily disable all global shortcuts (while recording a new one).
    Block(bool),
    /// Open the system's shortcut settings.
    OpenSystemSettings,
}

pub struct Hotkey {
    #[cfg(feature = "portal")]
    tx: tokio::sync::mpsc::UnboundedSender<HotkeyCmd>,
    pub status: Arc<Mutex<HotkeyStatus>>,
}

impl Hotkey {
    pub fn send(&self, cmd: HotkeyCmd) {
        #[cfg(feature = "portal")]
        let _ = self.tx.send(cmd);
        #[cfg(not(feature = "portal"))]
        let _ = cmd;
    }
}

// --- key name tables -------------------------------------------------------------------

/// (XDG key name, xkb keysym, Qt key code, display name)
const SPECIAL_KEYS: &[(&str, u32, i32, &str)] = &[
    ("space", 0x20, 0x20, "Space"),
    ("Return", 0xff0d, 0x0100_0004, "Enter"),
    ("Tab", 0xff09, 0x0100_0001, "Tab"),
    ("BackSpace", 0xff08, 0x0100_0003, "Backspace"),
    ("Delete", 0xffff, 0x0100_0007, "Del"),
    ("Insert", 0xff63, 0x0100_0006, "Ins"),
    ("Home", 0xff50, 0x0100_0010, "Home"),
    ("End", 0xff57, 0x0100_0011, "End"),
    ("Left", 0xff51, 0x0100_0012, "Left"),
    ("Up", 0xff52, 0x0100_0013, "Up"),
    ("Right", 0xff53, 0x0100_0014, "Right"),
    ("Down", 0xff54, 0x0100_0015, "Down"),
    ("Page_Up", 0xff55, 0x0100_0016, "PgUp"),
    ("Page_Down", 0xff56, 0x0100_0017, "PgDown"),
    ("Print", 0xff61, 0x0100_0009, "Print"),
    ("Pause", 0xff13, 0x0100_0008, "Pause"),
    ("Menu", 0xff67, 0x0100_0055, "Menu"),
    ("period", 0x2e, 0x2e, "."),
    ("comma", 0x2c, 0x2c, ","),
    ("minus", 0x2d, 0x2d, "-"),
    ("plus", 0x2b, 0x2b, "+"),
    ("slash", 0x2f, 0x2f, "/"),
    ("backslash", 0x5c, 0x5c, "\\"),
    ("semicolon", 0x3b, 0x3b, ";"),
    ("apostrophe", 0x27, 0x27, "'"),
    ("grave", 0x60, 0x60, "`"),
    ("equal", 0x3d, 0x3d, "="),
    ("bracketleft", 0x5b, 0x5b, "["),
    ("bracketright", 0x5d, 0x5d, "]"),
    ("numbersign", 0x23, 0x23, "#"),
];

const QT_SHIFT: i32 = 0x0200_0000;
const QT_CTRL: i32 = 0x0400_0000;
const QT_ALT: i32 = 0x0800_0000;
const QT_META: i32 = 0x1000_0000;

/// Builds a trigger from a key press while recording. Returns `None` for modifier keys
/// (recording continues until a real key is pressed).
pub fn trigger_from_keysym(sym: u32, ctrl: bool, alt: bool, shift: bool, logo: bool) -> Option<String> {
    let key = match sym {
        0x61..=0x7a | 0x30..=0x39 => char::from_u32(sym)?.to_string(),
        0x41..=0x5a => char::from_u32(sym + 0x20)?.to_string(),
        0xffbe..=0xffe0 => format!("F{}", sym - 0xffbe + 1),
        _ => SPECIAL_KEYS.iter().find(|k| k.1 == sym)?.0.to_string(),
    };
    let mut parts = Vec::new();
    if ctrl {
        parts.push("CTRL".to_string());
    }
    if alt {
        parts.push("ALT".to_string());
    }
    if shift {
        parts.push("SHIFT".to_string());
    }
    if logo {
        parts.push("LOGO".to_string());
    }
    parts.push(key);
    Some(parts.join("+"))
}

fn split_trigger(trigger: &str) -> (Vec<String>, String) {
    let mut parts: Vec<String> = trigger.split('+').map(|s| s.trim().to_string()).collect();
    // "CTRL++" -> key "+"
    if trigger.ends_with("++") {
        parts.retain(|p| !p.is_empty());
        parts.push("plus".into());
    }
    let key = parts.pop().unwrap_or_default();
    (parts.into_iter().map(|m| m.to_ascii_uppercase()).collect(), key)
}

/// `LOGO+space` -> `Meta+Space`
pub fn display(trigger: &str) -> String {
    if trigger.trim().is_empty() {
        return "–".into();
    }
    let (mods, key) = split_trigger(trigger);
    let mut out: Vec<String> = mods
        .iter()
        .map(|m| match m.as_str() {
            "LOGO" | "SUPER" | "META" => "Meta".into(),
            "CTRL" | "CONTROL" => "Ctrl".into(),
            "ALT" => "Alt".into(),
            "SHIFT" => "Shift".into(),
            o => o.to_string(),
        })
        .collect();
    let key_disp = SPECIAL_KEYS
        .iter()
        .find(|k| k.0.eq_ignore_ascii_case(&key))
        .map(|k| k.3.to_string())
        .unwrap_or_else(|| if key.len() == 1 { key.to_uppercase() } else { key.clone() });
    out.push(key_disp);
    out.join("+")
}

#[cfg_attr(not(feature = "portal"), allow(dead_code))]
fn to_qt(trigger: &str) -> Option<i32> {
    let (mods, key) = split_trigger(trigger);
    let mut code = if key.len() == 1 {
        let c = key.chars().next()?.to_ascii_uppercase();
        if !c.is_ascii_graphic() {
            return None;
        }
        c as i32
    } else if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<i32>().ok()) {
        0x0100_0030 + n - 1
    } else {
        SPECIAL_KEYS.iter().find(|k| k.0.eq_ignore_ascii_case(&key))?.2
    };
    for m in mods {
        code |= match m.as_str() {
            "LOGO" | "SUPER" | "META" => QT_META,
            "CTRL" | "CONTROL" => QT_CTRL,
            "ALT" => QT_ALT,
            "SHIFT" => QT_SHIFT,
            _ => return None,
        };
    }
    Some(code)
}

#[cfg_attr(not(feature = "portal"), allow(dead_code))]
fn qt_display(code: i32) -> String {
    let mut parts = Vec::new();
    if code & QT_META != 0 {
        parts.push("Meta".to_string());
    }
    if code & QT_CTRL != 0 {
        parts.push("Ctrl".to_string());
    }
    if code & QT_ALT != 0 {
        parts.push("Alt".to_string());
    }
    if code & QT_SHIFT != 0 {
        parts.push("Shift".to_string());
    }
    let key = code & 0x01ff_ffff;
    let name = if (0x0100_0030..0x0100_0053).contains(&key) {
        format!("F{}", key - 0x0100_0030 + 1)
    } else if let Some(k) = SPECIAL_KEYS.iter().find(|k| k.2 == key) {
        k.3.to_string()
    } else {
        char::from_u32(key as u32).map(|c| c.to_string()).unwrap_or_else(|| format!("{key:#x}"))
    };
    parts.push(name);
    parts.join("+")
}

// --- runtime ------------------------------------------------------------------------------

#[cfg(feature = "portal")]
pub fn spawn(
    trigger: String,
    enabled: bool,
    on_activate: impl Fn() + Send + 'static,
    on_status: impl Fn() + Send + 'static,
) -> Hotkey {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let status = Arc::new(Mutex::new(HotkeyStatus { backend: "…", ..Default::default() }));
    let st = status.clone();
    std::thread::Builder::new()
        .name("rlauncher-hotkey".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => return log::warn!("hotkey: cannot start runtime: {e}"),
            };
            let update = |f: &dyn Fn(&mut HotkeyStatus)| {
                if let Ok(mut s) = st.lock() {
                    f(&mut s);
                }
                on_status();
            };
            rt.block_on(async {
                let mut rx = rx;
                let result = match zbus::Connection::session().await {
                    Ok(conn) if kde::available(&conn).await => {
                        kde::run(&conn, &trigger, enabled, &mut rx, &on_activate, &update).await
                    }
                    Ok(_) => portal::run(&trigger, enabled, &mut rx, &on_activate, &update).await,
                    Err(e) => Err(e.into()),
                };
                if let Err(e) = result {
                    log::warn!("global hotkey unavailable: {e:#}. Bind `rlauncher-toggle` in your compositor instead.");
                    update(&|s| s.error = Some(format!("{e:#}")));
                    // keep answering commands so the settings dialog stays responsive
                    while rx.recv().await.is_some() {}
                }
            });
        })
        .expect("spawn hotkey thread");
    Hotkey { tx, status }
}

#[cfg(not(feature = "portal"))]
pub fn spawn(
    _trigger: String,
    _enabled: bool,
    _on_activate: impl Fn() + Send + 'static,
    _on_status: impl Fn() + Send + 'static,
) -> Hotkey {
    let status = HotkeyStatus {
        backend: "none",
        error: Some("built without hotkey support, bind `rlauncher-toggle` instead".into()),
        ..Default::default()
    };
    Hotkey { status: Arc::new(Mutex::new(status)) }
}

#[cfg(feature = "portal")]
type Update<'a> = dyn Fn(&dyn Fn(&mut HotkeyStatus)) + 'a;

#[cfg(feature = "portal")]
mod kde {
    use super::*;
    use futures_util::StreamExt;
    use zbus::zvariant::OwnedObjectPath;

    const COMPONENT: &str = "rlauncher";
    const ACTION: &str = "toggle";
    /// kglobalaccel flags
    const SET_PRESENT: u32 = 2;
    const NO_AUTOLOADING: u32 = 4;

    type ShortcutInfo = (String, String, String, String, String, String, Vec<i32>, Vec<i32>);

    fn action_id() -> Vec<&'static str> {
        vec![COMPONENT, ACTION, "rlauncher", "Show/hide rlauncher"]
    }

    pub async fn available(conn: &zbus::Connection) -> bool {
        let Ok(dbus) = zbus::fdo::DBusProxy::new(conn).await else { return false };
        match zbus::names::BusName::try_from("org.kde.kglobalaccel") {
            Ok(name) => dbus.name_has_owner(name).await.unwrap_or(false),
            Err(_) => false,
        }
    }

    async fn proxy<'a>(conn: &zbus::Connection) -> zbus::Result<zbus::Proxy<'a>> {
        zbus::Proxy::new(conn, "org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel").await
    }

    async fn current(kga: &zbus::Proxy<'_>) -> Option<String> {
        let keys: Vec<i32> = kga.call("shortcut", &(action_id(),)).await.ok()?;
        keys.into_iter().find(|k| *k != 0).map(qt_display)
    }

    pub async fn run(
        conn: &zbus::Connection,
        trigger: &str,
        enabled: bool,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<HotkeyCmd>,
        on_activate: &dyn Fn(),
        update: &Update<'_>,
    ) -> anyhow::Result<()> {
        let kga = proxy(conn).await?;
        kga.call_method("doRegister", &(action_id(),)).await?;
        update(&|s| {
            s.backend = "KDE (kglobalaccel)";
            s.can_set = true;
        });

        // Respect a key that is already saved (from an earlier run or the system settings);
        // otherwise use ours. Either way the action must be marked as present: kglobalaccel
        // only delivers key presses to components with present actions.
        let saved: Vec<i32> = kga.call("shortcut", &(action_id(),)).await.unwrap_or_default();
        let saved: Vec<i32> = saved.into_iter().filter(|k| *k != 0).collect();
        if !enabled {
            update(&|s| s.active = None);
        } else if saved.is_empty() {
            apply(&kga, trigger, false, false, update).await;
        } else {
            let keys: Vec<i32> = kga.call("setShortcut", &(action_id(), saved, SET_PRESENT)).await?;
            let active = keys.into_iter().find(|k| *k != 0).map(qt_display);
            update(&|s| s.active = active.clone());
        }

        let path: OwnedObjectPath = kga.call("getComponent", &(COMPONENT,)).await?;
        let component =
            zbus::Proxy::new(conn, "org.kde.kglobalaccel", path, "org.kde.kglobalaccel.Component").await?;
        let mut pressed = component.receive_signal("globalShortcutPressed").await?;
        log::info!("global hotkey registered with kglobalaccel");

        loop {
            tokio::select! {
                msg = pressed.next() => {
                    let Some(msg) = msg else { break };
                    if let Ok((_, action, _)) = msg.body().deserialize::<(String, String, i64)>()
                        && action == ACTION
                    {
                        on_activate();
                    }
                }
                cmd = rx.recv() => match cmd {
                    None => break,
                    Some(HotkeyCmd::Set { trigger, force }) => apply(&kga, &trigger, true, force, update).await,
                    Some(HotkeyCmd::Block(block)) => {
                        let _ = kga.call_method("blockGlobalShortcuts", &(block,)).await;
                    }
                    Some(HotkeyCmd::OpenSystemSettings) => {
                        let _ = std::process::Command::new("systemsettings").arg("kcm_keys").spawn();
                    }
                }
            }
        }
        Ok(())
    }

    async fn apply(kga: &zbus::Proxy<'_>, trigger: &str, explicit: bool, force: bool, update: &Update<'_>) {
        let flags = SET_PRESENT | if explicit { NO_AUTOLOADING } else { 0 };
        update(&|s| {
            s.conflict = None;
            s.error = None;
        });
        if trigger.trim().is_empty() {
            let _: zbus::Result<Vec<i32>> = kga.call("setShortcut", &(action_id(), Vec::<i32>::new(), flags)).await;
            update(&|s| s.active = None);
            return;
        }
        let Some(code) = to_qt(trigger) else {
            update(&|s| s.error = Some(format!("Unbekannte Taste: {trigger}")));
            return;
        };

        let others: Vec<ShortcutInfo> = kga.call("getGlobalShortcutsByKey", &(code,)).await.unwrap_or_default();
        let others: Vec<ShortcutInfo> = others.into_iter().filter(|o| o.2 != COMPONENT).collect();
        if !others.is_empty() {
            if force {
                for (action, action_name, comp, comp_name, _, _, keys, _) in &others {
                    let remaining: Vec<i32> = keys.iter().copied().filter(|k| *k != code).collect();
                    let id = vec![comp.as_str(), action.as_str(), comp_name.as_str(), action_name.as_str()];
                    if let Err(e) = kga.call_method("setForeignShortcut", &(id, remaining)).await {
                        log::warn!("cannot remove {comp}/{action}: {e}");
                    }
                }
            } else {
                let names: Vec<String> = others
                    .iter()
                    .map(|o| if o.1.is_empty() || o.1 == o.3 { o.3.clone() } else { format!("{} – {}", o.3, o.1) })
                    .collect();
                let active = current(kga).await;
                update(&|s| {
                    s.conflict = Some(names.join(", "));
                    s.active = active.clone();
                });
                return;
            }
        }

        match kga.call::<_, _, Vec<i32>>("setShortcut", &(action_id(), vec![code], flags)).await {
            Ok(keys) => {
                let active = keys.into_iter().find(|k| *k != 0).map(qt_display);
                let failed = active.is_none();
                update(&|s| {
                    s.active = active.clone();
                    if failed {
                        s.error = Some("KDE hat das Tastenkürzel abgelehnt".into());
                    }
                });
            }
            Err(e) => update(&|s| s.error = Some(e.to_string())),
        }
    }
}

#[cfg(feature = "portal")]
mod portal {
    use super::*;
    use crate::config::APP_ID;
    use ashpd::desktop::Session;
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;

    async fn bind(
        portal: &GlobalShortcuts,
        trigger: &str,
        update: &Update<'_>,
    ) -> anyhow::Result<Session<GlobalShortcuts>> {
        let session = portal.create_session(Default::default()).await?;
        let shortcut = NewShortcut::new("toggle", "Show/hide rlauncher")
            .preferred_trigger((!trigger.is_empty()).then_some(trigger));
        let bound = portal.bind_shortcuts(&session, &[shortcut], None, Default::default()).await?.response()?;
        let active = bound
            .shortcuts()
            .iter()
            .find(|s| s.id() == "toggle")
            .map(|s| s.trigger_description().to_string())
            .filter(|s| !s.is_empty());
        update(&|s| {
            s.active = active.clone();
            s.error = None;
        });
        Ok(session)
    }

    pub async fn run(
        trigger: &str,
        enabled: bool,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<HotkeyCmd>,
        on_activate: &dyn Fn(),
        update: &Update<'_>,
    ) -> anyhow::Result<()> {
        // Non-sandboxed apps have to tell the portal who they are (needs the installed
        // io.github.rlauncher.desktop file).
        if let Ok(id) = ashpd::AppID::try_from(APP_ID)
            && let Err(e) = ashpd::register_host_app(id).await
        {
            log::debug!("portal: host app registration failed: {e}");
        }
        let portal = GlobalShortcuts::new().await?;
        update(&|s| {
            s.backend = "XDG Desktop Portal";
            s.can_set = false;
        });
        let mut session = if enabled { Some(bind(&portal, trigger, update).await?) } else { None };
        let mut activated = portal.receive_activated().await?;
        loop {
            tokio::select! {
                ev = activated.next() => {
                    let Some(ev) = ev else { break };
                    if session.is_some() && ev.shortcut_id() == "toggle" {
                        on_activate();
                    }
                }
                cmd = rx.recv() => match cmd {
                    None => break,
                    Some(HotkeyCmd::Set { trigger, .. }) => {
                        if let Some(old) = session.take() {
                            let _ = old.close().await;
                        }
                        match bind(&portal, &trigger, update).await {
                            Ok(s) => session = Some(s),
                            Err(e) => update(&|s| s.error = Some(e.to_string())),
                        }
                    }
                    Some(HotkeyCmd::Block(_)) => {}
                    Some(HotkeyCmd::OpenSystemSettings) => {
                        if let Some(s) = &session {
                            let _ = portal.configure_shortcuts(s, None, Default::default()).await;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(to_qt("LOGO+space"), Some(QT_META | 0x20));
        assert_eq!(to_qt("CTRL+ALT+F12"), Some(QT_CTRL | QT_ALT | 0x0100_003b));
        assert_eq!(display("LOGO+space"), "Meta+Space");
        assert_eq!(qt_display(QT_META | 0x20), "Meta+Space");
        assert_eq!(trigger_from_keysym(0x41, true, false, true, false).as_deref(), Some("CTRL+SHIFT+a"));
        assert_eq!(trigger_from_keysym(0xffe1, false, false, true, false), None); // Shift_L
    }
}

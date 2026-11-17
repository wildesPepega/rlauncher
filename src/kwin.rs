//! KDE integration for the Run dialog window.
//!
//! Wayland clients can't position their windows, so on KWin a small KWin script places the
//! Run dialog where Windows puts it (bottom left, above the panel) or centered. A desktop
//! file for the dialog's app id gives it the "Run" icon in the title bar.

use crate::config::{Config, RunPosition};
use crate::win7::RUN_APP_ID;

const SCRIPT_NAME: &str = "rlauncher-run-placement";

/// The KWin script for the current settings.
pub fn placement_script(config: &Config) -> String {
    let (x, y) = match config.run_position {
        RunPosition::BottomLeft => ("area.x + 8".to_string(), "area.y + area.height - g.height - 8".to_string()),
        RunPosition::Center => (
            "area.x + (area.width - g.width) / 2".to_string(),
            format!("area.y + area.height * {}", config.top_offset.clamp(0.0, 0.8)),
        ),
    };
    format!(
        r#"// Installed by rlauncher: places its Run dialog window.
function place(w) {{
    if (!w || w.resourceClass !== "{RUN_APP_ID}" || w.transient) return;
    const area = workspace.clientArea(KWin.MaximizeArea, w);
    const g = w.frameGeometry;
    if (w.caption !== "Run") {{
        // the settings window: centered
        w.frameGeometry = {{ x: Math.round(area.x + (area.width - g.width) / 2), y: Math.round(area.y + (area.height - g.height) / 2), width: g.width, height: g.height }};
        return;
    }}
    w.frameGeometry = {{ x: Math.round({x}), y: Math.round({y}), width: g.width, height: g.height }};
}}
workspace.windowAdded.connect(place);
"#
    )
}

fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("KDE")))
}

/// Writes the desktop file for the dialog's app id (title bar icon) if it is missing.
fn ensure_desktop_file() {
    let Some(dir) = dirs::data_dir().map(|d| d.join("applications")) else { return };
    let file = dir.join(format!("{RUN_APP_ID}.desktop"));
    if file.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let content = "[Desktop Entry]\nType=Application\nName=Run\nComment=rlauncher's Run dialog\nExec=rlauncher-toggle\nIcon=system-run\nTerminal=false\nNoDisplay=true\n";
    if let Err(e) = std::fs::write(&file, content) {
        log::warn!("cannot write {}: {e}", file.display());
    }
}

/// Prepares the Run dialog window: desktop file and (on KDE) the placement script. The
/// script is loaded in the background.
pub fn install_run_window(script: &str) {
    ensure_desktop_file();
    if !is_kde() {
        return;
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("{SCRIPT_NAME}.js"));
    if let Err(e) = std::fs::write(&path, script) {
        log::warn!("cannot write {}: {e}", path.display());
        return;
    }
    load_script(path.to_string_lossy().into_owned());
}

#[cfg(feature = "portal")]
fn load_script(path: String) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        let result: zbus::Result<()> = rt.block_on(async {
            let conn = zbus::Connection::session().await?;
            let kwin = zbus::Proxy::new(&conn, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").await?;
            let loaded: bool = kwin.call("isScriptLoaded", &(SCRIPT_NAME,)).await?;
            if loaded {
                let _: bool = kwin.call("unloadScript", &(SCRIPT_NAME,)).await?;
            }
            let id: i32 = kwin.call("loadScript", &(path.as_str(), SCRIPT_NAME)).await?;
            let _: () = kwin.call("start", &()).await?;
            log::debug!("KWin placement script loaded (id {id})");
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("cannot load the KWin placement script: {e}");
        }
    });
}

#[cfg(not(feature = "portal"))]
fn load_script(_path: String) {
    log::warn!("built without D-Bus support: the Run dialog can't be placed by KWin");
}

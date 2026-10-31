//! Freedesktop icon theme lookup and background icon loading.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use egui::ColorImage;

/// Icon size (in physical pixels) that icons are rasterized at.
pub const ICON_PX: u32 = 96;

pub struct LoadedIcon {
    pub spec: String,
    pub image: Option<ColorImage>,
}

/// Handle to the loader thread.
pub struct IconLoader {
    tx: mpsc::Sender<String>,
}

impl IconLoader {
    /// Spawns the loader thread. `done` is called (from that thread) for every finished icon.
    pub fn spawn(done: impl Fn(LoadedIcon) + Send + 'static) -> IconLoader {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("rlauncher-icons".into())
            .spawn(move || {
                let index = IconIndex::build();
                for spec in rx {
                    let image = index.resolve(&spec).and_then(|p| load_image(&p, ICON_PX));
                    done(LoadedIcon { spec, image });
                }
            })
            .expect("spawn icon thread");
        IconLoader { tx }
    }

    pub fn request(&self, spec: &str) {
        let _ = self.tx.send(spec.to_string());
    }
}

struct IconIndex {
    /// icon name -> (theme rank, size penalty, path)
    icons: HashMap<String, (usize, u32, PathBuf)>,
}

fn icon_base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = dirs::data_dir() {
        dirs.push(d.join("icons"));
    }
    if let Some(h) = dirs::home_dir() {
        dirs.push(h.join(".icons"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|s| !s.is_empty()) {
        let p = Path::new(d).join("icons");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    for p in [
        dirs::data_dir().map(|d| d.join("flatpak/exports/share/icons")),
        Some(PathBuf::from("/var/lib/flatpak/exports/share/icons")),
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

/// The user's icon theme (KDE, GNOME or GTK settings), if it can be found.
fn user_icon_theme() -> Option<String> {
    let cfg = dirs::config_dir()?;
    if let Ok(text) = std::fs::read_to_string(cfg.join("kdeglobals")) {
        let mut in_icons = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_icons = line == "[Icons]";
            } else if in_icons && let Some(v) = line.strip_prefix("Theme=") {
                return Some(v.trim().to_string());
            }
        }
    }
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .stderr(std::process::Stdio::null())
        .output()
        && out.status.success()
    {
        let v = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    for file in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
        if let Ok(text) = std::fs::read_to_string(cfg.join(file)) {
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("gtk-icon-theme-name") {
                    return Some(v.trim_start_matches([' ', '=']).trim().to_string());
                }
            }
        }
    }
    None
}

fn theme_inherits(dirs: &[PathBuf]) -> Vec<String> {
    for d in dirs {
        if let Ok(text) = std::fs::read_to_string(d.join("index.theme")) {
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("Inherits=") {
                    return v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                }
            }
        }
    }
    Vec::new()
}

/// Lower is better: prefers scalable icons and bitmaps at least `ICON_PX` big.
fn size_penalty(path: &Path) -> u32 {
    let mut size = None;
    for comp in path.components().rev().skip(1) {
        let c = comp.as_os_str().to_string_lossy();
        if c == "scalable" || c == "symbolic" {
            return if c == "scalable" { 0 } else { 500 };
        }
        let num = c.split(['x', '@']).next().and_then(|n| n.parse::<u32>().ok());
        if let Some(n) = num {
            size = Some(n * if c.contains("@2") { 2 } else { 1 });
            break;
        }
    }
    match size {
        Some(s) if s >= ICON_PX => s - ICON_PX,
        Some(s) => (ICON_PX - s) * 4,
        None => 200,
    }
}

impl IconIndex {
    fn build() -> IconIndex {
        let started = std::time::Instant::now();
        let bases = icon_base_dirs();
        let mut themes: Vec<String> = Vec::new();
        let mut queue: Vec<String> = user_icon_theme().into_iter().collect();
        queue.extend(["breeze".to_string(), "Adwaita".to_string()]);
        // Breadth-first walk through the inheritance chain.
        while let Some(t) = (!queue.is_empty()).then(|| queue.remove(0)) {
            if themes.contains(&t) || t == "hicolor" || themes.len() > 12 {
                continue;
            }
            let dirs: Vec<PathBuf> = bases.iter().map(|b| b.join(&t)).filter(|p| p.is_dir()).collect();
            if dirs.is_empty() {
                continue;
            }
            queue.extend(theme_inherits(&dirs));
            themes.push(t);
        }
        themes.push("hicolor".into());

        let mut icons: HashMap<String, (usize, u32, PathBuf)> = HashMap::new();
        for (rank, theme) in themes.iter().enumerate() {
            for base in &bases {
                walk(&base.join(theme), 0, &mut |path| {
                    let Some(stem) = icon_stem(path) else { return };
                    let pen = size_penalty(path);
                    let entry = icons.entry(stem).or_insert((usize::MAX, u32::MAX, PathBuf::new()));
                    if (rank, pen) < (entry.0, entry.1) {
                        *entry = (rank, pen, path.to_path_buf());
                    }
                });
            }
        }
        let pixmap_rank = themes.len();
        for dir in ["/usr/share/pixmaps", "/usr/local/share/pixmaps"] {
            walk(Path::new(dir), 2, &mut |path| {
                if let Some(stem) = icon_stem(path) {
                    icons.entry(stem).or_insert((pixmap_rank, 0, path.to_path_buf()));
                }
            });
        }
        log::debug!("icon index: {} icons from {themes:?} in {:?}", icons.len(), started.elapsed());
        IconIndex { icons }
    }

    fn resolve(&self, spec: &str) -> Option<PathBuf> {
        let p = Path::new(spec);
        if p.is_absolute() {
            if p.is_file() {
                return Some(p.to_path_buf());
            }
            // Ulauncher often stores paths that point to removed files; try the name instead.
            return icon_stem(p).and_then(|s| self.icons.get(&s)).map(|e| e.2.clone());
        }
        let name = spec.trim_end_matches(".png").trim_end_matches(".svg");
        if let Some(e) = self.icons.get(name) {
            return Some(e.2.clone());
        }
        // Fallbacks like "org.gnome.Foo-symbolic" -> "org.gnome.Foo", "foo-bar" -> "foo"
        let mut n = name.trim_end_matches("-symbolic").to_string();
        while let Some(i) = n.rfind('-') {
            n.truncate(i);
            if let Some(e) = self.icons.get(&n) {
                return Some(e.2.clone());
            }
        }
        None
    }
}

fn icon_stem(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    if !matches!(ext, "png" | "svg") {
        return None;
    }
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

fn walk(dir: &Path, depth: usize, f: &mut dyn FnMut(&Path)) {
    if depth > 5 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() || (ft.is_symlink() && path.is_dir()) {
            walk(&path, depth + 1, f);
        } else {
            f(&path);
        }
    }
}

fn load_image(path: &Path, size: u32) -> Option<ColorImage> {
    let data = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|e| e == "svg") {
        return render_svg(&data, size);
    }
    let img = image::load_from_memory(&data).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let img = if w > size || h > size {
        let scale = size as f32 / w.max(h) as f32;
        image::imageops::resize(
            &img,
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let (w, h) = img.dimensions();
    Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()))
}

fn render_svg(data: &[u8], size: u32) -> Option<ColorImage> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let ts = tree.size();
    let scale = size as f32 / ts.width().max(ts.height());
    let w = ((ts.width() * scale).round() as u32).max(1);
    let h = ((ts.height() * scale).round() as u32).max(1);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
}

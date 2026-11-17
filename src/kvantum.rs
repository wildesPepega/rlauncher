//! A small renderer for the user's Kvantum theme (the Qt widget style), so the Run dialog's
//! controls look exactly like the ones in Qt applications on that desktop.
//!
//! Kvantum themes are an SVG with named elements plus a `.kvconfig`. A widget part such as a
//! push button is drawn from `<element>-<state>` (the interior) and the frame pieces
//! `<element>-<state>-top`, `-topleft`, … around it, with frame widths from the config.
//! Here the whole SVG is rasterized once into a texture sheet and the elements are drawn
//! from it as nine-slice pieces.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{Color32, Rect, TextureId, pos2};

/// Pixels per SVG unit of the texture sheet; elements are small, so render them sharper.
const SHEET_SCALE: f32 = 3.0;

#[derive(Debug, Clone, Copy, Default)]
pub struct Frame {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

pub struct Kvantum {
    pub name: String,
    dir: PathBuf,
    /// Rasterized SVG, taken out when uploaded as a texture.
    pub sheet: Option<egui::ColorImage>,
    sheet_size: egui::Vec2,
    /// Element id -> rect in sheet pixels.
    rects: HashMap<String, Rect>,
    sections: HashMap<String, HashMap<String, String>>,
}

impl std::fmt::Debug for Kvantum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Kvantum({}, {} elements)", self.name, self.rects.len())
    }
}

fn config_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs::config_dir().into_iter().collect();
    let xdg = std::env::var("XDG_CONFIG_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/etc/xdg".into());
    dirs.extend(xdg.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    dirs
}

fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == format!("[{section}]");
        } else if in_section
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// The Qt widget style of the Plasma session (`widgetStyle` in kdeglobals).
fn widget_style() -> Option<String> {
    config_dirs()
        .iter()
        .find_map(|d| std::fs::read_to_string(d.join("kdeglobals")).ok().and_then(|t| ini_value(&t, "KDE", "widgetStyle")))
}

/// The active Kvantum theme folder, if Qt apps use Kvantum.
fn theme_dir() -> Option<(String, PathBuf)> {
    if !widget_style()?.eq_ignore_ascii_case("kvantum") {
        return None;
    }
    let name = config_dirs().iter().find_map(|d| {
        [d.join("Kvantum/kvantum.kvconfig"), d.join("kvantum.kvconfig")]
            .iter()
            .find_map(|f| std::fs::read_to_string(f).ok().and_then(|t| ini_value(&t, "General", "theme")))
    })?;
    let mut candidates: Vec<PathBuf> = config_dirs().iter().map(|d| d.join("Kvantum").join(&name)).collect();
    candidates.push(PathBuf::from("/usr/share/Kvantum").join(&name));
    candidates.into_iter().find(|d| d.join(format!("{name}.kvconfig")).is_file()).map(|d| (name, d))
}

fn parse_kvconfig(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim_start_matches('%').to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            sections.entry(current.clone()).or_default().insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    sections
}

fn collect_rects(group: &resvg::usvg::Group, rects: &mut HashMap<String, Rect>) {
    for node in group.children() {
        if !node.id().is_empty() {
            let b = node.abs_stroke_bounding_box();
            if b.width() > 0.0 && b.height() > 0.0 {
                let r = Rect::from_min_max(
                    pos2(b.left() * SHEET_SCALE, b.top() * SHEET_SCALE),
                    pos2(b.right() * SHEET_SCALE, b.bottom() * SHEET_SCALE),
                );
                rects.insert(node.id().to_string(), r);
            }
        }
        if let resvg::usvg::Node::Group(g) = node {
            collect_rects(g, rects);
        }
    }
}

impl Kvantum {
    /// Loads the user's Kvantum theme. Slow (rasterizes the SVG), call it off the UI thread.
    pub fn load() -> Option<Kvantum> {
        let (name, dir) = theme_dir()?;
        Self::load_from(&name, &dir)
    }

    pub fn load_from(name: &str, dir: &Path) -> Option<Kvantum> {
        let started = std::time::Instant::now();
        let sections = parse_kvconfig(&std::fs::read_to_string(dir.join(format!("{name}.kvconfig"))).ok()?);
        let tree = Self::svg_tree(name, dir)?;
        let mut rects = HashMap::new();
        collect_rects(tree.root(), &mut rects);
        let sheet = Self::rasterize(&tree)?;
        let (w, h) = (sheet.size[0], sheet.size[1]);
        log::debug!("kvantum theme {name}: {} elements, {w}x{h} sheet in {:?}", rects.len(), started.elapsed());
        Some(Kvantum {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            sheet: Some(sheet),
            sheet_size: egui::vec2(w as f32, h as f32),
            rects,
            sections,
        })
    }

    fn svg_tree(name: &str, dir: &Path) -> Option<resvg::usvg::Tree> {
        let svg = std::fs::read(dir.join(format!("{name}.svg"))).ok()?;
        resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).ok()
    }

    fn rasterize(tree: &resvg::usvg::Tree) -> Option<egui::ColorImage> {
        let w = (tree.size().width() * SHEET_SCALE).ceil() as u32;
        let h = (tree.size().height() * SHEET_SCALE).ceil() as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
        resvg::render(tree, resvg::tiny_skia::Transform::from_scale(SHEET_SCALE, SHEET_SCALE), &mut pixmap.as_mut());
        Some(egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
    }

    /// Rasterizes the sheet again, for another egui context (the message box window).
    pub fn render_sheet(&self) -> Option<egui::ColorImage> {
        Self::rasterize(&Self::svg_tree(&self.name, &self.dir)?)
    }

    /// A config value, following `inherits=`.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let mut s = section;
        for _ in 0..8 {
            let sec = self.sections.get(s)?;
            if let Some(v) = sec.get(key) {
                return Some(v);
            }
            s = sec.get("inherits")?;
        }
        None
    }

    pub fn color(&self, key: &str) -> Option<Color32> {
        parse_color(self.get("GeneralColors", key)?)
    }

    pub fn section_color(&self, section: &str, key: &str) -> Option<Color32> {
        parse_color(self.get(section, key)?)
    }

    pub fn frame(&self, section: &str) -> Frame {
        let v = |k: &str| self.get(section, k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        Frame { top: v("frame.top"), bottom: v("frame.bottom"), left: v("frame.left"), right: v("frame.right") }
    }

    pub fn has(&self, id: &str) -> bool {
        self.rects.contains_key(id)
    }

    fn uv(&self, r: Rect) -> Rect {
        // half a texel inwards so neighbouring elements don't bleed in
        let r = r.shrink(0.5);
        Rect::from_min_max(
            pos2(r.min.x / self.sheet_size.x, r.min.y / self.sheet_size.y),
            pos2(r.max.x / self.sheet_size.x, r.max.y / self.sheet_size.y),
        )
    }

    /// Draws one element stretched into `rect`.
    pub fn paint_element(&self, painter: &egui::Painter, tex: TextureId, id: &str, rect: Rect, tint: Color32) -> bool {
        let Some(r) = self.rects.get(id) else { return false };
        if rect.width() > 0.0 && rect.height() > 0.0 {
            painter.image(tex, rect, self.uv(*r), tint);
        }
        true
    }

    /// Draws `<element>` (interior) with its frame pieces around it, nine-slice style.
    /// Returns false if the theme has no such element.
    pub fn paint_frame(&self, painter: &egui::Painter, tex: TextureId, element: &str, rect: Rect, f: Frame, tint: Color32) -> bool {
        let has_any = self.has(element) || self.has(&format!("{element}-top")) || self.has(&format!("{element}-center"));
        if !has_any {
            return false;
        }
        let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
        let (il, ir, it, ib) = (l + f.left, r - f.right, t + f.top, b - f.bottom);
        let piece = |suffix: &str, x0: f32, y0: f32, x1: f32, y1: f32| {
            let id = if suffix.is_empty() { element.to_string() } else { format!("{element}-{suffix}") };
            let target = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
            if !self.paint_element(painter, tex, &id, target, tint) && suffix.is_empty() {
                self.paint_element(painter, tex, &format!("{element}-center"), target, tint);
            }
        };
        piece("", il, it, ir, ib);
        piece("top", il, t, ir, it);
        piece("bottom", il, ib, ir, b);
        piece("left", l, it, il, ib);
        piece("right", ir, it, r, ib);
        piece("topleft", l, t, il, it);
        piece("topright", ir, t, r, it);
        piece("bottomleft", l, ib, il, b);
        piece("bottomright", ir, ib, r, b);
        true
    }
}

pub fn parse_color(v: &str) -> Option<Color32> {
    let hex = v.trim().strip_prefix('#')?;
    let n = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Color32::from_rgb(n(0)?, n(2)?, n(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(n(0)?, n(2)?, n(4)?, n(6)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kvconfig_inherits() {
        let k = Kvantum {
            name: "t".into(),
            dir: PathBuf::new(),
            sheet: None,
            sheet_size: egui::vec2(1.0, 1.0),
            rects: HashMap::new(),
            sections: parse_kvconfig(
                "[%General]\nx=1\n[PanelButtonCommand]\nframe.top=3\nframe.left=2\n[ComboBox]\ninherits=PanelButtonCommand\nframe.left=4\n[GeneralColors]\nwindow.color=#1e1e2e\n",
            ),
        };
        let f = k.frame("ComboBox");
        assert_eq!((f.top, f.left), (3.0, 4.0));
        assert_eq!(k.get("General", "x"), Some("1"));
        assert_eq!(k.color("window.color"), Some(Color32::from_rgb(0x1e, 0x1e, 0x2e)));
    }
}



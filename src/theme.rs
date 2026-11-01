//! Ulauncher theme support.
//!
//! Ulauncher themes are a `manifest.json` plus a GTK CSS file. We implement a small,
//! tolerant CSS reader that understands the subset Ulauncher themes actually use:
//! `@import`, `@define-color`, color functions (`alpha`, `shade`, `lighter`, `darker`,
//! `mix`, `rgb[a]`) and the handful of selectors that make up the launcher window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::Color32;
use serde::Deserialize;

use crate::config;

const BUILTIN_LIGHT: &str = include_str!("../themes/light/theme.css");
const BUILTIN_DARK: &str = include_str!("../themes/dark/theme.css");

#[derive(Debug, Clone)]
pub struct Theme {
    pub window_bg: Color32,
    pub window_border: Color32,
    pub border_width: f32,
    pub window_radius: f32,
    pub window_shadow: Color32,

    pub input_color: Color32,
    pub caret_color: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Color32,
    pub input_size: f32,
    /// Padding inside the input field: top, right, bottom, left.
    pub input_padding: [f32; 4],

    pub item_name: Color32,
    pub item_text: Color32,
    pub item_name_selected: Color32,
    pub item_text_selected: Color32,
    pub item_box_selected: Color32,
    pub item_radius: f32,
    pub item_shortcut: Color32,
    pub item_shortcut_selected: Color32,
    pub name_size: f32,
    pub descr_size: f32,

    pub hl_selected: Color32,
    pub hl_not_selected: Color32,
}

/// Base font size in logical pixels that percentages / rem / em are relative to.
const BASE_FONT: f32 = 15.0;

/// The GTK default font size in pixels. Ulauncher's input font (`font-size: 170%`) is
/// relative to it, so the input field matches Ulauncher exactly.
fn gtk_font_px() -> f32 {
    static SIZE: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        let from_name = |name: &str| -> Option<f32> {
            let pt: f32 = name.trim().trim_matches(['\'', '"']).rsplit([' ', ',']).next()?.parse().ok()?;
            (4.0..=40.0).contains(&pt).then_some(pt * 96.0 / 72.0)
        };
        let cfg = dirs::config_dir().unwrap_or_default();
        for file in ["gtk-3.0/settings.ini", "gtk-4.0/settings.ini"] {
            if let Ok(text) = std::fs::read_to_string(cfg.join(file)) {
                for line in text.lines() {
                    if let Some(v) = line.trim().strip_prefix("gtk-font-name")
                        && let Some(px) = from_name(v.trim_start_matches([' ', '=']))
                    {
                        return px;
                    }
                }
            }
        }
        if let Ok(out) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "font-name"])
            .stderr(std::process::Stdio::null())
            .output()
            && out.status.success()
            && let Some(px) = from_name(&String::from_utf8_lossy(&out.stdout))
        {
            return px;
        }
        // GTK's default: 11pt
        11.0 * 96.0 / 72.0
    })
}

/// CSS box shorthand (`5px`, `5px 7px`, `5px 0 5px 7px`, …) as [top, right, bottom, left].
fn parse_box(v: &str) -> Option<[f32; 4]> {
    let n: Vec<f32> = v.split_whitespace().map(|p| parse_length(p, BASE_FONT)).collect::<Option<_>>()?;
    Some(match n.as_slice() {
        [a] => [*a; 4],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return None,
    })
}

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    extend_theme: Option<String>,
    #[serde(default)]
    css_file: Option<String>,
    #[serde(rename = "css_file_gtk_3.20+", default)]
    css_file_gtk320: Option<String>,
    #[serde(default)]
    matched_text_hl_colors: Option<HlColors>,
}

#[derive(Debug, Deserialize, Clone)]
struct HlColors {
    when_selected: Option<String>,
    when_not_selected: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ThemeInfo {
    pub name: String,
    pub display_name: String,
    pub dir: Option<PathBuf>,
}

/// Additional theme folders from the config (`theme_dirs`).
static EXTRA_DIRS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

pub fn set_extra_dirs(dirs: &[String]) {
    let expanded = dirs
        .iter()
        .filter(|d| !d.trim().is_empty())
        .map(|d| match d.trim().strip_prefix("~/") {
            Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
            None => PathBuf::from(d.trim()),
        })
        .collect();
    if let Ok(mut e) = EXTRA_DIRS.lock() {
        *e = expanded;
    }
}

/// Folders searched for themes, in priority order.
pub fn theme_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![config::config_dir().join("themes")];
    if let Ok(extra) = EXTRA_DIRS.lock() {
        dirs.extend(extra.iter().cloned());
    }
    if let Some(c) = dirs::config_dir() {
        dirs.push(c.join("ulauncher/user-themes"));
    }
    if let Some(d) = dirs::data_dir() {
        dirs.push(d.join("rlauncher/themes"));
    }
    dirs.push(PathBuf::from("/usr/share/rlauncher/themes"));
    dirs.push(PathBuf::from("/usr/share/ulauncher/themes"));
    dirs
}

/// Theme folders inside a base dir. A base dir that is a theme itself counts too.
fn theme_candidates(base: &Path) -> Vec<PathBuf> {
    if base.join("manifest.json").is_file() {
        return vec![base.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(base) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs
}

fn find_theme_dir(name: &str) -> Option<PathBuf> {
    for base in theme_dirs() {
        for dir in theme_candidates(&base) {
            let Ok(manifest) = read_manifest(&dir) else { continue };
            if manifest.name == name {
                return Some(dir);
            }
        }
    }
    None
}

fn read_manifest(dir: &Path) -> anyhow::Result<Manifest> {
    let data = std::fs::read_to_string(dir.join("manifest.json"))?;
    Ok(serde_json::from_str(&data)?)
}

/// All themes that can be selected, including the builtin ones.
pub fn list_themes() -> Vec<ThemeInfo> {
    let mut out: Vec<ThemeInfo> = Vec::new();
    for base in theme_dirs() {
        for dir in theme_candidates(&base) {
            let Ok(m) = read_manifest(&dir) else { continue };
            if out.iter().any(|t| t.name == m.name) {
                continue;
            }
            out.push(ThemeInfo {
                display_name: m.display_name.clone().unwrap_or_else(|| m.name.clone()),
                name: m.name,
                dir: Some(dir),
            });
        }
    }
    for (name, display) in [("light", "Elementary Light (builtin)"), ("dark", "Elementary Dark (builtin)")] {
        if !out.iter().any(|t| t.name == name) {
            out.push(ThemeInfo { name: name.into(), display_name: display.into(), dir: None });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Collected CSS state: color definitions and declarations per selector.
#[derive(Default)]
struct Css {
    defines: HashMap<String, String>,
    /// selector -> property -> value (later declarations override earlier ones)
    rules: HashMap<String, HashMap<String, String>>,
}

impl Css {
    fn prop(&self, selector: &str, prop: &str) -> Option<&str> {
        self.rules.get(selector)?.get(prop).map(String::as_str)
    }

    fn color(&self, selector: &str, prop: &str) -> Option<Color32> {
        parse_color(self.prop(selector, prop)?, &self.defines, 0)
    }

    fn define(&self, name: &str) -> Option<Color32> {
        parse_color(self.defines.get(name)?, &self.defines, 0)
    }

    /// Parses a stylesheet, following `@import`s relative to `base_dir`.
    fn feed(&mut self, src: &str, base_dir: Option<&Path>, depth: usize) {
        if depth > 8 {
            return;
        }
        let src = strip_comments(src);
        let mut rest = src.as_str();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            if rest.starts_with('@') {
                let end = rest.find(';').unwrap_or(rest.len());
                let stmt = &rest[..end];
                rest = rest.get(end + 1..).unwrap_or("");
                self.at_rule(stmt, base_dir, depth);
                continue;
            }
            let Some(open) = rest.find('{') else { break };
            let selectors = &rest[..open];
            let Some(close) = rest[open..].find('}') else { break };
            let body = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            let decls = parse_declarations(body);
            for sel in selectors.split(',') {
                let sel = normalize_selector(sel);
                if sel.is_empty() {
                    continue;
                }
                let entry = self.rules.entry(sel).or_default();
                for (k, v) in &decls {
                    entry.insert(k.clone(), v.clone());
                }
            }
        }
    }

    fn at_rule(&mut self, stmt: &str, base_dir: Option<&Path>, depth: usize) {
        if let Some(def) = stmt.strip_prefix("@define-color") {
            let def = def.trim();
            if let Some((name, value)) = def.split_once(char::is_whitespace) {
                self.defines.insert(name.trim().to_string(), value.trim().to_string());
            }
        } else if let Some(imp) = stmt.strip_prefix("@import") {
            let imp = imp.trim();
            let path = imp
                .trim_start_matches("url(")
                .trim_end_matches(')')
                .trim()
                .trim_matches(|c| c == '"' || c == '\'');
            let path = Path::new(path);
            let full = if path.is_absolute() {
                path.to_path_buf()
            } else if let Some(base) = base_dir {
                base.join(path)
            } else {
                return;
            };
            // Ulauncher's base themes are bundled with rlauncher, so fall back to them
            // when an import points into a missing Ulauncher installation.
            let builtin = match full.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
                Some("light") if full.starts_with("/usr/share/ulauncher") => Some(BUILTIN_LIGHT),
                Some("dark") if full.starts_with("/usr/share/ulauncher") => Some(BUILTIN_DARK),
                _ => None,
            };
            if let Ok(text) = std::fs::read_to_string(&full) {
                self.feed(&text, full.parent(), depth + 1);
            } else if let Some(text) = builtin {
                self.feed(text, None, depth + 1);
            }
        }
    }
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn normalize_selector(sel: &str) -> String {
    sel.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_declarations(body: &str) -> Vec<(String, String)> {
    // Values can contain parentheses with commas/semicolons are not expected inside them,
    // so a simple split is fine for GTK CSS.
    body.split(';')
        .filter_map(|d| {
            let (k, v) = d.split_once(':')?;
            let v = v.trim().trim_end_matches("!important").trim();
            Some((k.trim().to_ascii_lowercase(), v.to_string()))
        })
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect()
}

/// Splits function arguments on top-level commas.
fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

fn parse_color(value: &str, defines: &HashMap<String, String>, depth: usize) -> Option<Color32> {
    if depth > 16 {
        return None;
    }
    let v = value.trim();
    if let Some(name) = v.strip_prefix('@') {
        return parse_color(defines.get(name)?, defines, depth + 1);
    }
    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(open) = v.find('(') {
        let func = v[..open].trim().to_ascii_lowercase();
        let inner = v[open + 1..].strip_suffix(')')?;
        let args = split_args(inner);
        let num = |i: usize| -> Option<f32> { args.get(i)?.trim().parse::<f32>().ok() };
        let col = |i: usize| -> Option<Rgba> { parse_color(args.get(i)?, defines, depth + 1).map(Rgba::from) };
        let rgba = match func.as_str() {
            "rgb" | "rgba" => {
                let chan = |i: usize| -> Option<f32> {
                    let a = args.get(i)?.trim();
                    if let Some(p) = a.strip_suffix('%') {
                        Some(p.trim().parse::<f32>().ok()? / 100.0)
                    } else {
                        Some(a.parse::<f32>().ok()? / 255.0)
                    }
                };
                Rgba { r: chan(0)?, g: chan(1)?, b: chan(2)?, a: if args.len() > 3 { num(3)? } else { 1.0 } }
            }
            "alpha" => {
                let mut c = col(0)?;
                c.a *= num(1)?;
                c
            }
            "shade" => col(0)?.shade(num(1)?),
            "lighter" => col(0)?.shade(1.3),
            "darker" => col(0)?.shade(0.7),
            "mix" => {
                let (a, b, f) = (col(0)?, col(1)?, num(2)?);
                Rgba {
                    r: a.r + (b.r - a.r) * f,
                    g: a.g + (b.g - a.g) * f,
                    b: a.b + (b.b - a.b) * f,
                    a: a.a + (b.a - a.a) * f,
                }
            }
            _ => return None,
        };
        return Some(rgba.into());
    }
    named_color(&v.to_ascii_lowercase())
}

fn parse_hex(hex: &str) -> Option<Color32> {
    let h = |s: &str| u8::from_str_radix(s, 16).ok();
    let d = |c: char| -> Option<u8> { let v = c.to_digit(16)? as u8; Some(v * 17) };
    let chars: Vec<char> = hex.chars().collect();
    match hex.len() {
        3 => Some(Color32::from_rgb(d(chars[0])?, d(chars[1])?, d(chars[2])?)),
        4 => Some(Color32::from_rgba_unmultiplied(d(chars[0])?, d(chars[1])?, d(chars[2])?, d(chars[3])?)),
        6 => Some(Color32::from_rgb(h(&hex[0..2])?, h(&hex[2..4])?, h(&hex[4..6])?)),
        8 => Some(Color32::from_rgba_unmultiplied(h(&hex[0..2])?, h(&hex[2..4])?, h(&hex[4..6])?, h(&hex[6..8])?)),
        _ => None,
    }
}

fn named_color(name: &str) -> Option<Color32> {
    Some(match name {
        "transparent" => Color32::TRANSPARENT,
        "white" => Color32::WHITE,
        "black" => Color32::BLACK,
        "red" => Color32::from_rgb(255, 0, 0),
        "green" => Color32::from_rgb(0, 128, 0),
        "blue" => Color32::from_rgb(0, 0, 255),
        "yellow" => Color32::from_rgb(255, 255, 0),
        "orange" => Color32::from_rgb(255, 165, 0),
        "gray" | "grey" => Color32::from_rgb(128, 128, 128),
        "silver" => Color32::from_rgb(192, 192, 192),
        "purple" => Color32::from_rgb(128, 0, 128),
        _ => return None,
    })
}

/// Unpremultiplied float color used for color math.
#[derive(Clone, Copy, Debug)]
struct Rgba {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl From<Color32> for Rgba {
    fn from(c: Color32) -> Self {
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }
    }
}

impl From<Rgba> for Color32 {
    fn from(c: Rgba) -> Self {
        let f = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Color32::from_rgba_unmultiplied(f(c.r), f(c.g), f(c.b), f(c.a))
    }
}

impl Rgba {
    /// GTK's `shade()`: scales lightness and saturation in HLS space.
    fn shade(self, factor: f32) -> Rgba {
        let (h, l, s) = rgb_to_hls(self.r, self.g, self.b);
        let l = (l * factor).clamp(0.0, 1.0);
        let s = (s * factor).clamp(0.0, 1.0);
        let (r, g, b) = hls_to_rgb(h, l, s);
        Rgba { r, g, b, a: self.a }
    }
}

fn rgb_to_hls(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, l, 0.0);
    }
    let d = max - min;
    let s = if l <= 0.5 { d / (max + min) } else { d / (2.0 - max - min) };
    let mut h = if r == max {
        (g - b) / d
    } else if g == max {
        2.0 + (b - r) / d
    } else {
        4.0 + (r - g) / d
    };
    h *= 60.0;
    if h < 0.0 {
        h += 360.0;
    }
    (h, l, s)
}

fn hls_to_rgb(h: f32, l: f32, s: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    let m2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let m1 = 2.0 * l - m2;
    let f = |mut hue: f32| {
        while hue > 360.0 {
            hue -= 360.0;
        }
        while hue < 0.0 {
            hue += 360.0;
        }
        if hue < 60.0 {
            m1 + (m2 - m1) * hue / 60.0
        } else if hue < 180.0 {
            m2
        } else if hue < 240.0 {
            m1 + (m2 - m1) * (240.0 - hue) / 60.0
        } else {
            m1
        }
    };
    (f(h + 120.0), f(h), f(h - 120.0))
}

/// Parses a length like `14px`, `120%`, `1.2rem`, `1.1em`, `11pt` relative to `base`.
fn parse_length(v: &str, base: f32) -> Option<f32> {
    let v = v.trim();
    let (num, unit) = v
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .map(|i| (&v[..i], &v[i..]))
        .unwrap_or((v, ""));
    let n: f32 = num.parse().ok()?;
    Some(match unit.trim() {
        "px" | "" => n,
        "%" => base * n / 100.0,
        "rem" | "em" => base * n,
        "pt" => n * 4.0 / 3.0,
        _ => return None,
    })
}

/// Extracts the width and color out of a `border` shorthand.
fn parse_border(v: &str, defines: &HashMap<String, String>) -> (Option<f32>, Option<Color32>) {
    let mut width = None;
    let mut color = None;
    // Re-join tokens inside parentheses so `rgba(1, 2, 3)` stays intact.
    let mut tokens: Vec<String> = Vec::new();
    let mut depth = 0;
    for part in v.split_whitespace() {
        if depth > 0 {
            if let Some(last) = tokens.last_mut() {
                last.push(' ');
                last.push_str(part);
            }
        } else {
            tokens.push(part.to_string());
        }
        depth += part.matches('(').count() as i32 - part.matches(')').count() as i32;
    }
    for t in tokens {
        if t == "none" {
            width = Some(0.0);
        } else if let Some(w) = parse_length(&t, BASE_FONT).filter(|_| t.ends_with("px") || t == "0") {
            width = Some(w);
        } else if let Some(c) = parse_color(&t, defines, 0) {
            color = Some(c);
        }
    }
    (width, color)
}

impl Theme {
    /// Loads a theme by name. Falls back to the builtin light theme.
    pub fn load(name: &str) -> Theme {
        let mut css = Css::default();
        css.feed(BUILTIN_LIGHT, None, 0);
        let mut hl: Option<HlColors> = None;
        let mut chain = Vec::new();
        load_chain(name, &mut chain, 0);
        if chain.is_empty() {
            log::warn!("theme '{name}' not found, using builtin light theme");
        }
        // chain is ordered from the requested theme down to its most basic parent
        for (dir, manifest) in chain.iter().rev() {
            if dir.is_none() && manifest.name == "dark" {
                css.feed(BUILTIN_DARK, None, 0);
            }
            if let Some(dir) = dir {
                let file = manifest.css_file_gtk320.as_ref().or(manifest.css_file.as_ref());
                if let Some(file) = file {
                    let path = dir.join(file);
                    match std::fs::read_to_string(&path) {
                        Ok(text) => css.feed(&text, Some(dir), 0),
                        Err(e) => log::warn!("cannot read {}: {e}", path.display()),
                    }
                }
            }
            if manifest.matched_text_hl_colors.is_some() {
                hl = manifest.matched_text_hl_colors.clone();
            }
        }
        Theme::from_css(name, &css, hl)
    }

    fn from_css(_name: &str, css: &Css, hl: Option<HlColors>) -> Theme {
        const SEL_ITEM: &str = ".selected.item-box";
        let c = |sel: &str, prop: &str, fallback: Color32| css.color(sel, prop).unwrap_or(fallback);

        let window_bg = css
            .color(".app", "background-color")
            .or_else(|| css.color(".app", "background"))
            .unwrap_or(Color32::from_rgb(0xf2, 0xf2, 0xf2));
        let (mut border_width, mut window_border) = css
            .prop(".app", "border")
            .map(|b| parse_border(b, &css.defines))
            .unwrap_or((None, None));
        if let Some(w) = css.prop(".app", "border-width").and_then(|v| parse_length(v, BASE_FONT)) {
            border_width = Some(w);
        }
        if let Some(col) = css.color(".app", "border-color") {
            window_border = Some(col);
        }
        let window_radius = css
            .prop(".app", "border-radius")
            .and_then(|v| parse_length(v.split_whitespace().next().unwrap_or(v), BASE_FONT))
            .unwrap_or(4.0);

        let input_color = c(".input", "color", Color32::from_gray(0x55));
        let caret_color = css
            .color(".input", "caret-color")
            .or_else(|| css.define("caret_color"))
            .unwrap_or(input_color);
        let input_size = css
            .prop(".input", "font-size")
            .and_then(|v| parse_length(v, gtk_font_px()))
            .unwrap_or(gtk_font_px() * 1.7);
        let input_padding = css.prop(".input", "padding").and_then(parse_box).unwrap_or([5.0, 0.0, 5.0, 7.0]);
        let selection_bg = css
            .color(".input *:selected", "background-color")
            .or_else(|| css.color("*:selected:focus", "background-color"))
            .unwrap_or(Color32::from_gray(0x99));
        let selection_fg = css.color(".input *:selected", "color").unwrap_or(Color32::WHITE);

        let item_name = c(".item-name", "color", Color32::from_gray(0x33));
        let item_text = c(".item-text", "color", Color32::from_gray(0x55));
        let item_name_selected = css
            .color(&format!("{SEL_ITEM} .item-name"), "color")
            .unwrap_or(item_name);
        let item_text_selected = css
            .color(&format!("{SEL_ITEM} .item-text"), "color")
            .unwrap_or(item_text);
        let item_box_selected = css
            .color(SEL_ITEM, "background-color")
            .or_else(|| css.color(SEL_ITEM, "background"))
            .unwrap_or(Color32::from_gray(0xd6));
        let item_radius = css
            .prop(SEL_ITEM, "border-radius")
            .and_then(|v| parse_length(v.split_whitespace().next().unwrap_or(v), BASE_FONT))
            .unwrap_or(0.0);
        let item_shortcut = c(".item-shortcut", "color", Color32::from_gray(0x77));
        let item_shortcut_selected = css
            .color(&format!("{SEL_ITEM} .item-shortcut"), "color")
            .unwrap_or(item_shortcut);
        let name_size = css
            .prop(".item-name", "font-size")
            .and_then(|v| parse_length(v, BASE_FONT))
            .unwrap_or(BASE_FONT * 1.2);
        let descr_size = css
            .prop(".item-descr", "font-size")
            .and_then(|v| parse_length(v, BASE_FONT))
            .unwrap_or(BASE_FONT * 0.8);

        let window_shadow = css.define("window_shadow").unwrap_or(Color32::from_black_alpha(110));

        let hl_color = |v: Option<&String>| v.and_then(|s| parse_color(s, &css.defines, 0));
        let hl_selected = hl_color(hl.as_ref().and_then(|h| h.when_selected.as_ref()))
            .unwrap_or(Color32::from_rgb(0x2c, 0x74, 0xcc));
        let hl_not_selected = hl_color(hl.as_ref().and_then(|h| h.when_not_selected.as_ref()))
            .unwrap_or(hl_selected);

        Theme {
            window_bg,
            window_border: window_border.unwrap_or(Color32::from_gray(0xb1)),
            border_width: border_width.unwrap_or(1.0),
            window_radius,
            window_shadow,
            input_color,
            caret_color,
            selection_bg,
            selection_fg,
            input_size,
            input_padding,
            item_name,
            item_text,
            item_name_selected,
            item_text_selected,
            item_box_selected,
            item_radius,
            item_shortcut,
            item_shortcut_selected,
            name_size,
            descr_size,
            hl_selected,
            hl_not_selected,
        }
    }
}

/// Resolves the `extend_theme` chain, starting with the requested theme.
fn load_chain(name: &str, chain: &mut Vec<(Option<PathBuf>, Manifest)>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Some(dir) = find_theme_dir(name) else {
        // Builtin bases that are not installed on disk.
        if matches!(name, "light" | "dark") {
            let css_manifest = Manifest {
                name: name.to_string(),
                display_name: None,
                extend_theme: if name == "dark" { Some("light".into()) } else { None },
                css_file: None,
                css_file_gtk320: None,
                matched_text_hl_colors: Some(if name == "dark" {
                    HlColors { when_selected: Some("#fff".into()), when_not_selected: Some("#fff".into()) }
                } else {
                    HlColors { when_selected: Some("#2c74cc".into()), when_not_selected: Some("#2c74cc".into()) }
                }),
            };
            chain.push((None, css_manifest));
        }
        return;
    };
    let Ok(manifest) = read_manifest(&dir) else { return };
    let parent = manifest.extend_theme.clone();
    chain.push((Some(dir), manifest));
    if let Some(parent) = parent.filter(|p| !p.is_empty()) {
        load_chain(&parent, chain, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catppuccin_like_theme() {
        let mut css = Css::default();
        css.feed(BUILTIN_LIGHT, None, 0);
        css.feed(
            r#"
            /* comment */
            @define-color backgroundColor #1e1e2e;
            @define-color accentColor #a6e3a1;
            .app { background-color: @backgroundColor; border-color: @accentColor; border-radius: 14px; }
            .selected.item-box { background-color: alpha (@accentColor, 0.3); border-radius: 14px; }
            .item-name { font-size: 1.2rem; }
            "#,
            None,
            0,
        );
        let t = Theme::from_css("x", &css, None);
        assert_eq!(t.window_bg, Color32::from_rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(t.window_border, Color32::from_rgb(0xa6, 0xe3, 0xa1));
        assert_eq!(t.window_radius, 14.0);
        assert_eq!(t.item_radius, 14.0);
        assert_eq!(t.border_width, 1.0);
        assert_eq!(t.item_box_selected.a(), (0.3f32 * 255.0).round() as u8);
        assert!((t.name_size - 18.0).abs() < 0.01);
    }

    #[test]
    fn shade_functions() {
        let defs = HashMap::new();
        let c = parse_color("lighter(#555)", &defs, 0).unwrap();
        assert!(c.r() > 0x55);
        let c = parse_color("darker(#999)", &defs, 0).unwrap();
        assert!(c.r() < 0x99);
        let c = parse_color("rgba(0, 0, 0, 0.5)", &defs, 0).unwrap();
        assert_eq!(c.a(), 128);
    }
}

//! Launcher state and the egui user interface.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

use crate::apps::{App, LaunchSpec};
use crate::config::{Config, Shortcut, Stats};
use crate::hotkey::{HotkeyCmd, HotkeyStatus};
use crate::icons::{IconLoader, LoadedIcon};
use crate::search::{self, Action, ResultItem};
use crate::settings::{Settings, SettingsAction};
use crate::theme::Theme;

/// Requests from the UI to the windowing layer.
#[derive(Debug)]
pub enum UiEvent {
    Hide,
    Launch(LaunchSpec),
    Copy(String),
    Hotkey(HotkeyCmd),
    OpenSettings,
}

/// Navigation keys handled by the launcher itself (not by the text field).
#[derive(Debug, Clone, Copy)]
pub enum NavKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Enter { alt: bool },
    /// Alt+<jump key>
    Jump(usize),
    Escape,
    /// Tab: completes directories / shortcut keywords, otherwise moves down.
    Complete,
}

enum IconState {
    Pending,
    Image(egui::ColorImage),
    Texture(egui::TextureHandle),
    Missing,
}

pub struct Launcher {
    pub config: Config,
    pub theme: Theme,
    shortcuts: Vec<Shortcut>,
    apps: Vec<App>,
    stats: Stats,

    query: String,
    results: Vec<ResultItem>,
    selected: usize,
    scroll: usize,
    /// Move the text cursor to the end on the next frame (after SetQuery).
    cursor_to_end: bool,
    /// Only select items on hover after the pointer actually moved.
    last_pointer: Option<Pos2>,

    icons: HashMap<String, IconState>,
    loader: IconLoader,
    pub events: Vec<UiEvent>,

    settings: Option<Settings>,
    hotkey_status: Arc<Mutex<HotkeyStatus>>,
}

const PADDING: f32 = 12.0;
const GEAR_SIZE: f32 = 22.0;
const ICON_SIZE: f32 = 40.0;

impl Launcher {
    pub fn new(icon_done: impl Fn(LoadedIcon) + Send + 'static) -> Launcher {
        let config = Config::load();
        let theme = Theme::load(&config.theme);
        Launcher {
            theme,
            shortcuts: crate::config::load_shortcuts(),
            apps: Vec::new(),
            stats: Stats::load(),
            config,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            cursor_to_end: false,
            last_pointer: None,
            icons: HashMap::new(),
            loader: IconLoader::spawn(icon_done),
            events: Vec::new(),
            settings: None,
            hotkey_status: Arc::default(),
        }
    }

    pub fn set_hotkey_status(&mut self, status: Arc<Mutex<HotkeyStatus>>) {
        self.hotkey_status = status;
    }

    pub fn in_settings(&self) -> bool {
        self.settings.is_some()
    }

    pub fn is_recording(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| s.recording)
    }

    pub fn open_settings(&mut self, tab: Option<&str>) {
        let s = self.settings.get_or_insert_with(|| Settings::new(self.config.clone(), self.shortcuts.clone()));
        if let Some(tab) = tab {
            s.open_tab(tab);
        }
    }

    /// Saves and leaves the settings page.
    pub fn close_settings(&mut self, ctx: &egui::Context) {
        if let Some(mut s) = self.settings.take() {
            if s.recording {
                self.events.push(UiEvent::Hotkey(HotkeyCmd::Block(false)));
            }
            s.save();
            self.reload(ctx);
            if !self.query.is_empty() {
                self.refresh(false);
            }
        }
    }

    /// Escape inside the settings: cancels recording, otherwise goes back to the search.
    pub fn settings_escape(&mut self, ctx: &egui::Context) {
        if let Some(s) = self.settings.as_mut()
            && s.recording
        {
            s.recording = false;
            self.events.push(UiEvent::Hotkey(HotkeyCmd::Block(false)));
            return;
        }
        self.close_settings(ctx);
    }

    /// A key combination was pressed while recording the hotkey.
    pub fn recorded(&mut self, trigger: String) {
        if let Some(s) = self.settings.as_mut() {
            s.recorded(trigger.clone());
            self.events.push(UiEvent::Hotkey(HotkeyCmd::Block(false)));
            self.events.push(UiEvent::Hotkey(HotkeyCmd::Set { trigger, force: false }));
        }
    }

    /// The window is being hidden: persist the settings. Returns hotkey commands to send.
    pub fn on_hide(&mut self, ctx: &egui::Context) -> Vec<HotkeyCmd> {
        let mut cmds = Vec::new();
        if self.is_recording() {
            cmds.push(HotkeyCmd::Block(false));
        }
        if let Some(mut s) = self.settings.take() {
            s.save();
            self.reload(ctx);
        }
        cmds
    }

    /// Re-reads config, theme and shortcuts. Called every time the launcher is shown.
    pub fn reload(&mut self, ctx: &egui::Context) {
        let config = Config::load();
        let theme_changed = config.theme != self.config.theme || config.font != self.config.font;
        self.config = config;
        self.theme = Theme::load(&self.config.theme);
        self.shortcuts = crate::config::load_shortcuts();
        if theme_changed {
            install_fonts(ctx, &self.config.font);
        }
        self.apply_style(ctx);
    }

    pub fn reset(&mut self) {
        self.query.clear();
        self.results.clear();
        self.selected = 0;
        self.scroll = 0;
        self.last_pointer = None;
    }

    pub fn set_apps(&mut self, apps: Vec<App>) {
        for app in &apps {
            if let Some(icon) = &app.icon {
                self.request_icon(icon);
            }
        }
        self.apps = apps;
        if !self.query.is_empty() {
            self.refresh(false);
        }
    }

    pub fn icon_loaded(&mut self, icon: LoadedIcon) {
        let state = match icon.image {
            Some(img) => IconState::Image(img),
            None => IconState::Missing,
        };
        self.icons.insert(icon.spec, state);
    }

    fn request_icon(&mut self, spec: &str) {
        if !self.icons.contains_key(spec) {
            self.icons.insert(spec.to_string(), IconState::Pending);
            self.loader.request(spec);
        }
    }

    pub fn setup(&mut self, ctx: &egui::Context) {
        install_fonts(ctx, &self.config.font);
        self.apply_style(ctx);
    }

    fn apply_style(&self, ctx: &egui::Context) {
        let t = self.theme.clone();
        let [r, g, b, _] = t.window_bg.to_array();
        let dark = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) < 128.0;
        ctx.all_styles_mut(|style| {
            // Widgets (used by the settings page) are derived from the theme colors.
            style.visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
            let v = &mut style.visuals;
            let mix = |a: Color32, b: Color32, f: f32| a.lerp_to_gamma(b, f);
            v.selection.bg_fill = t.selection_bg;
            v.selection.stroke = Stroke::new(1.0, t.selection_fg);
            v.text_cursor.stroke = Stroke::new(2.0, t.caret_color);
            v.extreme_bg_color = mix(t.window_bg, t.item_name, 0.06);
            v.window_fill = t.window_bg;
            v.window_stroke = Stroke::new(1.0, t.window_border);
            v.panel_fill = Color32::TRANSPARENT;
            v.hyperlink_color = t.hl_selected;
            v.override_text_color = None;
            let w = &mut v.widgets;
            w.noninteractive.fg_stroke = Stroke::new(1.0, t.item_name);
            w.noninteractive.bg_stroke = Stroke::new(1.0, mix(t.window_bg, t.item_text, 0.2));
            w.noninteractive.bg_fill = t.window_bg;
            w.inactive.bg_fill = mix(t.window_bg, t.item_name, 0.1);
            w.inactive.weak_bg_fill = w.inactive.bg_fill;
            w.inactive.fg_stroke = Stroke::new(1.0, t.item_name);
            w.inactive.bg_stroke = Stroke::NONE;
            let hover = mix(t.window_bg, t.hl_selected, 0.25);
            w.hovered.bg_fill = hover;
            w.hovered.weak_bg_fill = hover;
            w.hovered.fg_stroke = Stroke::new(1.5, t.item_name_selected);
            w.hovered.bg_stroke = Stroke::new(1.0, t.hl_selected);
            let active = mix(t.window_bg, t.hl_selected, 0.4);
            w.active.bg_fill = active;
            w.active.weak_bg_fill = active;
            w.active.fg_stroke = Stroke::new(2.0, t.item_name_selected);
            w.active.bg_stroke = Stroke::new(1.0, t.hl_selected);
            w.open = w.hovered;
            for ws in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                ws.corner_radius = CornerRadius::same(6);
            }
            style.spacing.item_spacing = vec2(0.0, 0.0);
        });
    }

    fn refresh(&mut self, reset_selection: bool) {
        let ctx = search::Context {
            apps: &self.apps,
            shortcuts: &self.shortcuts,
            stats: &self.stats,
            config: &self.config,
        };
        self.results = search::search(&self.query, &ctx);
        if reset_selection || self.selected >= self.results.len() {
            self.selected = 0;
            self.scroll = 0;
        }
        self.ensure_visible();
    }

    fn visible_rows(&self) -> usize {
        self.config.max_results.max(1)
    }

    fn ensure_visible(&mut self) {
        let rows = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
        let max_scroll = self.results.len().saturating_sub(rows);
        self.scroll = self.scroll.min(max_scroll);
    }

    pub fn nav(&mut self, key: NavKey) {
        let n = self.results.len();
        match key {
            NavKey::Escape => self.events.push(UiEvent::Hide),
            NavKey::Up if n > 0 => self.selected = (self.selected + n - 1) % n,
            NavKey::Down if n > 0 => self.selected = (self.selected + 1) % n,
            NavKey::PageUp if n > 0 => self.selected = self.selected.saturating_sub(self.visible_rows()),
            NavKey::PageDown if n > 0 => self.selected = (self.selected + self.visible_rows()).min(n - 1),
            NavKey::Enter { alt } => self.activate(self.selected, alt),
            NavKey::Jump(i) => {
                if i < n {
                    self.activate(i, false);
                }
            }
            NavKey::Complete => {
                let completion = self.results.get(self.selected).and_then(|r| match &r.action {
                    Action::SetQuery(q) => Some(q.clone()),
                    _ => None,
                });
                match completion {
                    Some(q) => self.set_query(q),
                    None if n > 0 => self.selected = (self.selected + 1) % n,
                    None => {}
                }
            }
            _ => {}
        }
        self.ensure_visible();
    }

    pub fn set_query(&mut self, q: String) {
        self.query = q;
        self.cursor_to_end = true;
        self.refresh(true);
    }

    fn activate(&mut self, index: usize, alt: bool) {
        let Some(item) = self.results.get(index).cloned() else { return };
        let action = if alt { item.alt_action.clone().unwrap_or(item.action.clone()) } else { item.action.clone() };
        match action {
            Action::SetQuery(q) => self.set_query(q),
            Action::Launch(spec) => {
                if item.record {
                    self.stats.record(&self.query, &item.key);
                }
                self.events.push(UiEvent::Launch(spec));
            }
            Action::Copy(text) => {
                self.events.push(UiEvent::Copy(text));
                self.events.push(UiEvent::Hide);
            }
        }
    }

    /// Where the window sits on screen (logical coordinates).
    fn panel_origin(&self, screen: Rect) -> (Pos2, f32) {
        let width = self.config.width.min(screen.width() - 32.0).max(200.0);
        let x = screen.center().x - width / 2.0;
        let y = screen.top() + screen.height() * self.config.top_offset.clamp(0.0, 0.8);
        (pos2(x.round(), y.round()), width)
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if self.settings.is_some() {
            self.settings_page(ctx);
            return;
        }
        let screen = ctx.content_rect();
        let (origin, width) = self.panel_origin(screen);
        let t = self.theme.clone();

        let frame = self.window_frame();
        let area = egui::Area::new(egui::Id::unique("rlauncher-panel"))
            .fixed_pos(origin)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    let inner_width = width - 2.0 * PADDING - 2.0 * t.border_width;
                    ui.set_width(inner_width);
                    self.input_ui(ui, &t);
                    if !self.results.is_empty() {
                        ui.add_space(PADDING * 0.8);
                        self.results_ui(ui, &t, inner_width);
                    }
                });
            });
        let panel_rect = area.response.rect;

        // Clicking anywhere outside of the window hides it.
        let (pressed, pos) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
        if pressed
            && self.config.close_on_focus_out
            && let Some(p) = pos
            && !panel_rect.contains(p)
        {
            self.events.push(UiEvent::Hide);
        }
    }

    fn window_frame(&self) -> egui::Frame {
        let t = &self.theme;
        egui::Frame::new()
            .fill(t.window_bg)
            .stroke(Stroke::new(t.border_width, t.window_border))
            .corner_radius(CornerRadius::same(t.window_radius.min(60.0) as u8))
            .inner_margin(egui::Margin::same(PADDING as i8))
            .shadow(if self.config.shadow {
                egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: t.window_shadow.gamma_multiply(0.6) }
            } else {
                egui::Shadow::NONE
            })
    }

    fn settings_page(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let width = self.config.width.max(860.0).min(screen.width() - 32.0);
        let x = screen.center().x - width / 2.0;
        let y = screen.top() + screen.height() * self.config.top_offset.clamp(0.0, 0.8);
        let y = y.min(screen.bottom() - 420.0).max(screen.top() + 16.0);
        let height = (screen.bottom() - y - 48.0).clamp(300.0, 600.0);
        let t = self.theme.clone();
        let status = self.hotkey_status.lock().map(|s| s.clone()).unwrap_or_default();
        let frame = self.window_frame();
        let mut actions = Vec::new();
        egui::Area::new(egui::Id::unique("rlauncher-settings"))
            .fixed_pos(pos2(x.round(), y.round()))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    let inner = width - 2.0 * PADDING - 2.0 * t.border_width;
                    // A child with a fixed rect: wide content can't stretch the window.
                    let (rect, _) = ui.allocate_exact_size(vec2(inner, height), Sense::hover());
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(rect.intersect(ui.clip_rect()));
                    if let Some(s) = self.settings.as_mut() {
                        actions = s.ui(&mut child, &t, &status, height);
                    }
                });
            });

        for action in actions {
            match action {
                SettingsAction::Close => self.close_settings(ctx),
                SettingsAction::StartRecording => self.events.push(UiEvent::Hotkey(HotkeyCmd::Block(true))),
                SettingsAction::CancelRecording => self.events.push(UiEvent::Hotkey(HotkeyCmd::Block(false))),
                SettingsAction::ApplyHotkey { force } => {
                    if let Some(s) = &self.settings {
                        let trigger = if s.config.hotkey_enabled { s.config.hotkey.clone() } else { String::new() };
                        self.events.push(UiEvent::Hotkey(HotkeyCmd::Set { trigger, force }));
                    }
                }
                SettingsAction::OpenSystemShortcuts => {
                    self.events.push(UiEvent::Hotkey(HotkeyCmd::OpenSystemSettings));
                }
                SettingsAction::ThemeChanged => {
                    // live preview
                    if let Some(s) = &self.settings {
                        self.theme = Theme::load(&s.config.theme);
                        self.config.shadow = s.config.shadow;
                        self.apply_style(ctx);
                    }
                }
            }
        }
    }

    fn input_ui(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let id = egui::Id::unique("rlauncher-input");
        let font = FontId::proportional(t.input_size);
        let hint = egui::RichText::new(&self.config.placeholder)
            .font(font.clone())
            .color(t.input_color.gamma_multiply(0.45));
        let edit = egui::TextEdit::singleline(&mut self.query)
            .id(id)
            .font(font)
            .text_color(t.input_color)
            .hint_text(hint)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::symmetric(6, 9))
            .desired_width(ui.available_width() - GEAR_SIZE - 10.0);
        let row = ui.horizontal(|ui| {
            let output = edit.show(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if gear_button(ui, t).clicked() {
                    self.events.push(UiEvent::OpenSettings);
                }
            });
            output
        });
        let mut output = row.inner;
        if self.cursor_to_end {
            self.cursor_to_end = false;
            let end = egui::text::CCursor::new(self.query.chars().count());
            output.state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            output.state.store(ui.ctx(), id);
        }
        if output.response.changed() {
            self.refresh(true);
        }
        // The text field always owns the keyboard.
        output.response.request_focus();
    }

    fn results_ui(&mut self, ui: &mut egui::Ui, t: &Theme, width: f32) {
        let rows = self.visible_rows();
        let row_h = (ICON_SIZE + 20.0).max(t.name_size + t.descr_size + 26.0);
        let ctx = ui.ctx().clone();

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        // The first known position is not a movement (the pointer may rest over the list).
        let pointer_moved = pointer.is_some() && self.last_pointer.is_some() && pointer != self.last_pointer;
        self.last_pointer = pointer;

        let (list_rect, list_resp) = ui.allocate_exact_size(
            vec2(width, row_h * rows.min(self.results.len()) as f32),
            Sense::hover(),
        );
        if list_resp.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.5 {
                let max_scroll = self.results.len().saturating_sub(rows);
                if scroll < 0.0 {
                    self.scroll = (self.scroll + 1).min(max_scroll);
                } else {
                    self.scroll = self.scroll.saturating_sub(1);
                }
                self.selected = self.selected.clamp(self.scroll, self.scroll + rows - 1);
            }
        }

        let jump_keys: Vec<char> = self.config.jump_keys.chars().collect();
        let mut clicked = None;
        let end = (self.scroll + rows).min(self.results.len());
        for i in self.scroll..end {
            let row = Rect::from_min_size(
                pos2(list_rect.left(), list_rect.top() + (i - self.scroll) as f32 * row_h),
                vec2(width, row_h),
            );
            let resp = ui.interact(row, ui.scope_id().with(("row", i)), Sense::click());
            if resp.hovered() && pointer_moved {
                self.selected = i;
            }
            if resp.clicked() {
                clicked = Some(i);
            }
            let sel = i == self.selected;
            let painter = ui.painter_at(row.expand(1.0));
            if sel {
                painter.rect_filled(row, CornerRadius::same(t.item_radius.min(60.0) as u8), t.item_box_selected);
            }

            let item = &self.results[i];
            let icon_rect = Rect::from_center_size(
                pos2(row.left() + 10.0 + ICON_SIZE / 2.0, row.center().y),
                Vec2::splat(ICON_SIZE),
            );
            let icon_spec = item.icon.clone();
            let title_initial = item.title.chars().next().unwrap_or('?');
            let shortcut_label = jump_keys.get(i).map(|k| format!("Alt+{k}"));
            let shortcut_color = if sel { t.item_shortcut_selected } else { t.item_shortcut };
            let mut text_right = row.right() - 12.0;
            if let Some(label) = &shortcut_label {
                let g = painter.layout_no_wrap(label.clone(), FontId::proportional(t.descr_size), shortcut_color);
                let pos = pos2(row.right() - 12.0 - g.size().x, row.center().y - g.size().y / 2.0);
                text_right = pos.x - 10.0;
                painter.galley(pos, g, shortcut_color);
            }

            let name_color = if sel { t.item_name_selected } else { t.item_name };
            let descr_color = if sel { t.item_text_selected } else { t.item_text };
            let hl_color = if sel { t.hl_selected } else { t.hl_not_selected };
            let text_left = icon_rect.right() + 12.0;
            let max_w = (text_right - text_left).max(20.0);

            let name_job = highlighted_job(&item.title, &item.highlight, t.name_size, name_color, hl_color, max_w);
            let name_galley = ctx.fonts_mut(|f| f.layout_job(name_job));
            let descr_galley = (!item.description.is_empty()).then(|| {
                let job = single_line_job(&item.description, t.descr_size, descr_color, max_w);
                ctx.fonts_mut(|f| f.layout_job(job))
            });
            let total_h = name_galley.size().y + descr_galley.as_ref().map_or(0.0, |g| g.size().y + 2.0);
            let mut y = row.center().y - total_h / 2.0;
            painter.galley(pos2(text_left, y), name_galley.clone(), name_color);
            y += name_galley.size().y + 2.0;
            if let Some(g) = descr_galley {
                painter.galley(pos2(text_left, y), g, descr_color);
            }

            match icon_spec.as_deref().map(|s| self.icon_texture(&ctx, s)) {
                Some(Some(tex)) => {
                    let size = tex.size_vec2();
                    let scale = ICON_SIZE / size.x.max(size.y);
                    let r = Rect::from_center_size(icon_rect.center(), size * scale);
                    painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                _ => {
                    painter.rect_filled(icon_rect.shrink(3.0), CornerRadius::same(8), hl_color.gamma_multiply(0.35));
                    painter.text(
                        icon_rect.center(),
                        Align2::CENTER_CENTER,
                        title_initial.to_uppercase().to_string(),
                        FontId::proportional(ICON_SIZE * 0.5),
                        name_color,
                    );
                }
            }
        }

        // Scroll indicator
        if self.results.len() > rows {
            let frac_start = self.scroll as f32 / self.results.len() as f32;
            let frac_len = rows as f32 / self.results.len() as f32;
            let h = list_rect.height();
            let bar = Rect::from_min_size(
                pos2(list_rect.right() - 3.0, list_rect.top() + h * frac_start),
                vec2(3.0, h * frac_len),
            );
            ui.painter().rect_filled(bar, CornerRadius::same(2), t.item_shortcut.gamma_multiply(0.5));
        }

        if let Some(i) = clicked {
            self.selected = i;
            self.activate(i, ctx.input(|i| i.modifiers.alt));
        }
    }

    fn icon_texture(&mut self, ctx: &egui::Context, spec: &str) -> Option<egui::TextureHandle> {
        match self.icons.get(spec) {
            None => {
                self.request_icon(spec);
                None
            }
            Some(IconState::Pending | IconState::Missing) => None,
            Some(IconState::Texture(t)) => Some(t.clone()),
            Some(IconState::Image(_)) => {
                let Some(IconState::Image(img)) = self.icons.remove(spec) else { return None };
                let tex = ctx.load_texture(format!("icon:{spec}"), img, egui::TextureOptions::LINEAR);
                self.icons.insert(spec.to_string(), IconState::Texture(tex.clone()));
                Some(tex)
            }
        }
    }
}

/// The settings button at the right end of the input row.
fn gear_button(ui: &mut egui::Ui, t: &Theme) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(GEAR_SIZE), Sense::click());
    // deliberately subtle: dim until hovered
    let color = if resp.hovered() { t.hl_selected } else { t.item_text.gamma_multiply(0.35) };
    paint_gear(ui.painter(), rect.center(), GEAR_SIZE * 0.32, color);
    resp.on_hover_text("Einstellungen")
}

fn paint_gear(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    // ring body, the center stays transparent
    painter.circle_stroke(center, radius * 0.55, Stroke::new(radius * 0.36, color));
    let teeth = 8;
    for i in 0..teeth {
        let a = i as f32 / teeth as f32 * std::f32::consts::TAU;
        let dir = vec2(a.cos(), a.sin());
        let side = vec2(-dir.y, dir.x) * radius * 0.21;
        let inner = center + dir * radius * 0.62;
        let outer = center + dir * radius * 1.1;
        painter.add(egui::Shape::convex_polygon(
            vec![inner - side * 1.15, outer - side * 0.8, outer + side * 0.8, inner + side * 1.15],
            color,
            Stroke::NONE,
        ));
    }
}

fn single_line_job(text: &str, size: f32, color: Color32, max_w: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.replace('\n', " "),
        TextFormat { font_id: FontId::proportional(size), color, ..Default::default() },
    );
    job.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job
}

fn highlighted_job(text: &str, hl: &[usize], size: f32, color: Color32, hl_color: Color32, max_w: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let normal = TextFormat { font_id: FontId::proportional(size), color, ..Default::default() };
    let highlighted = TextFormat {
        font_id: FontId::proportional(size),
        color: hl_color,
        underline: Stroke::NONE,
        ..Default::default()
    };
    let mut run = String::new();
    let mut run_hl = false;
    for (i, c) in text.chars().enumerate() {
        let is_hl = hl.binary_search(&i).is_ok();
        if is_hl != run_hl && !run.is_empty() {
            job.append(&run, 0.0, if run_hl { highlighted.clone() } else { normal.clone() });
            run.clear();
        }
        run_hl = is_hl;
        run.push(c);
    }
    if !run.is_empty() {
        job.append(&run, 0.0, if run_hl { highlighted } else { normal });
    }
    job.wrap = TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job
}

/// Uses the configured font, or the system's sans-serif font via fontconfig, with egui's
/// bundled fonts as fallback (for symbols and emoji).
fn install_fonts(ctx: &egui::Context, configured: &str) {
    let path = if configured.is_empty() {
        std::process::Command::new("fc-match")
            .args(["-f", "%{file}", "sans-serif:style=Regular"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|p| !p.is_empty())
    } else {
        Some(configured.to_string())
    };
    let mut fonts = egui::FontDefinitions::default();
    if let Some(path) = path
        && matches!(
            std::path::Path::new(&path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
            Some("ttf" | "otf")
        )
    {
        match std::fs::read(&path) {
            Ok(bytes) => {
                fonts.font_data.insert("system".into(), Arc::new(egui::FontData::from_owned(bytes)));
                if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                    family.insert(0, "system".into());
                }
                log::debug!("using font {path}");
            }
            Err(e) => log::warn!("cannot read font {path}: {e}"),
        }
    }
    ctx.set_fonts(fonts);
}

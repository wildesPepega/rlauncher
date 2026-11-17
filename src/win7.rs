//! The "Run dialog" layout: a Windows 7 style Run dialog (Aero glass frame, combo box,
//! OK / Cancel / Browse… buttons). The run history is shown as the combo box's drop-down.
//!
//! Geometry follows ExecBin's executedlg.ui (a QtWidgets recreation of the dialog):
//! a 397×175 client area with the icon at (18,22), the text at (72,6), the combo box at
//! (72,78) and the buttons at y = 137. No Microsoft resources are included.
//!
//! Two ways of showing it:
//! - as a normal window (`toplevel`): the compositor draws the real window frame (on
//!   AeroThemePlasma that is SMOD with Aero glass), the controls are drawn with the user's
//!   Kvantum theme when Qt apps use one, otherwise in the classic Windows 7 look;
//! - as an overlay (compositors without server-side decorations): the Aero frame is drawn here.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::apps::App;
use crate::config::{Config, RunPosition, Shortcut};
use crate::kvantum::Kvantum;
use crate::run;
use crate::ui::{NavKey, UiEvent};

/// Font family registered by the launcher (Segoe UI when installed).
pub const FONT: &str = "win7";

/// App id of the Run dialog window (KWin uses it to place the window and find its icon).
pub const RUN_APP_ID: &str = "io.github.rlauncher.run";
/// Size of the dialog's client area, as in ExecBin.
pub const CLIENT: Vec2 = vec2(397.0, 175.0);
const BORDER: f32 = 8.0;
const CAPTION: f32 = 30.0;
const FONT_SIZE: f32 = 12.0;
const ROW_H: f32 = 17.0;
const MAX_ROWS: usize = 8;

const TEXT: Color32 = Color32::BLACK;
const HIGHLIGHT: Color32 = Color32::from_rgb(0x33, 0x99, 0xff);
const STRIP: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);

fn font() -> FontId {
    FontId::new(FONT_SIZE, FontFamily::Name(FONT.into()))
}

#[derive(Default)]
pub struct RunDialog {
    pub text: String,
    history: Vec<String>,
    /// Entries of the drop-down.
    list: Vec<String>,
    dropdown: bool,
    selected: Option<usize>,
    scroll: usize,
    /// What the user typed before arrowing through the list.
    typed: String,
    select_all: bool,
    cursor_to_end: bool,
    /// The line that could not be resolved; shows the error message box.
    error: Option<String>,
    last_pointer: Option<Pos2>,
    /// KDE blurs the glass frame (`org_kde_kwin_blur`); otherwise it is drawn more opaque.
    pub blur_available: bool,
    /// Screen rects of the glass frames, for the blur region.
    pub glass: Vec<Rect>,
    /// Shown as a normal window with a compositor-drawn frame (instead of an overlay).
    pub toplevel: bool,
    /// The user's Kvantum theme, used for the controls when available.
    pub kv: Option<Kvantum>,
    kv_tex: Option<egui::TextureHandle>,
    /// The same sheet in the message box window's egui context.
    msg_kv_tex: Option<egui::TextureHandle>,
    /// Icons from the icon theme ("system-run", "dialog-error"), set by the launcher.
    pub run_icon: Option<egui::TextureId>,
    pub error_icon: Option<egui::TextureId>,
    /// Drop-down rows that fit.
    rows: usize,
}

impl RunDialog {
    /// Called when the dialog opens: like Windows, the last command is pre-filled and selected.
    pub fn reset(&mut self) {
        self.history = run::load_history();
        self.text = self.history.first().cloned().unwrap_or_default();
        self.typed = self.text.clone();
        self.select_all = true;
        self.close_dropdown();
        self.error = None;
        self.last_pointer = None;
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.typed = self.text.clone();
        self.cursor_to_end = true;
        self.select_all = false;
        self.close_dropdown();
    }

    fn close_dropdown(&mut self) {
        self.dropdown = false;
        self.selected = None;
        self.scroll = 0;
    }

    /// The text was edited by the user: show matching history entries (autocomplete).
    fn typed_changed(&mut self) {
        self.typed = self.text.clone();
        self.list = run::matches(&self.history, &self.text).into_iter().map(String::from).collect();
        self.dropdown = !self.text.trim().is_empty() && !self.list.is_empty();
        self.selected = None;
        self.scroll = 0;
    }

    fn open_full_history(&mut self) {
        self.list = self.history.clone();
        self.dropdown = !self.list.is_empty();
        self.selected = None;
        self.scroll = 0;
    }

    fn select(&mut self, index: Option<usize>) {
        self.selected = index;
        match index.and_then(|i| self.list.get(i)) {
            Some(entry) => self.text = entry.clone(),
            None => self.text = self.typed.clone(),
        }
        self.cursor_to_end = true;
        if let Some(i) = index {
            if i < self.scroll {
                self.scroll = i;
            } else if i >= self.scroll + self.rows.max(1) {
                self.scroll = i + 1 - self.rows.max(1);
            }
        }
    }

    pub fn nav(&mut self, key: NavKey, apps: &[App], shortcuts: &[Shortcut], events: &mut Vec<UiEvent>) {
        if self.error.is_some() {
            if matches!(key, NavKey::Enter { .. } | NavKey::Escape) {
                self.error = None;
                self.select_all = true;
            }
            return;
        }
        let n = self.list.len();
        match key {
            NavKey::Escape if self.dropdown => {
                self.select(None);
                self.close_dropdown();
            }
            NavKey::Escape => events.push(UiEvent::Hide),
            NavKey::Down | NavKey::PageDown => {
                if !self.dropdown {
                    self.typed = self.text.clone();
                    let matching = run::matches(&self.history, &self.text);
                    if matching.is_empty() || self.text.trim().is_empty() {
                        self.open_full_history();
                    } else {
                        self.list = matching.into_iter().map(String::from).collect();
                        self.dropdown = true;
                    }
                    if self.dropdown {
                        self.select(Some(0));
                    }
                } else if n > 0 {
                    let step = if matches!(key, NavKey::PageDown) { self.rows.max(1) } else { 1 };
                    self.select(Some(self.selected.map_or(0, |i| (i + step).min(n - 1))));
                }
            }
            NavKey::Up | NavKey::PageUp if self.dropdown => {
                let step = if matches!(key, NavKey::PageUp) { self.rows.max(1) } else { 1 };
                self.select(match self.selected {
                    Some(i) if i >= step => Some(i - step),
                    Some(i) if i > 0 => Some(0),
                    _ => None,
                });
            }
            NavKey::Enter { .. } => self.run(apps, shortcuts, events),
            _ => {}
        }
    }

    fn run(&mut self, apps: &[App], shortcuts: &[Shortcut], events: &mut Vec<UiEvent>) {
        let text = self.text.trim().to_string();
        self.close_dropdown();
        if text.is_empty() {
            return;
        }
        match run::resolve(&text, apps, shortcuts) {
            Some(spec) => {
                run::push_history(&mut self.history, &text);
                events.push(UiEvent::Launch(spec));
            }
            None => self.error = Some(text),
        }
    }

    /// Outer rect of the overlay dialog (glass frame included).
    fn outer_rect(config: &Config, screen: Rect) -> Rect {
        let size = CLIENT + vec2(2.0 * BORDER, CAPTION + BORDER);
        let min = match config.run_position {
            RunPosition::BottomLeft => pos2(screen.left() + 8.0, screen.bottom() - 8.0 - size.y),
            RunPosition::Center => pos2(
                screen.center().x - size.x / 2.0,
                screen.top() + screen.height() * config.top_offset.clamp(0.0, 0.8),
            ),
        };
        Rect::from_min_size(pos2(min.x.round(), min.y.round().max(screen.top())), size)
    }

    /// The line that could not be found, while the error message box is shown.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Uploads the Kvantum sheet (once per egui context) and returns how to draw the controls.
    fn take_look(&mut self, ctx: &egui::Context, message: bool) -> (Option<Kvantum>, Palette, Option<egui::TextureId>) {
        if let Some(k) = self.kv.as_mut() {
            if !message && let Some(sheet) = k.sheet.take() {
                self.kv_tex = Some(ctx.load_texture("kvantum", sheet, egui::TextureOptions::LINEAR));
            }
            if message && self.msg_kv_tex.is_none() && let Some(sheet) = k.render_sheet() {
                self.msg_kv_tex = Some(ctx.load_texture("kvantum", sheet, egui::TextureOptions::LINEAR));
            }
        }
        let kv = self.kv.take();
        let pal = kv.as_ref().map(Palette::kvantum).unwrap_or_else(Palette::classic);
        let handle = if message { &self.msg_kv_tex } else { &self.kv_tex };
        let tex = if kv.is_some() { handle.as_ref().map(|t| t.id()) } else { None };
        (kv, pal, tex)
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        config: &Config,
        apps: &[App],
        shortcuts: &[Shortcut],
        events: &mut Vec<UiEvent>,
    ) {
        let (kv, pal, tex) = self.take_look(ctx, false);
        let look = Look { kv: kv.as_ref().zip(tex), pal };
        let screen = ctx.content_rect();
        let (outer, client) = if self.toplevel {
            (None, Rect::from_min_size(screen.min, CLIENT))
        } else {
            let outer = Self::outer_rect(config, screen);
            (Some(outer), Rect::from_min_size(outer.min + vec2(BORDER, CAPTION), CLIENT))
        };
        let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(client.min + vec2(x, y), vec2(w, h));
        self.glass = outer.into_iter().collect();
        let mut hit_rects: Vec<Rect> = outer.into_iter().collect();

        egui::Area::new(egui::Id::unique("rlauncher-run"))
            .fade_in(false)
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                look.apply(ui);
                // the message box is a window of its own in toplevel mode
                let blocked = self.error.is_some();

                // ---- frame (overlay only)
                if let Some(outer) = outer {
                    paint_window(ui.painter(), outer, client, "Run", self.blur_available);
                    if close_button(ui, outer, "run-close").clicked() && !blocked {
                        events.push(UiEvent::Hide);
                    }
                }

                // ---- client area: window color with the list-view-colored upper part
                let p = ui.painter();
                p.rect_filled(client, 0.0, pal.window);
                p.rect_filled(at(2.0, 2.0, 394.0, 122.0), 0.0, pal.base);

                let icon = at(18.0, 22.0, 32.0, 32.0);
                match self.run_icon {
                    Some(t) => {
                        p.image(t, icon, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => paint_run_icon(p, icon),
                }
                let icon_resp = ui.interact(icon, ui.scope_id().with("run-icon"), Sense::click());
                if icon_resp.clicked() && !blocked {
                    events.push(UiEvent::OpenSettings);
                }
                icon_resp.on_hover_text("rlauncher settings");

                let desc = at(72.0, 6.0, 313.0, 62.0);
                let galley = ui.painter().layout(
                    "Type the name of a program, folder, document, or Internet resource, and rlauncher will open it for you."
                        .into(),
                    font(),
                    pal.text,
                    desc.width(),
                );
                let pos = pos2(desc.left(), desc.center().y - galley.size().y / 2.0);
                ui.painter().galley(pos, galley, pal.text);

                let label = at(12.0, 72.0, 42.0, 33.0);
                let g = ui.painter().layout_job(mnemonic("Open:", Some(0), pal.text));
                ui.painter().galley(pos2(label.left(), label.center().y - g.size().y / 2.0 + 1.0), g, pal.text);

                // ---- combo box
                let combo = at(72.0, 78.0, 308.0, 24.0);
                let arrow = Rect::from_min_max(pos2(combo.right() - 18.0, combo.top() + 1.0), combo.max - vec2(1.0, 1.0));
                let arrow_resp = ui.interact(arrow, ui.scope_id().with("run-arrow"), Sense::click());
                look.combo(ui.painter(), combo, arrow, arrow_resp.hovered(), self.dropdown);
                if arrow_resp.clicked() && !blocked {
                    if self.dropdown {
                        self.close_dropdown();
                    } else {
                        self.typed = self.text.clone();
                        self.open_full_history();
                    }
                }

                let edit_rect = Rect::from_min_max(combo.min + vec2(4.0, 1.0), pos2(arrow.left() - 1.0, combo.bottom() - 1.0));
                let id = egui::Id::unique("rlauncher-run-input");
                let edit = egui::TextEdit::singleline(&mut self.text)
                    .id(id)
                    .font(font())
                    .text_color(pal.edit_text)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .align(egui::Align2::LEFT_CENTER)
                    .min_size(edit_rect.size())
                    .desired_width(edit_rect.width());
                let mut output = ui.scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| edit.show(ui)).inner;
                if self.select_all || self.cursor_to_end {
                    let n = self.text.chars().count();
                    let range = if self.select_all {
                        egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))
                    } else {
                        egui::text::CCursorRange::one(egui::text::CCursor::new(n))
                    };
                    output.state.cursor.set_char_range(Some(range));
                    output.state.store(ui.ctx(), id);
                    self.select_all = false;
                    self.cursor_to_end = false;
                }
                if output.response.changed() {
                    self.typed_changed();
                }
                if !blocked {
                    output.response.request_focus();
                } else {
                    output.response.surrender_focus();
                }

                // ---- buttons
                let enabled = !self.text.trim().is_empty();
                if look.button(ui, at(110.0, 137.0, 88.0, 24.0), "OK", None, enabled, true).clicked() && !blocked {
                    self.run(apps, shortcuts, events);
                }
                if look.button(ui, at(206.0, 137.0, 86.0, 24.0), "Cancel", None, true, false).clicked() && !blocked {
                    events.push(UiEvent::Hide);
                }
                // "Browse..." opens rlauncher's settings
                if look.button(ui, at(300.0, 137.0, 88.0, 24.0), "Browse...", Some(0), true, false).clicked() && !blocked {
                    events.push(UiEvent::OpenSettings);
                }

                // ---- drop-down with the run history
                if self.dropdown && !self.list.is_empty() {
                    // a window can't draw outside itself: then the list has to fit below the combo box
                    let bounds = if self.toplevel { client } else { screen };
                    let rect = self.dropdown_ui(ui, combo, bounds, &look);
                    hit_rects.push(rect);
                }

                // ---- error message box (overlay mode)
                if !self.toplevel
                    && let (Some(text), Some(outer)) = (self.error.clone(), outer)
                {
                    let (rect, close) = overlay_message_box(ui, outer, screen, &text, self.blur_available, &look, self.error_icon);
                    self.glass.push(rect);
                    hit_rects.push(rect);
                    if close {
                        self.close_error();
                    }
                }
            });

        // Clicking outside of the overlay dialog hides it.
        if !self.toplevel {
            let (pressed, pos) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos()));
            if pressed
                && config.close_on_focus_out
                && let Some(p) = pos
                && !hit_rects.iter().any(|r| r.contains(p))
            {
                if self.dropdown {
                    self.close_dropdown();
                } else {
                    events.push(UiEvent::Hide);
                }
            }
        }
        self.kv = kv;
    }

    pub fn close_error(&mut self) {
        self.error = None;
        self.select_all = true;
    }

    fn message_text(text: &str) -> String {
        format!("rlauncher cannot find '{text}'. Make sure you typed the name correctly, and then try again.")
    }

    /// Client size of the error message box window.
    pub fn message_size(&self, ctx: &egui::Context) -> Vec2 {
        let text = Self::message_text(self.error.as_deref().unwrap_or_default());
        let galley = ctx.fonts_mut(|f| f.layout(text, font(), Color32::WHITE, MSG_TEXT_W));
        message_client_size(galley.size())
    }

    /// The error message box as a window of its own (toplevel mode).
    pub fn message_ui(&mut self, ctx: &egui::Context) {
        let Some(text) = self.error.clone() else { return };
        let (kv, pal, tex) = self.take_look(ctx, true);
        let look = Look { kv: kv.as_ref().zip(tex), pal };
        let mut close = false;
        egui::Area::new(egui::Id::unique("rlauncher-msgbox"))
            .fade_in(false)
            .fixed_pos(ctx.content_rect().min)
            .show(ctx, |ui| {
                look.apply(ui);
                let client = ctx.content_rect();
                close = message_client(ui, client, &text, &look, self.error_icon);
            });
        if close {
            self.close_error();
        }
        self.kv = kv;
    }

    fn dropdown_ui(&mut self, ui: &mut egui::Ui, combo: Rect, bounds: Rect, look: &Look) -> Rect {
        let pal = look.pal;
        let room_below = ((bounds.bottom() - combo.bottom() - 2.0) / ROW_H).floor().max(0.0) as usize;
        let room_above = ((combo.top() - bounds.top() - 2.0) / ROW_H).floor().max(0.0) as usize;
        let wanted = self.list.len().min(MAX_ROWS);
        let below = room_below >= wanted || room_below >= room_above;
        let rows = wanted.min(if below { room_below } else { room_above }).max(1);
        self.rows = rows;
        let h = rows as f32 * ROW_H + 2.0;
        let rect = if below {
            Rect::from_min_size(pos2(combo.left(), combo.bottom()), vec2(combo.width(), h))
        } else {
            Rect::from_min_size(pos2(combo.left(), combo.top() - h), vec2(combo.width(), h))
        };
        let p = ui.painter();
        p.add(egui::Shadow { offset: [2, 2], blur: 6, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(rect, 0));
        p.rect_filled(rect, 0.0, pal.base);
        p.rect_stroke(rect, 0.0, Stroke::new(1.0, pal.list_border), StrokeKind::Inside);

        let resp = ui.interact(rect, ui.scope_id().with("run-dropdown"), Sense::click());
        let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
        let moved = pointer.is_some() && pointer != self.last_pointer && self.last_pointer.is_some();
        if pointer.is_some() {
            self.last_pointer = pointer;
        }
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll.abs() > 0.5 {
            let max = self.list.len().saturating_sub(rows);
            self.scroll = if scroll < 0.0 { (self.scroll + 1).min(max) } else { self.scroll.saturating_sub(1) };
        }
        self.scroll = self.scroll.min(self.list.len().saturating_sub(rows));

        let mut clicked = None;
        for (row, i) in (self.scroll..self.list.len()).take(rows).enumerate() {
            let r = Rect::from_min_size(pos2(rect.left() + 1.0, rect.top() + 1.0 + row as f32 * ROW_H), vec2(rect.width() - 2.0, ROW_H));
            if let Some(pp) = pointer
                && r.contains(pp)
            {
                if moved && self.selected != Some(i) {
                    self.selected = Some(i);
                }
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
            let sel = self.selected == Some(i);
            let color = if sel { look.selected_row(ui.painter(), r) } else { pal.edit_text };
            let g = ui.painter().layout_no_wrap(self.list[i].clone(), font(), color);
            let clip = ui.painter().with_clip_rect(r.shrink2(vec2(3.0, 0.0)));
            clip.galley(pos2(r.left() + 3.0, r.center().y - g.size().y / 2.0), g, color);
        }
        if let Some(i) = clicked {
            self.select(Some(i));
            self.typed = self.text.clone();
            self.close_dropdown();
        }
        rect
    }
}

/// Colors of the dialog: the classic Windows 7 ones or the Kvantum theme's.
#[derive(Clone, Copy)]
struct Palette {
    window: Color32,
    base: Color32,
    text: Color32,
    edit_text: Color32,
    button_text: Color32,
    disabled_text: Color32,
    highlight: Color32,
    highlight_text: Color32,
    list_border: Color32,
}

impl Palette {
    fn classic() -> Palette {
        Palette {
            window: STRIP,
            base: Color32::WHITE,
            text: TEXT,
            edit_text: TEXT,
            button_text: TEXT,
            disabled_text: rgba(0x83, 0x83, 0x83, 255),
            highlight: HIGHLIGHT,
            highlight_text: Color32::WHITE,
            list_border: rgba(0x64, 0x64, 0x64, 255),
        }
    }

    fn kvantum(k: &Kvantum) -> Palette {
        let c = Palette::classic();
        let window = k.color("window.color").unwrap_or(c.window);
        let text = k.color("window.text.color").or_else(|| k.color("text.color")).unwrap_or(c.text);
        Palette {
            window,
            base: k.color("base.color").unwrap_or(c.base),
            text,
            edit_text: k.color("text.color").unwrap_or(text),
            button_text: k
                .section_color("PanelButtonCommand", "text.normal.color")
                .or_else(|| k.color("button.text.color"))
                .unwrap_or(text),
            disabled_text: k.color("disabled.text.color").unwrap_or(c.disabled_text),
            highlight: k.color("highlight.color").unwrap_or(c.highlight),
            highlight_text: k.color("highlight.text.color").unwrap_or(c.highlight_text),
            list_border: k.color("light.color").or_else(|| k.color("mid.color")).unwrap_or(c.list_border),
        }
    }
}

/// How controls are drawn: with the Kvantum theme (sheet texture) or classic.
struct Look<'a> {
    kv: Option<(&'a Kvantum, egui::TextureId)>,
    pal: Palette,
}

impl Look<'_> {
    fn apply(&self, ui: &mut egui::Ui) {
        let v = ui.visuals_mut();
        v.selection.bg_fill = self.pal.highlight;
        v.selection.stroke = Stroke::new(1.0, self.pal.highlight_text);
        v.text_cursor.stroke = Stroke::new(1.0, self.pal.edit_text);
        v.override_text_color = Some(self.pal.text);
    }

    fn combo(&self, p: &egui::Painter, combo: Rect, arrow: Rect, arrow_hover: bool, open: bool) {
        let Some((k, tex)) = self.kv else {
            paint_combo(p, combo, arrow, arrow_hover, open);
            return;
        };
        // Kvantum draws editable combo boxes like line edits (combo_as_lineedit)
        p.rect_filled(combo, 0.0, self.pal.base);
        if !k.paint_frame(p, tex, "ledit-focused", combo, k.frame("LineEdit"), Color32::WHITE) {
            k.paint_frame(p, tex, "ledit-normal", combo, k.frame("LineEdit"), Color32::WHITE);
        }
        // a focused editable combo box shows the "pressed" arrow in Qt (like the Qt reference)
        let state = if arrow_hover && !open { "focused" } else { "pressed" };
        let size = k.get("ComboBox", "indicator.size").and_then(|v| v.parse::<f32>().ok()).unwrap_or(8.0) + 1.0;
        let r = Rect::from_center_size(pos2(arrow.center().x - 1.0, arrow.center().y), vec2(size, size));
        if !k.paint_element(p, tex, &format!("arrow-down-{state}"), r, Color32::WHITE) {
            k.paint_element(p, tex, "arrow-down-normal", r, Color32::WHITE);
        }
    }

    /// Paints the highlight of a selected list row and returns the text color for it.
    fn selected_row(&self, p: &egui::Painter, r: Rect) -> Color32 {
        if let Some((k, tex)) = self.kv
            && k.paint_frame(p, tex, "iview-pressed", r, k.frame("ItemView"), Color32::WHITE)
        {
            return k.section_color("ItemView", "text.press.color").unwrap_or(self.pal.edit_text);
        }
        p.rect_filled(r, 0.0, self.pal.highlight);
        self.pal.highlight_text
    }

    fn button(&self, ui: &mut egui::Ui, rect: Rect, label: &str, underline: Option<usize>, enabled: bool, default: bool) -> egui::Response {
        let Some((k, tex)) = self.kv else {
            return button(ui, rect, label, underline, enabled, default);
        };
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let resp = ui.interact(rect, ui.scope_id().with(("kv-button", label)), sense);
        let p = ui.painter();
        let pressed = enabled && resp.is_pointer_button_down_on();
        let hovered = enabled && resp.hovered();
        let state = if pressed {
            "pressed"
        } else if hovered {
            "focused"
        } else if enabled && default && k.has("btn-default") {
            "default"
        } else {
            "normal"
        };
        let tint = if enabled { Color32::WHITE } else { Color32::from_white_alpha(140) };
        k.paint_frame(p, tex, &format!("btn-{state}"), rect, k.frame("PanelButtonCommand"), tint);
        let color = if !enabled {
            self.pal.disabled_text
        } else if pressed {
            k.section_color("PanelButtonCommand", "text.press.color").unwrap_or(self.pal.button_text)
        } else if hovered {
            k.section_color("PanelButtonCommand", "text.focus.color").unwrap_or(self.pal.button_text)
        } else {
            self.pal.button_text
        };
        let g = p.layout_job(mnemonic(label, underline, color));
        let shift = if pressed { vec2(0.0, 1.0) } else { Vec2::ZERO };
        p.galley(rect.center() - g.size() / 2.0 + shift, g, color);
        resp
    }
}

/// Text with one underlined character (keyboard mnemonic, like "_Open:").
fn mnemonic(text: &str, underline: Option<usize>, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (i, c) in text.chars().enumerate() {
        let fmt = TextFormat {
            font_id: font(),
            color,
            underline: if Some(i) == underline { Stroke::new(1.0, color) } else { Stroke::NONE },
            ..Default::default()
        };
        job.append(&c.to_string(), 0.0, fmt);
    }
    job
}

// ----------------------------------------------------------------------------- painting

/// Fills a rounded rect with a vertical gradient. `stops` are (position 0..1, color); two
/// stops at the same position give a hard edge (the glossy look of Windows 7 buttons).
pub fn gradient_rect(painter: &egui::Painter, rect: Rect, radius: f32, stops: &[(f32, Color32)]) {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let span = |y: f32| {
        let dy = if y < rect.top() + r {
            rect.top() + r - y
        } else if y > rect.bottom() - r {
            y - (rect.bottom() - r)
        } else {
            0.0
        };
        let inset = if dy > 0.0 { r - (r * r - dy * dy).max(0.0).sqrt() } else { 0.0 };
        (rect.left() + inset, rect.right() - inset)
    };
    let mut mesh = egui::Mesh::default();
    let mut push_band = |ya: f32, ca: Color32, yb: f32, cb: Color32| {
        // sample the corner arcs so the rounded ends stay round
        let mut ys = vec![ya];
        let mut y = ya.floor() + 1.0;
        while y < yb {
            if y < rect.top() + r || y > rect.bottom() - r {
                ys.push(y);
            }
            y += 1.0;
        }
        ys.push(yb);
        for w in ys.windows(2) {
            let (y0, y1) = (w[0], w[1]);
            let f = |y: f32| if yb > ya { (y - ya) / (yb - ya) } else { 0.0 };
            let (c0, c1) = (ca.lerp_to_gamma(cb, f(y0)), ca.lerp_to_gamma(cb, f(y1)));
            let (a0, b0) = span(y0);
            let (a1, b1) = span(y1);
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(a0, y0), c0);
            mesh.colored_vertex(pos2(b0, y0), c0);
            mesh.colored_vertex(pos2(b1, y1), c1);
            mesh.colored_vertex(pos2(a1, y1), c1);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
    };
    for w in stops.windows(2) {
        let (ya, yb) = (rect.top() + w[0].0 * rect.height(), rect.top() + w[1].0 * rect.height());
        if yb > ya {
            push_band(ya, w[0].1, yb, w[1].1);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

/// The Aero glass frame with caption and client edge.
fn paint_window(p: &egui::Painter, outer: Rect, client: Rect, title: &str, blur: bool) {
    p.add(egui::Shadow { offset: [0, 4], blur: 18, spread: 1, color: Color32::from_black_alpha(110) }.as_shape(outer, 7));
    // Windows 7 "Sky" glass. Without compositor blur it has to be more opaque.
    let a = if blur { 150 } else { 236 };
    gradient_rect(
        p,
        outer,
        6.0,
        &[(0.0, rgba(196, 220, 244, a)), (0.16, rgba(170, 203, 236, a)), (1.0, rgba(150, 188, 228, a))],
    );
    // diagonal reflections
    let clip = p.with_clip_rect(outer.shrink(2.0));
    for (x, w, alpha) in [(0.18, 34.0, 28u8), (0.30, 12.0, 20), (0.62, 46.0, 22), (0.80, 16.0, 16)] {
        let x0 = outer.left() + outer.width() * x;
        let pts = vec![
            pos2(x0, outer.top()),
            pos2(x0 + w, outer.top()),
            pos2(x0 + w - 60.0, outer.bottom()),
            pos2(x0 - 60.0, outer.bottom()),
        ];
        clip.add(egui::Shape::convex_polygon(pts, Color32::from_white_alpha(alpha), Stroke::NONE));
    }
    gradient_rect(
        p,
        Rect::from_min_size(outer.min, vec2(outer.width(), CAPTION * 0.55)),
        6.0,
        &[(0.0, Color32::from_white_alpha(70)), (1.0, Color32::from_white_alpha(0))],
    );
    p.rect_stroke(outer, 6.0, Stroke::new(1.0, rgba(0, 0, 0, 150)), StrokeKind::Inside);
    p.rect_stroke(outer.shrink(1.0), 5.0, Stroke::new(1.0, Color32::from_white_alpha(120)), StrokeKind::Inside);
    // client edge: a dark line with a light one outside
    p.rect_stroke(client.expand(2.0), 1.0, Stroke::new(1.0, Color32::from_white_alpha(110)), StrokeKind::Inside);
    p.rect_stroke(client.expand(1.0), 0.0, Stroke::new(1.0, rgba(40, 60, 90, 120)), StrokeKind::Inside);

    // caption: small icon and the title with its white glow
    paint_run_icon(p, Rect::from_min_size(outer.min + vec2(8.0, 7.0), vec2(16.0, 16.0)));
    let g = p.layout_no_wrap(title.into(), font(), TEXT);
    let pos = pos2(outer.left() + 30.0, outer.top() + 15.0 - g.size().y / 2.0);
    let glow = Rect::from_min_size(pos, g.size()).expand2(vec2(2.0, -2.0));
    p.with_clip_rect(outer.shrink(2.0))
        .add(egui::Shadow { offset: [0, 0], blur: 10, spread: 0, color: Color32::from_white_alpha(170) }.as_shape(glow, 5));
    p.galley(pos, g, TEXT);
}

/// The red caption button that closes the window.
fn close_button(ui: &mut egui::Ui, outer: Rect, id: &str) -> egui::Response {
    let rect = Rect::from_min_size(pos2(outer.right() - 6.0 - 43.0, outer.top() + 1.0), vec2(43.0, 18.0));
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    let p = ui.painter();
    let (top, mid_hi, mid_lo, bottom) = if resp.is_pointer_button_down_on() {
        (rgba(0xd1, 0x8c, 0x7d, 255), rgba(0xb2, 0x41, 0x2c, 255), rgba(0x8b, 0x18, 0x06, 255), rgba(0xc4, 0x56, 0x2d, 255))
    } else if resp.hovered() {
        (rgba(0xf3, 0xb6, 0xa8, 255), rgba(0xe8, 0x6f, 0x5b, 255), rgba(0xd4, 0x2a, 0x10, 255), rgba(0xf2, 0x9a, 0x5a, 255))
    } else {
        (rgba(0xe5, 0xa7, 0x99, 255), rgba(0xcf, 0x6b, 0x59, 255), rgba(0xb8, 0x35, 0x1f, 255), rgba(0xd7, 0x80, 0x5c, 255))
    };
    gradient_rect(p, rect, 3.0, &[(0.0, top), (0.5, mid_hi), (0.5, mid_lo), (1.0, bottom)]);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, rgba(60, 20, 10, 200)), StrokeKind::Inside);
    p.rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(1.0, Color32::from_white_alpha(70)), StrokeKind::Inside);
    let c = rect.center();
    let d = 4.0;
    for (w, color) in [(3.2, rgba(70, 20, 10, 170)), (1.8, Color32::WHITE)] {
        p.line_segment([c + vec2(-d, -d + 0.5), c + vec2(d, d + 0.5)], Stroke::new(w, color));
        p.line_segment([c + vec2(-d, d + 0.5), c + vec2(d, -d + 0.5)], Stroke::new(w, color));
    }
    resp
}

/// An original "run a program" icon: a small window with a command prompt.
fn paint_run_icon(p: &egui::Painter, rect: Rect) {
    let s = rect.width() / 32.0;
    let at = |x: f32, y: f32| rect.min + vec2(x * s, y * s);
    let win = Rect::from_min_max(at(2.0, 5.0), at(30.0, 27.0));
    p.add(egui::Shadow { offset: [0, 1], blur: (2.0 * s) as u8, spread: 0, color: Color32::from_black_alpha(60) }.as_shape(win, 2.0 * s));
    gradient_rect(p, win, 2.0 * s, &[(0.0, Color32::WHITE), (1.0, rgba(0xe4, 0xec, 0xf5, 255))]);
    let bar = Rect::from_min_max(win.min, pos2(win.right(), win.top() + 7.0 * s));
    gradient_rect(p, bar, 2.0 * s, &[(0.0, rgba(0x7d, 0xb3, 0xec, 255)), (1.0, rgba(0x2b, 0x67, 0xb4, 255))]);
    p.rect_stroke(win, 2.0 * s, Stroke::new(s.max(0.8), rgba(0x3a, 0x5f, 0x8f, 255)), StrokeKind::Inside);
    let ink = Stroke::new((2.2 * s).max(1.1), rgba(0x24, 0x2c, 0x38, 255));
    p.line_segment([at(8.0, 13.5), at(12.5, 17.0)], ink);
    p.line_segment([at(12.5, 17.0), at(8.0, 20.5)], ink);
    p.line_segment([at(14.5, 21.0), at(21.0, 21.0)], ink);
}

/// The combo box frame and its drop-down arrow.
fn paint_combo(p: &egui::Painter, combo: Rect, arrow: Rect, arrow_hover: bool, open: bool) {
    p.rect_filled(combo, 0.0, Color32::WHITE);
    // focused border: darker blue on top, lighter at the sides and bottom
    let (top, side, bottom) = (rgba(0x3d, 0x7b, 0xad, 255), rgba(0xa4, 0xc9, 0xe3, 255), rgba(0xb7, 0xd9, 0xed, 255));
    let r = combo.shrink(0.5);
    p.line_segment([r.left_top(), r.right_top()], Stroke::new(1.0, top));
    p.line_segment([r.left_top(), r.left_bottom()], Stroke::new(1.0, side));
    p.line_segment([r.right_top(), r.right_bottom()], Stroke::new(1.0, side));
    p.line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0, bottom));
    if arrow_hover || open {
        let stops: &[(f32, Color32)] = if open {
            &[(0.0, rgba(0xe5, 0xf4, 0xfc, 255)), (0.5, rgba(0xc4, 0xe5, 0xf6, 255)), (0.5, rgba(0x98, 0xd1, 0xef, 255)), (1.0, rgba(0x68, 0xb3, 0xdb, 255))]
        } else {
            &[(0.0, rgba(0xea, 0xf6, 0xfd, 255)), (0.5, rgba(0xd9, 0xf0, 0xfc, 255)), (0.5, rgba(0xbe, 0xe6, 0xfd, 255)), (1.0, rgba(0xa7, 0xd9, 0xf5, 255))]
        };
        gradient_rect(p, arrow, 2.0, stops);
        p.rect_stroke(arrow, 2.0, Stroke::new(1.0, rgba(0x3c, 0x7f, 0xb1, 255)), StrokeKind::Inside);
    }
    let c = arrow.center() + vec2(0.0, 0.5);
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-3.5, -2.0), c + vec2(3.5, -2.0), c + vec2(0.0, 2.0)],
        TEXT,
        Stroke::NONE,
    ));
}

/// A Windows 7 push button.
fn button(ui: &mut egui::Ui, rect: Rect, label: &str, underline: Option<usize>, enabled: bool, default: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, ui.scope_id().with(("win7-button", label)), sense);
    let p = ui.painter();
    let (border, stops): (Color32, [(f32, Color32); 4]) = if !enabled {
        (rgba(0xad, 0xb2, 0xb5, 255), [(0.0, rgba(0xf4, 0xf4, 0xf4, 255)), (0.5, rgba(0xf4, 0xf4, 0xf4, 255)), (0.5, rgba(0xf4, 0xf4, 0xf4, 255)), (1.0, rgba(0xf4, 0xf4, 0xf4, 255))])
    } else if resp.is_pointer_button_down_on() {
        (rgba(0x2c, 0x62, 0x8b, 255), [(0.0, rgba(0xe5, 0xf4, 0xfc, 255)), (0.5, rgba(0xc4, 0xe5, 0xf6, 255)), (0.5, rgba(0x98, 0xd1, 0xef, 255)), (1.0, rgba(0x68, 0xb3, 0xdb, 255))])
    } else if resp.hovered() {
        (rgba(0x3c, 0x7f, 0xb1, 255), [(0.0, rgba(0xea, 0xf6, 0xfd, 255)), (0.5, rgba(0xd9, 0xf0, 0xfc, 255)), (0.5, rgba(0xbe, 0xe6, 0xfd, 255)), (1.0, rgba(0xa7, 0xd9, 0xf5, 255))])
    } else {
        let border = if default { rgba(0x3c, 0x7f, 0xb1, 255) } else { rgba(0x70, 0x70, 0x70, 255) };
        (border, [(0.0, rgba(0xf2, 0xf2, 0xf2, 255)), (0.5, rgba(0xeb, 0xeb, 0xeb, 255)), (0.5, rgba(0xdd, 0xdd, 0xdd, 255)), (1.0, rgba(0xcf, 0xcf, 0xcf, 255))])
    };
    gradient_rect(p, rect, 3.0, &stops);
    let inner = if enabled && default && !resp.hovered() { rgba(0x9a, 0xd4, 0xf5, 255) } else { Color32::from_white_alpha(if enabled { 170 } else { 0 }) };
    p.rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(1.0, inner), StrokeKind::Inside);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, border), StrokeKind::Inside);
    let color = if enabled { TEXT } else { rgba(0x83, 0x83, 0x83, 255) };
    let g = p.layout_job(mnemonic(label, underline, color));
    p.galley(rect.center() - g.size() / 2.0, g, color);
    resp
}

const MSG_TEXT_W: f32 = 290.0;

fn message_client_size(text: Vec2) -> Vec2 {
    vec2(MSG_TEXT_W + 74.0 + 24.0, (text.y.max(32.0) + 2.0 * 26.0).round() + 41.0)
}

/// The content of the "cannot find" message box. Returns true when OK was clicked.
fn message_client(ui: &mut egui::Ui, client: Rect, text: &str, look: &Look, icon: Option<egui::TextureId>) -> bool {
    let pal = look.pal;
    let galley = ui.painter().layout(RunDialog::message_text(text), font(), pal.text, MSG_TEXT_W);
    let p = ui.painter();
    let strip_top = client.bottom() - 41.0;
    p.rect_filled(client, 0.0, pal.window);
    p.rect_filled(Rect::from_min_max(client.min, pos2(client.right(), strip_top)), 0.0, pal.base);
    if look.kv.is_none() {
        p.hline(client.x_range(), strip_top + 0.5, Stroke::new(1.0, rgba(0xdf, 0xdf, 0xdf, 255)));
    }
    let icon_rect = Rect::from_min_size(client.min + vec2(26.0, 26.0), Vec2::splat(32.0));
    match icon {
        Some(t) => {
            p.image(t, icon_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => paint_error_icon(p, icon_rect.center()),
    }
    let tpos = pos2(client.left() + 74.0, client.top() + 26.0 + (32.0 - galley.size().y).max(0.0) / 2.0);
    p.galley(tpos, galley, pal.text);
    let ok = Rect::from_min_size(pos2(client.right() - 11.0 - 75.0, strip_top + 9.0), vec2(75.0, 23.0));
    look.button(ui, ok, "OK", None, true, true).clicked()
}

fn paint_error_icon(p: &egui::Painter, c: Pos2) {
    let r = Rect::from_center_size(c, Vec2::splat(30.0));
    p.add(egui::Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(80) }.as_shape(r, 15));
    gradient_rect(
        p,
        r,
        15.0,
        &[(0.0, rgba(0xf6, 0x8d, 0x7b, 255)), (0.5, rgba(0xe0, 0x30, 0x1e, 255)), (1.0, rgba(0xa9, 0x10, 0x05, 255))],
    );
    p.circle_stroke(c, 14.5, Stroke::new(1.0, rgba(0x8a, 0x10, 0x05, 255)));
    let d = 6.0;
    let w = Stroke::new(3.0, Color32::WHITE);
    p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], w);
    p.line_segment([c + vec2(-d, d), c + vec2(d, -d)], w);
}

/// The message box drawn with its own glass frame over the overlay dialog. Returns its
/// outer rect and whether it was dismissed.
fn overlay_message_box(
    ui: &mut egui::Ui,
    owner: Rect,
    screen: Rect,
    text: &str,
    blur: bool,
    look: &Look,
    icon: Option<egui::TextureId>,
) -> (Rect, bool) {
    let galley = ui.painter().layout(RunDialog::message_text(text), font(), TEXT, MSG_TEXT_W);
    let client_size = message_client_size(galley.size());
    let outer_size = client_size + vec2(2.0 * BORDER, CAPTION + BORDER);
    let c = owner.center() + vec2(12.0, 12.0) - outer_size / 2.0;
    let c = pos2(
        c.x.clamp(screen.left(), (screen.right() - outer_size.x).max(screen.left())),
        c.y.clamp(screen.top(), (screen.bottom() - outer_size.y).max(screen.top())),
    );
    let outer = Rect::from_min_size(pos2(c.x.round(), c.y.round()), outer_size);
    let client = Rect::from_min_size(outer.min + vec2(BORDER, CAPTION), client_size);

    // block the dialog underneath
    let _ = ui.interact(outer, ui.scope_id().with("msgbox-block"), Sense::click());
    let title: String = text.chars().take(40).collect();
    paint_window(ui.painter(), outer, client, &title, blur);
    let close = close_button(ui, outer, "msgbox-close").clicked();
    let ok = message_client(ui, client, text, look, icon);
    (outer, close || ok)
}

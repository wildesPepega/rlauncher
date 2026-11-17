//! The "Run dialog" layout: a Windows 7 style Run dialog (Aero glass frame, combo box,
//! OK / Cancel / Browse… buttons). The run history is shown as the combo box's drop-down.
//!
//! Geometry follows ExecBin's executedlg.ui (a QtWidgets recreation of the dialog):
//! a 397×175 client area with the icon at (18,22), the text at (72,6), the combo box at
//! (72,78) and the buttons at y = 137. Everything is painted here; no Microsoft
//! resources are used.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use crate::apps::App;
use crate::config::{Config, RunPosition, Shortcut};
use crate::run;
use crate::ui::{NavKey, UiEvent};

/// Font family registered by the launcher (Segoe UI when installed).
pub const FONT: &str = "win7";

const CLIENT: Vec2 = vec2(397.0, 175.0);
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
            } else if i >= self.scroll + MAX_ROWS {
                self.scroll = i + 1 - MAX_ROWS;
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
                    let step = if matches!(key, NavKey::PageDown) { MAX_ROWS } else { 1 };
                    self.select(Some(self.selected.map_or(0, |i| (i + step).min(n - 1))));
                }
            }
            NavKey::Up | NavKey::PageUp if self.dropdown => {
                let step = if matches!(key, NavKey::PageUp) { MAX_ROWS } else { 1 };
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

    /// Outer rect of the dialog (glass frame included).
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

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        config: &Config,
        apps: &[App],
        shortcuts: &[Shortcut],
        events: &mut Vec<UiEvent>,
    ) {
        let screen = ctx.content_rect();
        let outer = Self::outer_rect(config, screen);
        let client = Rect::from_min_size(outer.min + vec2(BORDER, CAPTION), CLIENT);
        let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(client.min + vec2(x, y), vec2(w, h));
        self.glass = vec![outer];
        let mut hit_rects = vec![outer];

        egui::Area::new(egui::Id::unique("rlauncher-run"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let v = ui.visuals_mut();
                v.selection.bg_fill = HIGHLIGHT;
                v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
                v.text_cursor.stroke = Stroke::new(1.0, TEXT);
                v.override_text_color = Some(TEXT);
                let blocked = self.error.is_some();

                // ---- frame
                paint_window(ui.painter(), outer, client, "Run", self.blur_available);
                if close_button(ui, outer, "run-close").clicked() && !blocked {
                    events.push(UiEvent::Hide);
                }

                // ---- client area
                let p = ui.painter();
                let strip_top = client.top() + 126.0;
                p.rect_filled(Rect::from_min_max(client.min, pos2(client.right(), strip_top)), 0.0, Color32::WHITE);
                p.rect_filled(Rect::from_min_max(pos2(client.left(), strip_top), client.max), 0.0, STRIP);
                p.hline(client.x_range(), strip_top + 0.5, Stroke::new(1.0, Color32::from_rgb(0xdf, 0xdf, 0xdf)));

                let icon = at(18.0, 22.0, 32.0, 32.0);
                paint_run_icon(p, icon);
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
                    TEXT,
                    desc.width(),
                );
                let pos = pos2(desc.left(), desc.center().y - galley.size().y / 2.0);
                ui.painter().galley(pos, galley, TEXT);

                let label = at(12.0, 72.0, 42.0, 33.0);
                let g = ui.painter().layout_job(mnemonic("Open:", 0));
                ui.painter().galley(pos2(label.left(), label.center().y - g.size().y / 2.0 + 1.0), g, TEXT);

                // ---- combo box
                let combo = at(72.0, 78.0, 308.0, 24.0);
                let arrow = Rect::from_min_max(pos2(combo.right() - 18.0, combo.top() + 1.0), combo.max - vec2(1.0, 1.0));
                let arrow_resp = ui.interact(arrow, ui.scope_id().with("run-arrow"), Sense::click());
                paint_combo(ui.painter(), combo, arrow, arrow_resp.hovered(), self.dropdown);
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
                    .text_color(TEXT)
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
                if button(ui, at(110.0, 137.0, 88.0, 24.0), "OK", None, enabled, true).clicked() && !blocked {
                    self.run(apps, shortcuts, events);
                }
                if button(ui, at(206.0, 137.0, 86.0, 24.0), "Cancel", None, true, false).clicked() && !blocked {
                    events.push(UiEvent::Hide);
                }
                if button(ui, at(300.0, 137.0, 88.0, 24.0), "Browse...", Some(0), true, false).clicked() && !blocked {
                    events.push(UiEvent::Browse);
                }

                // ---- drop-down with the run history
                if self.dropdown && !self.list.is_empty() {
                    let rect = self.dropdown_ui(ui, combo, screen);
                    hit_rects.push(rect);
                }

                // ---- error message box
                if let Some(text) = self.error.clone() {
                    let (rect, close) = message_box(ui, outer, screen, &text, self.blur_available);
                    self.glass.push(rect);
                    hit_rects.push(rect);
                    if close {
                        self.error = None;
                        self.select_all = true;
                    }
                }
            });

        // Clicking anywhere outside of the dialog hides it.
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

    fn dropdown_ui(&mut self, ui: &mut egui::Ui, combo: Rect, screen: Rect) -> Rect {
        let rows = self.list.len().min(MAX_ROWS);
        let h = rows as f32 * ROW_H + 2.0;
        let below = combo.bottom() + h <= screen.bottom();
        let rect = if below {
            Rect::from_min_size(pos2(combo.left(), combo.bottom()), vec2(combo.width(), h))
        } else {
            Rect::from_min_size(pos2(combo.left(), combo.top() - h), vec2(combo.width(), h))
        };
        let p = ui.painter();
        p.add(egui::Shadow { offset: [2, 2], blur: 6, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(rect, 0));
        p.rect_filled(rect, 0.0, Color32::WHITE);
        p.rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_rgb(0x64, 0x64, 0x64)), StrokeKind::Inside);

        let resp = ui.interact(rect, ui.scope_id().with("run-dropdown"), Sense::click());
        let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
        let moved = pointer.is_some() && pointer != self.last_pointer && self.last_pointer.is_some();
        if pointer.is_some() {
            self.last_pointer = pointer;
        }
        let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if resp.hovered() && scroll.abs() > 0.5 {
            let max = self.list.len().saturating_sub(MAX_ROWS);
            self.scroll = if scroll < 0.0 { (self.scroll + 1).min(max) } else { self.scroll.saturating_sub(1) };
        }

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
            if sel {
                ui.painter().rect_filled(r, 0.0, HIGHLIGHT);
            }
            let color = if sel { Color32::WHITE } else { TEXT };
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

/// Text with one underlined character (keyboard mnemonic, like "_Open:").
fn mnemonic(text: &str, underline: usize) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (i, c) in text.chars().enumerate() {
        let fmt = TextFormat {
            font_id: font(),
            color: TEXT,
            underline: if i == underline { Stroke::new(1.0, TEXT) } else { Stroke::NONE },
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
    let mut job = match underline {
        Some(i) => mnemonic(label, i),
        None => LayoutJob::simple_singleline(label.into(), font(), color),
    };
    for s in &mut job.sections {
        s.format.color = color;
    }
    let g = p.layout_job(job);
    p.galley(rect.center() - g.size() / 2.0, g, color);
    resp
}

/// The "cannot find" error message box, centered over the dialog. Returns its outer rect
/// and whether it was dismissed.
fn message_box(ui: &mut egui::Ui, owner: Rect, screen: Rect, text: &str, blur: bool) -> (Rect, bool) {
    let msg = format!(
        "rlauncher cannot find '{text}'. Make sure you typed the name correctly, and then try again."
    );
    let text_w = 290.0;
    let galley = ui.painter().layout(msg, font(), TEXT, text_w);
    let body_h = (galley.size().y.max(32.0) + 2.0 * 26.0).round();
    let client_size = vec2(text_w + 74.0 + 24.0, body_h + 41.0);
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
    let mut close = close_button(ui, outer, "msgbox-close").clicked();
    let p = ui.painter();
    let strip_top = client.bottom() - 41.0;
    p.rect_filled(Rect::from_min_max(client.min, pos2(client.right(), strip_top)), 0.0, Color32::WHITE);
    p.rect_filled(Rect::from_min_max(pos2(client.left(), strip_top), client.max), 0.0, STRIP);
    p.hline(client.x_range(), strip_top + 0.5, Stroke::new(1.0, rgba(0xdf, 0xdf, 0xdf, 255)));

    // error icon: red circle with a white cross
    let icon_c = client.min + vec2(26.0 + 16.0, 26.0 + 16.0);
    p.add(egui::Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(80) }.as_shape(Rect::from_center_size(icon_c, Vec2::splat(30.0)), 15));
    gradient_rect(
        p,
        Rect::from_center_size(icon_c, Vec2::splat(30.0)),
        15.0,
        &[(0.0, rgba(0xf6, 0x8d, 0x7b, 255)), (0.5, rgba(0xe0, 0x30, 0x1e, 255)), (1.0, rgba(0xa9, 0x10, 0x05, 255))],
    );
    p.circle_stroke(icon_c, 14.5, Stroke::new(1.0, rgba(0x8a, 0x10, 0x05, 255)));
    let d = 6.0;
    let w = Stroke::new(3.0, Color32::WHITE);
    p.line_segment([icon_c + vec2(-d, -d), icon_c + vec2(d, d)], w);
    p.line_segment([icon_c + vec2(-d, d), icon_c + vec2(d, -d)], w);

    let tpos = pos2(client.left() + 74.0, client.top() + 26.0 + (32.0 - galley.size().y).max(0.0) / 2.0);
    p.galley(tpos, galley, TEXT);
    let ok = Rect::from_min_size(pos2(client.right() - 11.0 - 75.0, strip_top + 9.0), vec2(75.0, 23.0));
    if button(ui, ok, "OK", None, true, true).clicked() {
        close = true;
    }
    (outer, close)
}

//! The preferences page (opened with the gear button in the input row).

use egui::{Color32, FontId, RichText, Stroke, vec2};

use crate::config::{self, Config, Shortcut};
use crate::hotkey::{self, HotkeyStatus};
use crate::theme::{Theme, ThemeInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Hotkey,
    Queries,
    Appearance,
}

/// Things the settings page asks the launcher to do.
#[derive(Debug)]
pub enum SettingsAction {
    Close,
    StartRecording,
    CancelRecording,
    /// Apply the hotkey (after recording, or to take it over from another action).
    ApplyHotkey { force: bool },
    OpenSystemShortcuts,
    ThemeChanged,
}

pub struct Settings {
    tab: Tab,
    pub config: Config,
    pub shortcuts: Vec<Shortcut>,
    autostart: bool,
    selected: Option<usize>,
    pub recording: bool,
    themes: Vec<ThemeInfo>,
    /// Loaded lazily when the appearance tab is shown.
    previews: Option<Vec<Theme>>,
    new_theme_dir: String,
    original_config: Config,
    original_autostart: bool,
}

impl Settings {
    pub fn new(config: Config, shortcuts: Vec<Shortcut>) -> Settings {
        let autostart = config::autostart_enabled();
        Settings {
            tab: Tab::General,
            selected: if shortcuts.is_empty() { None } else { Some(0) },
            original_config: config.clone(),
            original_autostart: autostart,
            config,
            shortcuts,
            autostart,
            recording: false,
            themes: crate::theme::list_themes(),
            previews: None,
            new_theme_dir: String::new(),
        }
    }

    /// Selects a tab by name ("general", "hotkey", "queries", "appearance").
    pub fn open_tab(&mut self, name: &str) {
        self.tab = match name {
            "hotkey" => Tab::Hotkey,
            "queries" => Tab::Queries,
            "appearance" => Tab::Appearance,
            _ => Tab::General,
        };
    }

    /// Writes everything to disk.
    pub fn save(&mut self) {
        // Nameless queries are named after their keyword; empty ones are dropped.
        self.shortcuts.retain(|s| !(s.keyword.trim().is_empty() && s.cmd.trim().is_empty()));
        for s in &mut self.shortcuts {
            s.keyword = s.keyword.trim().to_string();
            if s.name.trim().is_empty() {
                s.name = s.keyword.clone();
            }
        }
        if let Err(e) = config::save_shortcuts(&self.shortcuts) {
            log::error!("cannot save queries: {e}");
        }
        if let Err(e) = self.config.save() {
            log::error!("cannot save config: {e}");
        }
        if self.autostart != self.original_autostart {
            if let Err(e) = config::set_autostart(self.autostart) {
                log::error!("cannot change autostart: {e}");
            }
            self.original_autostart = self.autostart;
        }
        self.original_config = self.config.clone();
    }

    /// Called with the trigger of a key combination pressed while recording.
    pub fn recorded(&mut self, trigger: String) {
        self.recording = false;
        self.config.hotkey = trigger;
        self.config.hotkey_enabled = true;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, t: &Theme, status: &HotkeyStatus, height: f32) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.spacing_mut().button_padding = vec2(10.0, 5.0);
        ui.spacing_mut().interact_size.y = 26.0;
        ui.spacing_mut().slider_width = 260.0;

        // Header: title, tabs, close button
        ui.horizontal(|ui| {
            ui.label(RichText::new("Einstellungen").font(FontId::proportional(t.name_size * 1.1)).color(t.item_name));
            ui.add_space(16.0);
            for (tab, label) in [
                (Tab::General, "Allgemein"),
                (Tab::Hotkey, "Hotkey"),
                (Tab::Queries, "Queries"),
                (Tab::Appearance, "Aussehen"),
            ] {
                let text = RichText::new(label).size(t.descr_size * 1.25);
                if ui.selectable_label(self.tab == tab, text).clicked() {
                    if self.recording {
                        self.recording = false;
                        actions.push(SettingsAction::CancelRecording);
                    }
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Fertig").size(t.descr_size * 1.25)).clicked() {
                    actions.push(SettingsAction::Close);
                }
            });
        });
        ui.separator();

        let body_height = (height - 60.0).max(150.0);
        egui::ScrollArea::vertical()
            .id_salt(("settings", self.tab as u8))
            .max_height(body_height)
            .min_scrolled_height(body_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(4.0);
                match self.tab {
                    Tab::General => self.general_ui(ui, t),
                    Tab::Hotkey => self.hotkey_ui(ui, t, status, &mut actions),
                    Tab::Queries => self.queries_ui(ui, t),
                    Tab::Appearance => self.appearance_ui(ui, t, &mut actions),
                }
            });
        actions
    }

    fn general_ui(&mut self, ui: &mut egui::Ui, t: &Theme) {
        let c = &mut self.config;
        section(ui, t, "Start");
        ui.checkbox(&mut self.autostart, "Beim Login automatisch starten");
        ui.checkbox(&mut c.close_on_focus_out, "Schließen, wenn das Fenster den Fokus verliert oder daneben geklickt wird");

        section(ui, t, "Suche");
        ui.checkbox(&mut c.calculator, "Rechner (z. B. 2*(3+4) oder sqrt(2))");
        ui.checkbox(&mut c.file_browser, "Dateibrowser für Eingaben, die mit / oder ~ beginnen");
        egui::Grid::new("general-grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label("Sichtbare Ergebnisse");
            ui.add(egui::Slider::new(&mut c.max_results, 3..=15));
            ui.end_row();
            ui.label("Schnellwahl-Tasten (Alt+…)");
            ui.add(egui::TextEdit::singleline(&mut c.jump_keys).desired_width(320.0));
            ui.end_row();
            ui.label("Platzhaltertext");
            ui.add(egui::TextEdit::singleline(&mut c.placeholder).desired_width(320.0));
            ui.end_row();
            ui.label("Terminal für Terminal-Apps");
            ui.add(egui::TextEdit::singleline(&mut c.terminal).hint_text("automatisch (kitty, foot, alacritty, …)").desired_width(320.0));
            ui.end_row();
        });

        section(ui, t, "Fenster");
        egui::Grid::new("window-grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label("Breite");
            ui.add(egui::Slider::new(&mut c.width, 400.0..=1400.0).step_by(10.0).suffix(" px"));
            ui.end_row();
            ui.label("Abstand von oben");
            let mut pct = c.top_offset * 100.0;
            if ui.add(egui::Slider::new(&mut pct, 5.0..=60.0).step_by(1.0).suffix(" %")).changed() {
                c.top_offset = pct / 100.0;
            }
            ui.end_row();
        });
    }

    fn hotkey_ui(&mut self, ui: &mut egui::Ui, t: &Theme, status: &HotkeyStatus, actions: &mut Vec<SettingsAction>) {
        section(ui, t, "Globales Tastenkürzel");
        ui.horizontal(|ui| {
            ui.label("Aktiv:");
            let active = status.active.clone().unwrap_or_else(|| "keins".into());
            ui.label(RichText::new(active).strong().size(t.name_size).color(t.hl_selected));
        });
        ui.horizontal(|ui| {
            ui.label("Gewünscht:");
            ui.label(RichText::new(hotkey::display(&self.config.hotkey)).strong());
        });

        if self.recording {
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(t.item_box_selected)
                .corner_radius(8)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("Drücke jetzt die gewünschte Tastenkombination …")
                            .size(t.name_size)
                            .color(t.item_name_selected),
                    );
                    ui.label(RichText::new("Esc bricht ab").color(t.item_text_selected));
                });
        } else {
            ui.horizontal(|ui| {
                if ui.add_enabled(status.can_set || status.backend.contains("Portal"), egui::Button::new("Neues Tastenkürzel aufnehmen …")).clicked() {
                    self.recording = true;
                    actions.push(SettingsAction::StartRecording);
                }
                if ui.button("Systemeinstellungen öffnen").clicked() {
                    actions.push(SettingsAction::OpenSystemShortcuts);
                }
            });
        }

        if let Some(conflict) = &status.conflict {
            ui.add_space(4.0);
            ui.colored_label(
                Color32::from_rgb(0xf3, 0x8b, 0xa8),
                format!("{} ist bereits belegt von: {conflict}", hotkey::display(&self.config.hotkey)),
            );
            if ui.button(format!("Trotzdem verwenden (bei „{conflict}“ entfernen)")).clicked() {
                actions.push(SettingsAction::ApplyHotkey { force: true });
            }
        }
        if let Some(err) = &status.error {
            ui.colored_label(Color32::from_rgb(0xf3, 0x8b, 0xa8), err);
        }

        let before = self.config.hotkey_enabled;
        ui.checkbox(&mut self.config.hotkey_enabled, "Globales Tastenkürzel registrieren");
        if before != self.config.hotkey_enabled {
            actions.push(SettingsAction::ApplyHotkey { force: false });
        }

        section(ui, t, "Info");
        ui.label(RichText::new(format!("Backend: {}", status.backend)).color(t.item_text));
        ui.label(
            RichText::new(
                "Alternativ kann der Befehl `rlauncher-toggle` in jedem Compositor auf eine Taste gelegt werden \
                 (Sway, Hyprland, niri, …). Er startet rlauncher auch, falls es noch nicht läuft.",
            )
            .color(t.item_text),
        );
    }

    fn queries_ui(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(
            RichText::new(
                "Queries werden über ihr Keyword aufgerufen, z. B. „re“ findet „reboot“. \
                 Mit Leerzeichen nach dem Keyword wird der Rest als Argument übergeben.",
            )
            .color(t.item_text),
        );
        ui.add_space(4.0);
        let full = ui.available_width();
        let list_w = 230.0_f32.min(full * 0.35);
        ui.horizontal_top(|ui| {
            // list
            ui.vertical(|ui| {
                ui.set_width(list_w);
                if ui.button("+  Neue Query").clicked() {
                    self.shortcuts.push(Shortcut {
                        id: config::new_id(),
                        name: String::new(),
                        keyword: String::new(),
                        cmd: String::new(),
                        icon: None,
                        is_default_search: false,
                        run_without_argument: false,
                        added: config::now_secs(),
                    });
                    self.selected = Some(self.shortcuts.len() - 1);
                }
                ui.add_space(4.0);
                for (i, s) in self.shortcuts.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(list_w, 44.0), egui::Sense::click());
                    let selected = self.selected == Some(i);
                    if selected || resp.hovered() {
                        let fill = if selected { t.item_box_selected } else { t.item_box_selected.gamma_multiply(0.5) };
                        ui.painter().rect_filled(rect, t.item_radius.min(10.0), fill);
                    }
                    let name = if s.name.is_empty() { "(ohne Namen)" } else { s.name.as_str() };
                    let (name_c, kw_c) = if selected {
                        (t.item_name_selected, t.item_text_selected)
                    } else {
                        (t.item_name, t.item_text)
                    };
                    let left = rect.left() + 10.0;
                    ui.painter().text(
                        egui::pos2(left, rect.top() + 6.0),
                        egui::Align2::LEFT_TOP,
                        name,
                        FontId::proportional(t.descr_size * 1.2),
                        name_c,
                    );
                    let kw = if s.keyword.is_empty() { "kein Keyword".to_string() } else { format!("Keyword: {}", s.keyword) };
                    ui.painter().text(
                        egui::pos2(left, rect.bottom() - 6.0),
                        egui::Align2::LEFT_BOTTOM,
                        kw,
                        FontId::proportional(t.descr_size),
                        kw_c,
                    );
                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                }
            });
            ui.separator();
            // editor
            ui.vertical(|ui| {
                let Some(idx) = self.selected.filter(|i| *i < self.shortcuts.len()) else {
                    ui.label(RichText::new("Wähle links eine Query aus oder lege eine neue an.").color(t.item_text));
                    return;
                };
                let duplicate = {
                    let kw = self.shortcuts[idx].keyword.trim();
                    !kw.is_empty() && self.shortcuts.iter().enumerate().any(|(i, s)| i != idx && s.keyword.trim() == kw)
                };
                let s = &mut self.shortcuts[idx];
                let field_w = ui.available_width() - 8.0;
                egui::Grid::new("query-grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut s.name).hint_text("z. B. Neustart").desired_width(field_w - 90.0));
                    ui.end_row();
                    ui.label("Keyword");
                    ui.add(egui::TextEdit::singleline(&mut s.keyword).hint_text("z. B. reboot").desired_width(field_w - 90.0));
                    ui.end_row();
                    ui.label("Icon");
                    let mut icon = s.icon.clone().unwrap_or_default();
                    if ui
                        .add(egui::TextEdit::singleline(&mut icon).hint_text("Icon-Name oder Pfad (optional)").desired_width(field_w - 90.0))
                        .changed()
                    {
                        s.icon = (!icon.trim().is_empty()).then_some(icon);
                    }
                    ui.end_row();
                });
                if duplicate {
                    ui.colored_label(Color32::from_rgb(0xf3, 0x8b, 0xa8), "Dieses Keyword wird schon von einer anderen Query verwendet.");
                }
                ui.label("Befehl, Script oder URL");
                ui.add(
                    egui::TextEdit::multiline(&mut s.cmd)
                        .code_editor()
                        .hint_text("systemctl reboot\n\n#!/bin/sh\nnotify-send \"$1\"\n\nhttps://google.com/search?q=%s")
                        .desired_rows(5)
                        .desired_width(field_w),
                );
                ui.checkbox(&mut s.run_without_argument, "Ohne Argument ausführen (Enter führt sofort aus)");
                ui.checkbox(&mut s.is_default_search, "Als Standardsuche anbieten (Fallback bei jeder Suche)");
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "%s wird durch das Argument ersetzt. Beginnt der Befehl mit #!, wird er als Script ausgeführt \
                         und das Argument angehängt ($1, $2 …). http(s)://-Adressen werden im Browser geöffnet.",
                    )
                    .size(t.descr_size)
                    .color(t.item_text),
                );
                ui.add_space(6.0);
                if ui.button(RichText::new("Query löschen").color(Color32::from_rgb(0xf3, 0x8b, 0xa8))).clicked() {
                    self.shortcuts.remove(idx);
                    self.selected = if self.shortcuts.is_empty() { None } else { Some(idx.saturating_sub(1)) };
                }
            });
        });
    }

    fn appearance_ui(&mut self, ui: &mut egui::Ui, t: &Theme, actions: &mut Vec<SettingsAction>) {
        section(ui, t, "Theme");
        if self.previews.is_none() {
            self.previews = Some(self.themes.iter().map(|th| Theme::load(&th.name)).collect());
        }
        // Cards with a small preview of each theme
        let card = vec2(196.0, 92.0);
        let per_row = ((ui.available_width() + 10.0) / (card.x + 10.0)).floor().max(1.0) as usize;
        let previews = self.previews.clone().unwrap_or_default();
        for (row_themes, row_previews) in self.themes.chunks(per_row).zip(previews.chunks(per_row)) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for (info, preview) in row_themes.iter().zip(row_previews) {
                    let selected = info.name == self.config.theme;
                    if theme_card(ui, card, info, preview, selected, t).clicked() && !selected {
                        self.config.theme = info.name.clone();
                        actions.push(SettingsAction::ThemeChanged);
                    }
                }
            });
        }

        section(ui, t, "Theme-Ordner");
        ui.label(
            RichText::new(
                "Themes im Ulauncher-Format (Ordner mit manifest.json + CSS). Ein Ordner kann mehrere Themes \
                 enthalten oder selbst ein Theme sein.",
            )
            .color(t.item_text),
        );
        let extra_count = self.config.theme_dirs.len();
        let mut remove = None;
        for (i, dir) in crate::theme::theme_dirs().iter().enumerate() {
            let exists = dir.is_dir();
            let configured = i <= extra_count;
            // system locations that don't exist on this machine are just noise
            if !exists && !configured {
                continue;
            }
            ui.horizontal(|ui| {
                let color = if exists { t.item_name } else { t.item_text.gamma_multiply(0.6) };
                let mut text = RichText::new(dir.display().to_string()).monospace().color(color);
                if !exists {
                    text = text.italics();
                }
                ui.add(egui::Label::new(text).truncate());
                if !exists {
                    ui.label(RichText::new("(nicht vorhanden)").color(t.item_text.gamma_multiply(0.6)));
                } else if ui.small_button("Öffnen").clicked() {
                    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
                }
                // index 0 is rlauncher's own folder, then the configured ones
                if (1..=extra_count).contains(&i) && ui.small_button("Entfernen").clicked() {
                    remove = Some(i - 1);
                }
            });
        }
        if let Some(i) = remove {
            self.config.theme_dirs.remove(i);
            self.dirs_changed();
        }
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.new_theme_dir)
                    .hint_text("weiterer Ordner, z. B. ~/themes/ulauncher")
                    .desired_width(360.0),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Hinzufügen").clicked() || enter) && !self.new_theme_dir.trim().is_empty() {
                self.config.theme_dirs.push(self.new_theme_dir.trim().to_string());
                self.new_theme_dir.clear();
                self.dirs_changed();
            }
            if ui.button("Neu einlesen").clicked() {
                self.dirs_changed();
            }
        });

        section(ui, t, "Darstellung");
        if ui.checkbox(&mut self.config.shadow, "Schatten um das Fenster").changed() {
            actions.push(SettingsAction::ThemeChanged);
        }
        ui.horizontal(|ui| {
            ui.label("Schriftart (Pfad zu .ttf/.otf)");
            ui.add(egui::TextEdit::singleline(&mut self.config.font).hint_text("System-Standard").desired_width(320.0));
        });
    }

    fn dirs_changed(&mut self) {
        crate::theme::set_extra_dirs(&self.config.theme_dirs);
        self.themes = crate::theme::list_themes();
        self.previews = None;
    }
}

/// A clickable miniature of a theme: window, input line and a selected result.
fn theme_card(ui: &mut egui::Ui, size: egui::Vec2, info: &ThemeInfo, p: &Theme, selected: bool, t: &Theme) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size + vec2(0.0, 22.0), egui::Sense::click());
    let painter = ui.painter();
    let win = egui::Rect::from_min_size(rect.min, size);
    let radius = p.window_radius.clamp(2.0, 12.0);
    painter.rect_filled(win, radius, p.window_bg);
    let outline = if selected {
        Stroke::new(2.5, t.hl_selected)
    } else if resp.hovered() {
        Stroke::new(1.5, t.hl_selected.gamma_multiply(0.6))
    } else {
        Stroke::new(1.0, p.window_border)
    };
    painter.rect_stroke(win, radius, outline, egui::StrokeKind::Inside);

    let m = 10.0;
    painter.text(win.min + vec2(m, 8.0), egui::Align2::LEFT_TOP, "fire", FontId::proportional(15.0), p.input_color);
    let sel = egui::Rect::from_min_size(win.min + vec2(6.0, 34.0), vec2(size.x - 12.0, 24.0));
    painter.rect_filled(sel, p.item_radius.min(8.0), p.item_box_selected);
    let pos = sel.left_center() + vec2(8.0, 0.0);
    let g = painter.layout_no_wrap("Fire".into(), FontId::proportional(12.0), p.hl_selected);
    let w = g.size().x;
    painter.galley(pos - vec2(0.0, g.size().y / 2.0), g, p.hl_selected);
    painter.text(pos + vec2(w, 0.0), egui::Align2::LEFT_CENTER, "fox", FontId::proportional(12.0), p.item_name_selected);
    painter.text(sel.right_center() - vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER, "Alt+1", FontId::proportional(10.0), p.item_shortcut_selected);
    painter.text(
        win.min + vec2(14.0, 70.0),
        egui::Align2::LEFT_CENTER,
        "Files",
        FontId::proportional(12.0),
        p.item_name,
    );

    let label_color = if selected { t.hl_selected } else { t.item_name };
    let label = painter.layout(
        info.display_name.clone(),
        FontId::proportional(t.descr_size),
        label_color,
        size.x,
    );
    painter.galley(egui::pos2(rect.left() + 2.0, win.bottom() + 4.0), label, label_color);
    let source = if info.dir.as_ref().is_some_and(|d| d.to_string_lossy().contains("ulauncher")) {
        "Ulauncher-Theme"
    } else if info.dir.is_none() {
        "eingebaut"
    } else {
        "rlauncher-Theme"
    };
    resp.on_hover_text(format!(
        "{source}\n{}",
        info.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "builtin".into())
    ))
}

fn section(ui: &mut egui::Ui, t: &Theme, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().size(t.descr_size * 1.3).color(t.hl_not_selected));
    let r = ui.available_rect_before_wrap();
    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, t.item_text.gamma_multiply(0.2)));
    ui.add_space(4.0);
}

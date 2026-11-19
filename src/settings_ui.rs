//! The Settings screen: a search box, tabs, cards of settings with "Reset to defaults", and (on the Terminal tab) a preview
//! of the terminal that follows every change.
//!
//! The rows are data (`TERMINAL`, `GENERAL`), so the search can look through them without drawing anything.

use crate::app::{find_scheme, App};
use crate::icons::{self, Icon};
use crate::model::*;
use crate::schemes::{Scheme, SCHEMES};
use crate::term::{cell_size, paint_cursor, rgb};
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, FontId, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

struct Row {
    key: &'static str,
    title: &'static str,
    desc: &'static str,
    /// More words the search should find this row by.
    also: &'static str,
}

struct Group {
    title: &'static str,
    rows: &'static [Row],
}

const META_TITLE: &str = if cfg!(target_os = "macos") { "Use Option as Meta key" } else { "Use Alt as Meta key" };
const ZOOM_HINT: &str = if cfg!(target_os = "macos") { "Cmd + and Cmd − also work inside a terminal" } else { "Ctrl + and Ctrl − also work inside a terminal" };

const TERMINAL: &[Group] = &[
    Group {
        title: "Appearance",
        rows: &[
            Row { key: "scheme", title: "Color scheme", desc: "Used by every terminal unless a host picks its own", also: "colour theme palette" },
            Row { key: "font", title: "Font", desc: "Monospace fonts installed on this computer", also: "family typeface" },
            Row { key: "size", title: "Font size", desc: ZOOM_HINT, also: "zoom bigger smaller" },
            Row { key: "lineh", title: "Line height", desc: "Room between terminal rows", also: "spacing" },
            Row { key: "cursor", title: "Cursor", desc: "Its shape, and whether it blinks", also: "blink block bar underline" },
            Row { key: "bold", title: "Brighten bold text", desc: "Draw bold text in the bright colors", also: "" },
        ],
    },
    Group {
        title: "Behavior",
        rows: &[
            Row { key: "scroll", title: "Scrollback", desc: "Lines kept for scrolling back", also: "buffer" },
            Row { key: "term", title: "Terminal type", desc: "Value of TERM sent to the server", also: "xterm" },
            Row { key: "rclick", title: "Right click to paste", desc: "Otherwise right click opens a menu", also: "mouse" },
            Row { key: "meta", title: META_TITLE, desc: "The key sends Esc before the key pressed with it", also: "alt option keyboard" },
            Row { key: "bell", title: "Bell", desc: "What happens when a program rings the bell", also: "sound flash beep" },
            Row { key: "card", title: "Session card", desc: "Where the card with Snippets, Share and Disconnect sits on a terminal. Hidden still shows it when the connection drops", also: "position corner" },
        ],
    },
    Group {
        title: "Autocomplete & history",
        rows: &[
            Row { key: "ac", title: "Autocomplete", desc: "Suggest commands under the cursor from this host's history, snippets and common commands", also: "suggestions" },
            Row { key: "rhist", title: "Read host history", desc: "On the first connection to a host, also read its own shell history (bash, zsh, fish). Commands that look like they hold secrets are skipped", also: "remote" },
            Row { key: "chist", title: "Command history", desc: "Commands remembered for autocomplete, stored only on this computer", also: "clear" },
        ],
    },
];

const GENERAL: &[Group] = &[
    Group { title: "Appearance", rows: &[
        Row { key: "apptheme", title: "App theme", desc: "Colors of the app itself. Terminals keep their own color scheme", also: "dark light mode system look" },
        Row { key: "uifont", title: "Interface font", desc: "The font of menus, labels and buttons. Terminals have their own font", also: "typeface text inter dm sans system" },
    ] },
    Group { title: "Profile", rows: &[Row { key: "name", title: "Your name", desc: "Shown to the others in a shared terminal", also: "profile collaboration avatar" }] },
    Group {
        title: "Connection",
        rows: &[
            Row { key: "keepalive", title: "Keep-alive interval", desc: "Seconds between keep-alive packets (0 = off)", also: "timeout" },
            Row { key: "reconnect", title: "Auto-reconnect", desc: "Reconnect when a long-running SSH link drops", also: "" },
        ],
    },
    Group { title: "SFTP", rows: &[Row { key: "editor", title: "Default editor", desc: "Used by Open in the SFTP file browser", also: "files" }] },
    Group { title: "Hosts", rows: &[Row { key: "sort", title: "Sort hosts by", desc: "Order of the hosts on the Hosts screen", also: "order" }] },
    Group { title: "Data", rows: &[Row { key: "data", title: "Data folder", desc: "", also: "storage location path" }] },
];

const ANSI: [&str; 16] = [
    "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White", "Bright black", "Bright red", "Bright green", "Bright yellow", "Bright blue", "Bright magenta", "Bright cyan", "Bright white",
];

const TERM_TYPES: [&str; 6] = ["xterm-256color", "xterm", "screen-256color", "tmux-256color", "linux", "vt100"];

/// Does `q` (lower case, words in any order) match the row?
fn hit(q: &str, g: &Group, r: &Row) -> bool {
    let hay = format!("{} {} {} {}", g.title, r.title, r.desc, r.also).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
}

/// Puts the setting behind `key` back to its default. Keys that are not settings do nothing.
fn reset(s: &mut Settings, key: &str) {
    let d = Settings::default();
    match key {
        "apptheme" => s.app_theme = d.app_theme,
        "uifont" => s.ui_font = d.ui_font,
        "scheme" => s.color_scheme = d.color_scheme,
        "font" => s.font_family = d.font_family,
        "size" => s.font_size = d.font_size,
        "lineh" => s.line_height = d.line_height,
        "cursor" => {
            s.cursor_shape = d.cursor_shape;
            s.cursor_blink = d.cursor_blink;
        }
        "bold" => s.brighten_bold_text = d.brighten_bold_text,
        "scroll" => s.scrollback = d.scrollback,
        "term" => s.terminal_type = d.terminal_type,
        "rclick" => s.right_click_to_paste = d.right_click_to_paste,
        "meta" => s.option_is_meta = d.option_is_meta,
        "bell" => {
            s.bell = d.bell;
            s.bell_sound = d.bell_sound;
        }
        "card" => s.card_position = d.card_position,
        "ac" => s.autocomplete = d.autocomplete,
        "rhist" => s.remote_history = d.remote_history,
        "keepalive" => s.keep_alive = d.keep_alive,
        "reconnect" => s.autoreconnect = d.autoreconnect,
        "editor" => s.default_editor = d.default_editor,
        "sort" => s.sort_by = d.sort_by,
        _ => {}
    }
}

/// True when some setting of the group is not at its default (so "Reset to defaults" has something to do).
fn differs(s: &Settings, g: &Group) -> bool {
    let mut d = s.clone();
    for r in g.rows {
        reset(&mut d, r.key);
    }
    serde_json::to_value(&d).ok() != serde_json::to_value(s).ok()
}

fn resettable(g: &Group) -> bool {
    g.rows.iter().any(|r| !matches!(r.key, "chist" | "data" | "name"))
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a row needs besides the settings themselves.
struct RowCtx<'a> {
    s: &'a mut Settings,
    changed: bool,
    hist_count: usize,
    clear_armed: &'a mut bool,
    clear_now: bool,
    /// Hosts that pick their own color scheme.
    own_scheme: &'a [String],
    data_path: &'a str,
}

/// A row of buttons, one of them chosen.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = FontId::proportional(12.5);
    let widths: Vec<f32> = options.iter().map(|(_, t)| ui.painter().layout_no_wrap(t.to_string(), font.clone(), TEXT()).size().x + 26.0).collect();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(widths.iter().sum::<f32>() + 6.0, 32.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, 8.0, BG_WINDOW(), Stroke::new(1.0, BORDER()), StrokeKind::Inside);
    let mut x = rect.left() + 3.0;
    let mut changed = false;
    for ((v, text), w) in options.iter().zip(widths) {
        let r = Rect::from_min_size(Pos2::new(x, rect.top() + 3.0), Vec2::new(w, rect.height() - 6.0));
        let resp = ui.interact(r, ui.id().with(("seg", text)), Sense::click());
        let on = *value == *v;
        if on {
            ui.painter().rect_filled(r, 6.0, CARD_HOVER());
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, CARD());
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *text, font.clone(), if on || resp.hovered() { TEXT() } else { MUTED() });
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
            *value = *v;
            changed = true;
        }
        x += w;
    }
    changed
}

/// "−  13 px  +"
fn stepper(ui: &mut egui::Ui, value: &mut f32, min: f32, max: f32) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(128.0, 32.0), Sense::hover());
    ui.painter().rect(rect, 8.0, BG_WINDOW(), Stroke::new(1.0, BORDER()), StrokeKind::Inside);
    let mut changed = false;
    for (i, sign) in [(0, -1.0), (1, 1.0)] {
        let r = if i == 0 { Rect::from_min_size(rect.min, Vec2::new(34.0, rect.height())) } else { Rect::from_min_size(Pos2::new(rect.right() - 34.0, rect.top()), Vec2::new(34.0, rect.height())) };
        let resp = ui.interact(r.shrink(3.0), ui.id().with(("step", i)), Sense::click());
        let can = if sign < 0.0 { *value > min } else { *value < max };
        if resp.hovered() && can {
            ui.painter().rect_filled(r.shrink(3.0), 6.0, CARD());
        }
        let col = if !can { BORDER() } else if resp.hovered() { TEXT() } else { MUTED() };
        let c = r.center();
        ui.painter().line_segment([c - Vec2::new(5.0, 0.0), c + Vec2::new(5.0, 0.0)], Stroke::new(1.6, col));
        if sign > 0.0 {
            ui.painter().line_segment([c - Vec2::new(0.0, 5.0), c + Vec2::new(0.0, 5.0)], Stroke::new(1.6, col));
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && can {
            *value = (value.round() + sign).clamp(min, max);
            changed = true;
        }
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, format!("{} px", value.round() as i32), FontId::proportional(12.5), TEXT());
    changed
}

/// A small rounded label, like the one that warns that a setting runs something on the server.
fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    let g = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), color);
    let (r, _) = ui.allocate_exact_size(g.size() + Vec2::new(16.0, 8.0), Sense::hover());
    ui.painter().rect_filled(r, 10.0, color.linear_multiply(0.14));
    ui.painter().galley(r.min + Vec2::new(8.0, 4.0), g, color);
}

fn combo(ui: &mut egui::Ui, id: &str, width: f32, selected: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::ComboBox::from_id_salt(id).width(width).height(320.0).selected_text(RichText::new(selected).size(12.5)).show_ui(ui, add);
}

fn control(ui: &mut egui::Ui, key: &str, cx: &mut RowCtx) {
    let s = &mut *cx.s;
    let mut ch = false;
    match key {
        "scheme" => combo(ui, "scheme", 190.0, &s.color_scheme.clone(), |ui| {
            for sc in SCHEMES {
                ch |= ui.selectable_value(&mut s.color_scheme, sc.name.to_string(), sc.name).changed();
            }
        }),
        "font" => {
            let current = crate::fonts::label(&s.font_family).to_string();
            combo(ui, "font", 190.0, &current, |ui| {
                ch |= ui.selectable_value(&mut s.font_family, String::new(), format!("{} (built in)", crate::fonts::BUILT_IN)).changed();
                for (name, _) in crate::fonts::installed() {
                    ch |= ui.selectable_value(&mut s.font_family, name.to_string(), *name).changed();
                }
            });
        }
        "size" => ch |= stepper(ui, &mut s.font_size, MIN_FONT_SIZE, MAX_FONT_SIZE),
        "lineh" => ch |= segmented(ui, &mut s.line_height, &[(1.0, "1.0"), (1.2, "1.2"), (1.4, "1.4")]),
        "cursor" => {
            ch |= toggle(ui, &mut s.cursor_blink).changed();
            ui.label(RichText::new("Blink").size(12.5).color(MUTED()));
            ui.add_space(10.0);
            let mut shape: &str = match s.cursor_shape.as_str() {
                "bar" => "bar",
                "underline" => "underline",
                _ => "block",
            };
            if segmented(ui, &mut shape, &[("block", "Block"), ("bar", "Bar"), ("underline", "Underline")]) {
                s.cursor_shape = shape.to_string();
                ch = true;
            }
        }
        "bold" => ch |= toggle(ui, &mut s.brighten_bold_text).changed(),
        "scroll" => {
            ch |= ui
                .add(
                    egui::DragValue::new(&mut s.scrollback)
                        .range(0..=1_000_000)
                        .speed(100.0)
                        .custom_formatter(|n, _| format!("{} lines", thousands(n as u64)))
                        .custom_parser(|t| t.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<f64>().ok()),
                )
                .changed()
        }
        "term" => combo(ui, "term-type", 190.0, &s.terminal_type.clone(), |ui| {
            let mut names: Vec<String> = TERM_TYPES.iter().map(|t| t.to_string()).collect();
            if !names.contains(&s.terminal_type) && !s.terminal_type.is_empty() {
                names.push(s.terminal_type.clone());
            }
            for n in names {
                ch |= ui.selectable_value(&mut s.terminal_type, n.clone(), n).changed();
            }
        }),
        "rclick" => ch |= toggle(ui, &mut s.right_click_to_paste).changed(),
        "meta" => ch |= toggle(ui, &mut s.option_is_meta).changed(),
        "bell" => {
            let mut mode: &str = match s.bell_mode() {
                "sound" => "sound",
                "flash" => "flash",
                _ => "off",
            };
            if segmented(ui, &mut mode, &[("off", "Off"), ("sound", "Sound"), ("flash", "Flash")]) {
                s.bell = mode.to_string();
                ch = true;
            }
        }
        "card" => {
            let label = crate::card::POSITIONS.iter().find(|(k, _)| *k == s.card_position).map(|(_, l)| *l).unwrap_or("Top right");
            combo(ui, "card-position", 150.0, label, |ui| {
                for (k, l) in crate::card::POSITIONS {
                    ch |= ui.selectable_value(&mut s.card_position, k.to_string(), l).changed();
                }
            });
        }
        "ac" => ch |= toggle(ui, &mut s.autocomplete).changed(),
        "rhist" => ch |= toggle(ui, &mut s.remote_history).changed(),
        "chist" => {
            if *cx.clear_armed {
                if danger_button(ui, "Clear").clicked() {
                    cx.clear_now = true;
                    *cx.clear_armed = false;
                }
                if flat_button(ui, "Cancel").clicked() {
                    *cx.clear_armed = false;
                }
            } else if ui.add_enabled_ui(cx.hist_count > 0, |ui| danger_button(ui, "Clear history…")).inner.clicked() {
                *cx.clear_armed = true;
            }
        }
        "name" => ch |= ui.add(egui::TextEdit::singleline(&mut s.user_name).hint_text("Your name").desired_width(190.0).margin(Vec2::new(8.0, 5.0))).changed(),
        "keepalive" => ch |= ui.add(egui::DragValue::new(&mut s.keep_alive).range(0..=3600).suffix(" s")).changed(),
        "reconnect" => ch |= toggle(ui, &mut s.autoreconnect).changed(),
        "editor" => combo(ui, "editor", 190.0, editor_label(&s.default_editor), |ui| {
            for (k, label) in EDITORS {
                ch |= ui.selectable_value(&mut s.default_editor, k.to_string(), label).changed();
            }
        }),
        "uifont" => {
            let current = crate::fonts::UI_FONTS.iter().find(|(k, _)| *k == s.ui_font).map(|(_, n)| *n).unwrap_or("Inter");
            combo(ui, "uifont", 190.0, current, |ui| {
                for (k, n) in crate::fonts::UI_FONTS {
                    ch |= ui.selectable_value(&mut s.ui_font, k.to_string(), *n).changed();
                }
            });
        }
        "apptheme" => {
            let mut v: &str = match s.app_theme.as_str() {
                "light" => "light",
                "system" => "system",
                _ => "dark",
            };
            if segmented(ui, &mut v, &[("dark", "Dark"), ("light", "Light"), ("system", "System")]) {
                s.app_theme = v.to_string();
                ch = true;
            }
        }
        "sort" => {
            let mut by: &str = if s.sort_by == "Name" { "Name" } else { "Date" };
            if segmented(ui, &mut by, &[("Date", "Last used"), ("Name", "Name")]) {
                s.sort_by = by.to_string();
                ch = true;
            }
        }
        _ => {}
    }
    cx.changed |= ch;
}

fn draw_row(ui: &mut egui::Ui, r: &Row, cx: &mut RowCtx) {
    let desc = match r.key {
        "chist" => format!("{} commands, stored only on this computer", thousands(cx.hist_count as u64)),
        "data" => format!("Everything is stored on this computer in {}", cx.data_path),
        _ => r.desc.to_string(),
    };
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let text_w = (ui.available_width() - 340.0).max(170.0);
        ui.vertical(|ui| {
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(RichText::new(r.title).size(13.5).color(TEXT()));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(11.5).color(MUTED()));
            }
            if r.key == "rhist" {
                ui.add_space(2.0);
                chip(ui, "Runs a command on the server", AMBER());
            }
            if r.key == "scheme" && !cx.own_scheme.is_empty() {
                let n = cx.own_scheme.len();
                let text = if n == 1 { "1 host has its own scheme".to_string() } else { format!("{n} hosts have their own scheme") };
                ui.label(RichText::new(text).size(11.5).color(BLUE())).on_hover_text(cx.own_scheme.join("\n"));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| control(ui, r.key, cx));
    });
    ui.add_space(12.0);
}

/// One card of settings with its title and "Reset to defaults". Draws nothing when the search leaves none of its rows.
fn draw_group(ui: &mut egui::Ui, g: &Group, q: &str, cx: &mut RowCtx) -> bool {
    let rows: Vec<&Row> = g.rows.iter().filter(|r| hit(q, g, r)).collect();
    if rows.is_empty() {
        return false;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(g.title).size(14.5).strong().color(TEXT()));
        if resettable(g) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let on = differs(cx.s, g);
                let b = ui.add_enabled(on, egui::Button::new(RichText::new("Reset to defaults").size(12.0).color(if on { BLUE() } else { MUTED() })).frame(false));
                if b.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    for r in g.rows {
                        reset(cx.s, r.key);
                    }
                    cx.changed = true;
                }
            });
        }
    });
    ui.add_space(4.0);
    Frame::new().fill(CARD()).stroke(Stroke::new(1.0, BORDER())).corner_radius(10).inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        for (i, r) in rows.iter().enumerate() {
            if i > 0 {
                let (sep, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().hline(sep.x_range(), sep.center().y, Stroke::new(1.0, BORDER().linear_multiply(0.7)));
            }
            draw_row(ui, r, cx);
        }
    });
    ui.add_space(24.0);
    true
}

/// A small terminal drawn with the current settings, then the 16 colors of the scheme.
/// Longest preview line, in cells ("app  logs  notes.txt  deploy.sh" is 31), plus one cell of air.
const PREVIEW_COLS: f32 = 32.0;

/// Width the preview card needs so its longest line fits at the chosen font, size and line height.
fn preview_width(ui: &egui::Ui, s: &Settings) -> f32 {
    let (cs, _) = cell_size(ui, &crate::fonts::term(s.font_size.clamp(8.0, 20.0)), s.line_height);
    // terminal padding 12 + 12, card margin 12 + 12, border and a few px of air
    PREVIEW_COLS * cs.x + 24.0 + 24.0 + 8.0
}

fn preview(ui: &mut egui::Ui, s: &Settings, below: bool) {
    let sch: &Scheme = find_scheme(&s.color_scheme);
    let pal = |i: usize| rgb(sch.palette[i]);
    let fg = rgb(sch.foreground);
    let bg = rgb(sch.background);
    ui.label(RichText::new("Preview").size(14.5).strong().color(TEXT()));
    ui.add_space(4.0);
    Frame::new().fill(CARD()).stroke(Stroke::new(1.0, BORDER())).corner_radius(10).inner_margin(Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let w = ui.available_width();
        let mut size = s.font_size.clamp(8.0, 20.0);
        let (mut cs, _) = cell_size(ui, &crate::fonts::term(size), s.line_height);
        // when the window is too narrow for the card to grow, the text shrinks just enough to fit rather than being cut off
        if PREVIEW_COLS * cs.x > w - 24.0 {
            size *= (w - 24.0) / (PREVIEW_COLS * cs.x);
            cs = cell_size(ui, &crate::fonts::term(size), s.line_height).0;
        }
        let font = crate::fonts::term(size);
        let (cs, ty) = cell_size(ui, &font, s.line_height);
        let bold_dir = if s.brighten_bold_text { pal(12) } else { pal(4) };
        let prompt = |cmd: &'static str| vec![("user@web-01", pal(2), false), (":", fg, false), ("~", pal(4), false), ("$ ", fg, false), (cmd, fg, false)];
        let lines: Vec<Vec<(&str, Color32, bool)>> = vec![
            prompt("ls"),
            vec![("app", bold_dir, true), ("  ", fg, false), ("logs", bold_dir, true), ("  notes.txt", fg, false), ("  deploy.sh", pal(2), false)],
            prompt("git status"),
            vec![("On branch ", fg, false), ("main", pal(6), false)],
            vec![("modified:   ", pal(1), false), ("config.yml", fg, false)],
            prompt(""),
        ];
        let bar = 26.0;
        let (win, _) = ui.allocate_exact_size(Vec2::new(w, bar + 12.0 + cs.y * lines.len() as f32 + 10.0), Sense::hover());
        let p = ui.painter().with_clip_rect(win);
        p.rect_filled(win, 8.0, bg);
        let top = Rect::from_min_size(win.min, Vec2::new(w, bar));
        p.rect_filled(top, egui::CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, crate::term::blend(bg, Color32::BLACK, 0.25));
        for (i, c) in [Color32::from_rgb(0xFF, 0x5F, 0x57), Color32::from_rgb(0xFE, 0xBC, 0x2E), Color32::from_rgb(0x28, 0xC8, 0x40)].iter().enumerate() {
            p.circle_filled(Pos2::new(win.left() + 14.0 + i as f32 * 14.0, top.center().y), 4.5, *c);
        }
        p.text(top.center(), Align2::CENTER_CENTER, "user@web-01: ~", FontId::proportional(11.0), fg.linear_multiply(0.6));
        let origin = Pos2::new(win.left() + 12.0, top.bottom() + 8.0);
        let mut end = origin;
        for (row, spans) in lines.iter().enumerate() {
            let mut col = 0usize;
            for (text, color, bold) in spans {
                let at = Pos2::new(origin.x + col as f32 * cs.x, origin.y + row as f32 * cs.y + ty);
                let g = p.layout_no_wrap(text.to_string(), font.clone(), *color);
                p.galley(at, g.clone(), *color);
                if *bold {
                    p.galley(at + Vec2::new(0.6, 0.0), g, *color);
                }
                col += text.chars().count();
            }
            end = Pos2::new(origin.x + col as f32 * cs.x, origin.y + row as f32 * cs.y);
        }
        let on = !s.cursor_blink || (ui.input(|i| i.time) * 1.8) as i64 % 2 == 0;
        if on {
            paint_cursor(&p, Rect::from_min_size(end, cs), &s.cursor_shape, rgb(sch.cursor));
        }
        if s.cursor_blink {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(280));
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(sch.name).size(12.5).strong().color(TEXT()));
            ui.label(RichText::new(format!("{} · {} px", crate::fonts::label(&s.font_family), s.font_size.round() as i32)).size(11.5).color(MUTED()));
        });
        ui.add_space(6.0);
        let gap = 5.0;
        let sw = ((w - gap * 7.0) / 8.0).floor();
        for half in 0..2 {
            let (rowr, _) = ui.allocate_exact_size(Vec2::new(w, 22.0), Sense::hover());
            for k in 0..8 {
                let i = half * 8 + k;
                let r = Rect::from_min_size(Pos2::new(rowr.left() + k as f32 * (sw + gap), rowr.top()), Vec2::new(sw, 22.0));
                ui.painter().rect(r, 5.0, pal(i), Stroke::new(1.0, wash(18)), StrokeKind::Inside);
                ui.interact(r, ui.id().with(("swatch", i)), Sense::hover()).on_hover_text(format!("{}  #{:06X}", ANSI[i], sch.palette[i]));
            }
            ui.add_space(4.0);
        }
        ui.add_space(4.0);
        ui.label(RichText::new(if below { "Preview follows every change below" } else { "Preview follows every change on the left" }).size(11.0).color(MUTED()));
    });
}

/// The search box at the top right of Settings.
fn search_box(ui: &mut egui::Ui, text: &mut String, width: f32) {
    search_field(ui, text, width, "settings-search", "Search settings (font, cursor, history…)");
}

/// A rounded search box with a magnifier and a clear button.
pub(crate) fn search_field(ui: &mut egui::Ui, text: &mut String, width: f32, id: &str, hint: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::hover());
    let id = egui::Id::new(id);
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect(rect, 8.0, CARD(), Stroke::new(if focused { 1.5 } else { 1.0 }, if focused { BLUE() } else { BORDER() }), StrokeKind::Inside);
    icons::paint(ui.painter(), Pos2::new(rect.left() + 17.0, rect.center().y), 14.0, Icon::Search, MUTED());
    let clear_w = if text.is_empty() { 0.0 } else { 28.0 };
    let inner = Rect::from_min_max(rect.min + Vec2::new(34.0, 0.0), rect.max - Vec2::new(10.0 + clear_w, 0.0));
    let mut edit_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let r = edit_ui.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(Frame::NONE)
            .hint_text(hint)
            .desired_width(inner.width())
            .margin(Vec2::ZERO)
            .vertical_align(egui::Align::Center)
            .min_size(Vec2::new(0.0, 34.0)),
    );
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        text.clear();
    }
    if !text.is_empty() {
        let cr = Rect::from_center_size(Pos2::new(rect.right() - 20.0, rect.center().y), Vec2::splat(22.0));
        let resp = ui.interact(cr, id.with("clear"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(cr, 5.0, BORDER());
        }
        icons::paint(ui.painter(), cr.center(), 12.0, Icon::Close, if resp.hovered() { TEXT() } else { MUTED() });
        if resp.on_hover_text("Clear the search").clicked() {
            text.clear();
        }
    }
}

/// The small green check before "Changes save automatically".
fn check_mark(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
    let c = r.center();
    ui.painter().add(egui::Shape::line(vec![c + Vec2::new(-4.5, 0.0), c + Vec2::new(-1.5, 3.0), c + Vec2::new(4.5, -3.5)], Stroke::new(1.8, GREEN())));
}

/// The app icon as a texture, decoded once and kept in egui's memory.
fn app_icon(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("app-icon-texture");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let tex = ctx.load_texture("app-icon", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

const REPO_URL: &str = "https://github.com/tonihardianto/sshtup";

/// A titled card on the About tab.
fn about_card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).size(14.5).strong().color(TEXT()));
    ui.add_space(4.0);
    Frame::new().fill(CARD()).stroke(Stroke::new(1.0, BORDER())).corner_radius(10).inner_margin(Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui);
    });
    ui.add_space(24.0);
}

/// One row of an About card: a label on the left, the value on the right.
fn about_row(ui: &mut egui::Ui, label: &str, separator: bool, value: impl FnOnce(&mut egui::Ui)) {
    if separator {
        let (sep, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(sep.x_range(), sep.center().y, Stroke::new(1.0, BORDER().linear_multiply(0.7)));
    }
    ui.horizontal(|ui| {
        ui.set_min_height(42.0);
        ui.label(RichText::new(label).size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), value);
    });
}

impl App {
    /// The About tab: what this app is, its version and where to find it.
    fn settings_about(&mut self, ui: &mut egui::Ui) {
        let version = env!("CARGO_PKG_VERSION");
        let platform = format!("{} {}", std::env::consts::OS, std::env::consts::ARCH);
        let data_path = data_dir().to_string_lossy().to_string();

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            let (tile, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
            match app_icon(ui.ctx()) {
                Some(tex) => {
                    ui.painter().image(tex.id(), tile, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    ui.painter().rect_filled(tile, 14.0, BLUE().linear_multiply(0.18));
                    crate::icons::paint(ui.painter(), tile.center(), 28.0, crate::icons::Icon::Terminal, BLUE());
                }
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new("SSHTUP").size(24.0).strong());
                ui.label(RichText::new(format!("Version {version}")).size(13.0).color(MUTED()));
                ui.label(RichText::new("An SSH client with self-hosted sync.").size(13.0).color(TEXT()));
            });
        });
        ui.add_space(20.0);
        if flat_button(ui, "Copy version info").on_hover_text("Handy to paste into a bug report").clicked() {
            ui.ctx().copy_text(format!("SSHTUP {version} ({platform})"));
            self.toast = Some(("Version info copied".into(), std::time::Instant::now(), GREEN()));
        }
        ui.add_space(22.0);

        about_card(ui, "Details", |ui| {
            about_row(ui, "Version", false, |ui| {
                ui.label(RichText::new(version).size(13.0).color(MUTED()));
            });
            about_row(ui, "Platform", true, |ui| {
                ui.label(RichText::new(&platform).size(13.0).color(MUTED()));
            });
            about_row(ui, "License", true, |ui| {
                ui.label(RichText::new("MIT").size(13.0).color(MUTED()));
            });
            about_row(ui, "Built with", true, |ui| {
                ui.label(RichText::new("Rust and egui").size(13.0).color(MUTED()));
            });
            about_row(ui, "Data folder", true, |ui| {
                ui.add(egui::Label::new(RichText::new(&data_path).monospace().size(11.5).color(MUTED())).truncate()).on_hover_text(&data_path);
            });
        });

        let update = self.updates.state();
        let mut check = false;
        about_card(ui, "Updates", |ui| {
            about_row(ui, "Status", false, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let busy = update == crate::update::State::Checking;
                    if ui.add_enabled(!busy, egui::Button::new(RichText::new(if busy { "Checking…" } else { "Check for updates" }).size(12.5)).min_size(Vec2::new(0.0, 30.0))).clicked() {
                        check = true;
                    }
                    ui.add_space(8.0);
                    match &update {
                        crate::update::State::Idle => {
                            ui.label(RichText::new(format!("You have {version}")).size(13.0).color(MUTED()));
                        }
                        crate::update::State::Checking => {
                            ui.spinner();
                        }
                        crate::update::State::UpToDate => {
                            ui.label(RichText::new("You are up to date").size(13.0).color(GREEN()));
                        }
                        crate::update::State::Available { version: v, url } => {
                            ui.hyperlink_to(RichText::new(format!("Version {v} is available — download")).size(13.0), url);
                        }
                        crate::update::State::Failed(msg) => {
                            ui.add(egui::Label::new(RichText::new(msg).size(12.5).color(RED())).truncate());
                        }
                    }
                });
            });
        });
        if check {
            self.updates.start(ui.ctx());
        }

        about_card(ui, "Links", |ui| {
            about_row(ui, "Project", false, |ui| {
                ui.hyperlink_to(RichText::new("github.com/tonihardianto/sshtup").size(13.0), REPO_URL);
            });
            about_row(ui, "Latest release", true, |ui| {
                ui.hyperlink_to(RichText::new("Download").size(13.0), format!("{REPO_URL}/releases/latest"));
            });
            about_row(ui, "Problems or ideas", true, |ui| {
                ui.hyperlink_to(RichText::new("Open an issue").size(13.0), format!("{REPO_URL}/issues"));
            });
        });

        ui.add(egui::Label::new(RichText::new("Early software. The encryption design has not had an independent review, so read SYNC.md before trusting it with anything important. The colour schemes other than Default Dark and Light are community themes that belong to their authors.").size(12.0).color(MUTED())).wrap());
        ui.add_space(20.0);
    }

    pub fn screen_settings(&mut self, ui: &mut egui::Ui) {
        // header: title, the autosave note, search
        let mut query = std::mem::take(&mut self.settings_query);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Settings").size(22.0).strong());
            ui.add_space(8.0);
            check_mark(ui);
            ui.label(RichText::new("Changes save automatically").size(12.0).color(MUTED()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let w = (ui.available_width() - 20.0).clamp(160.0, 300.0);
                search_box(ui, &mut query, w);
            });
        });
        ui.add_space(10.0);
        let q = query.trim().to_lowercase();
        let searching = !q.is_empty();

        // tabs, underlined
        let (bar, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
        ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, BORDER()));
        let mut x = bar.left();
        // shown in this order; the number is the page (kept stable: 0 is Terminal)
        use crate::icons::Icon;
        let tabs = [(1u8, "General", Icon::Sliders), (0, "Terminal", Icon::Terminal), (2, "Security", Icon::Lock), (3, "Sync", Icon::Sync), (4, "About", Icon::Info)];
        for (i, name, icon) in tabs {
            let on = self.settings_tab == i && !searching;
            let g = ui.painter().layout_no_wrap(name.to_string(), FontId::proportional(13.5), TEXT());
            let r = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(g.size().x + 26.0, bar.height()));
            let resp = ui.interact(r, egui::Id::new(("settings-tab", i)), Sense::click());
            let col = if on || resp.hovered() { TEXT() } else { MUTED() };
            crate::icons::paint(ui.painter(), Pos2::new(r.left() + 9.0, r.center().y - 2.0), 15.0, icon, col);
            ui.painter().text(Pos2::new(r.left() + 24.0, r.center().y - 2.0), Align2::LEFT_CENTER, name, FontId::proportional(13.5), col);
            if on {
                ui.painter().rect_filled(Rect::from_min_max(Pos2::new(r.left(), bar.bottom() - 2.0), Pos2::new(r.right(), bar.bottom())), 1.0, BLUE());
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.settings_tab = i;
                query.clear();
                self.clear_armed = false;
            }
            x = r.right() + 26.0;
        }
        ui.add_space(18.0);
        let tab = self.settings_tab;
        let searching = !query.trim().is_empty();

        // body: the settings on the left, the preview on the right (or on top when the window is narrow)
        let body = ui.available_rect_before_wrap();
        ui.allocate_rect(body, Sense::hover());
        let show_preview = !searching && tab == 0;
        let side = show_preview && body.width() >= 900.0;
        let (left, right) = if side {
            // the settings fill the width left of the preview, which keeps to the right edge and grows a little on wide windows
            // wide enough for the longest preview line at the chosen font size, but always leaving the settings 480 px
            let need = preview_width(ui, &self.vault.settings);
            let (pw, gap) = ((body.width() * 0.26).clamp(300.0, 400.0).max(need).min(body.width() - 28.0 - 480.0), 28.0);
            let lw = body.width() - pw - gap;
            let px = body.left() + lw + gap;
            (Rect::from_min_max(body.min, Pos2::new(body.left() + lw, body.bottom())), Some(Rect::from_min_max(Pos2::new(px, body.top()), Pos2::new(px + pw, body.bottom()))))
        } else {
            (body, None)
        };

        let own_scheme: Vec<String> = self.vault.hosts.iter().filter(|h| h.color_scheme.as_deref().map(|s| !s.is_empty()).unwrap_or(false)).map(|h| h.display_label()).collect();
        let data_path = data_dir().to_string_lossy().to_string();
        let hist_count = self.history.count();
        let mut clear_armed = self.clear_armed;
        let own_page = !searching && tab >= 2;
        let mut cx = RowCtx { s: &mut self.vault.settings, changed: false, hist_count, clear_armed: &mut clear_armed, clear_now: false, own_scheme: &own_scheme, data_path: &data_path };
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(left));
        if !own_page {
        egui::ScrollArea::vertical().auto_shrink(false).id_salt(("settings_scroll", tab, searching)).show(&mut lui, |ui| {
            if !side {
                ui.set_max_width(ui.available_width().min(780.0));
            }
            if show_preview && !side {
                ui.set_max_width(ui.available_width().min(560.0));
                preview(ui, cx.s, true);
                ui.add_space(24.0);
            }
            if searching {
                let mut any = false;
                for g in TERMINAL.iter().chain(GENERAL) {
                    any |= draw_group(ui, g, &q, &mut cx);
                }
                if !any {
                    ui.add_space(30.0);
                    ui.label(RichText::new(format!("No settings match \u{201C}{}\u{201D}", query.trim())).size(14.0).color(TEXT()));
                    ui.label(RichText::new("Security and Sync have their own tabs.").size(12.0).color(MUTED()));
                }
            } else {
                for g in if tab == 0 { TERMINAL } else { GENERAL } {
                    draw_group(ui, g, "", &mut cx);
                }
            }
        });
        }
        let (changed, clear_now) = (cx.changed, cx.clear_now);
        // Security, Sync and About draw themselves (they need the whole App)
        if own_page {
            egui::ScrollArea::vertical().auto_shrink(false).id_salt(("settings_scroll", tab)).show(&mut lui, |ui| {
                ui.set_max_width(ui.available_width().min(720.0));
                match tab {
                    2 => self.settings_security(ui),
                    3 => self.settings_sync(ui),
                    _ => self.settings_about(ui),
                }
            });
        }
        if let Some(r) = right {
            let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(r));
            preview(&mut rui, &self.vault.settings, false);
        }

        self.clear_armed = clear_armed;
        self.settings_query = query;
        if clear_now {
            self.history.clear();
            self.hist_tried.clear();
            self.history.save();
            self.toast = Some(("Command history cleared".into(), std::time::Instant::now(), GREEN()));
        }
        if changed {
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(40_000), "40,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn search_finds_rows_by_title_description_or_extra_words() {
        let g = &TERMINAL[0];
        let cursor = g.rows.iter().find(|r| r.key == "cursor").unwrap();
        assert!(hit("cursor", g, cursor));
        assert!(hit("blink", g, cursor));
        assert!(hit("appearance under", g, cursor));
        assert!(!hit("history", g, cursor));
    }

    #[test]
    fn reset_puts_a_group_back_to_its_defaults() {
        let mut s = Settings::default();
        s.cursor_shape = "bar".into();
        s.line_height = 1.4;
        s.scrollback = 5;
        assert!(differs(&s, &TERMINAL[0]));
        for r in TERMINAL[0].rows {
            reset(&mut s, r.key);
        }
        assert!(!differs(&s, &TERMINAL[0]));
        assert_eq!(s.scrollback, 5, "another group's setting stays");
    }

    #[test]
    fn every_row_key_is_handled_by_reset_or_is_not_a_setting() {
        for g in TERMINAL.iter().chain(GENERAL) {
            for r in g.rows {
                if matches!(r.key, "chist" | "data" | "name") {
                    continue;
                }
                let mut s = Settings::default();
                // flip everything a reset could touch, then check the row's reset brings back something
                s.app_theme = "light".into();
                s.ui_font = "dmsans".into();
                s.color_scheme = "x".into();
                s.font_family = "x".into();
                s.font_size = 30.0;
                s.line_height = 1.4;
                s.cursor_shape = "bar".into();
                s.cursor_blink = false;
                s.brighten_bold_text = true;
                s.scrollback = 1;
                s.terminal_type = "x".into();
                s.right_click_to_paste = true;
                s.option_is_meta = true;
                s.bell = "flash".into();
                s.card_position = "hidden".into();
                s.autocomplete = false;
                s.remote_history = false;
                s.keep_alive = 1;
                s.autoreconnect = false;
                s.default_editor = "x".into();
                s.sort_by = "Name".into();
                let before = serde_json::to_value(&s).unwrap();
                reset(&mut s, r.key);
                assert_ne!(before, serde_json::to_value(&s).unwrap(), "reset does nothing for {}", r.key);
            }
        }
    }

    #[test]
    fn old_bell_setting_still_counts() {
        let s: Settings = serde_json::from_str(r#"{"bell_sound": true}"#).unwrap();
        assert_eq!(s.bell_mode(), "sound");
        let s: Settings = serde_json::from_str(r#"{"bell_sound": true, "bell": "flash"}"#).unwrap();
        assert_eq!(s.bell_mode(), "flash");
        assert_eq!(Settings::default().bell_mode(), "off");
    }
}

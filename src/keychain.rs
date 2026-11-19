//! The Keychain page: a searchable list of keys (or identities) and, beside it, the details of the one picked.

use crate::app::{expand_home, App, Del, Editor};
use crate::icons::Icon;
use crate::key_info::{self, KeyInfo};
use crate::model::*;
use crate::screens::KeyImport;
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[allow(non_snake_case)]
fn PILL() -> Color32 {
    pick(0x2A2D42, 0xE3E6EE)
}
#[allow(non_snake_case)]
fn WELL() -> Color32 {
    pick(0x1E2236, 0xEEF0F5)
}
const ROW_H: f32 = 64.0;
const PANEL_W: f32 = 372.0;

/// Save a copy of a key under a name of the person's choosing.
pub struct KeyExport {
    id: u64,
    private: bool,
    dest: String,
}

#[derive(Default)]
pub struct KeychainUi {
    /// 0 = Keys, 1 = Identities.
    tab: usize,
    sel: Option<u64>,
    search: String,
    inited: bool,
    /// (is an identity, id, new name)
    rename: Option<(bool, u64, String)>,
    export: Option<KeyExport>,
    /// What was read from each key's files, and when (the files are looked at again every few seconds).
    info: HashMap<u64, (String, String, Instant, KeyInfo)>,
}

enum Act {
    NewKey,
    NewIdentity,
    ImportFile,
    Edit(bool, u64),
    Rename(bool, u64),
    Export(u64),
    Delete(bool, u64),
}

/// One row of the list, whichever tab it is on.
struct Row {
    id: u64,
    title: String,
    /// A small tag after the title: the key type, or the username.
    tag: String,
    /// The badges after the tag: (icon, text, ok?)
    badges: Vec<(Icon, String, bool)>,
    /// The line under the title.
    sub: String,
    sub_mono: bool,
    used: usize,
    when: String,
    warn: bool,
    identity: bool,
}

impl App {
    fn key_info(&mut self, k: &SshKey) -> KeyInfo {
        let now = Instant::now();
        if let Some((path, public, at, info)) = self.kc.info.get(&k.id) {
            if *path == k.path && *public == k.public_key && now.duration_since(*at) < Duration::from_secs(4) {
                return info.clone();
            }
        }
        let info = key_info::inspect(&k.path, &k.public_key);
        self.kc.info.insert(k.id, (k.path.clone(), k.public_key.clone(), now, info.clone()));
        info
    }

    /// The hosts that use a key, directly or through an identity: (label, is an identity).
    fn key_used_by(&self, key: u64) -> Vec<(String, bool)> {
        let ids_with_key: Vec<u64> = self.vault.identities.iter().filter(|i| i.key == Some(key)).map(|i| i.id).collect();
        let mut v: Vec<(String, bool)> = self.vault.hosts.iter().filter(|h| h.key == Some(key) || h.identity.is_some_and(|i| ids_with_key.contains(&i))).map(|h| (h.display_label(), false)).collect();
        v.extend(self.vault.identities.iter().filter(|i| i.key == Some(key)).map(|i| (i.label.clone(), true)));
        v
    }

    fn identity_used_by(&self, id: u64) -> Vec<(String, bool)> {
        self.vault.hosts.iter().filter(|h| h.identity == Some(id)).map(|h| (h.display_label(), false)).collect()
    }

    fn key_rows(&mut self) -> Vec<Row> {
        let keys: Vec<_> = self.vault.keys.iter().filter(|k| self.in_vault("key", k.id)).cloned().collect();
        let q = self.kc.search.trim().to_lowercase();
        let mut rows = vec![];
        for k in &keys {
            let info = self.key_info(k);
            let hay = format!("{} {} {} {}", k.label, info.fingerprint, info.type_label(), k.path).to_lowercase();
            if !q.is_empty() && !hay.contains(&q) {
                continue;
            }
            let mut badges = vec![];
            match info.protected {
                Some(true) => badges.push((Icon::Lock, "Passphrase".to_string(), true)),
                Some(false) => badges.push((Icon::Alert, "No passphrase".to_string(), false)),
                None => {}
            }
            if info.weak {
                badges.push((Icon::Alert, "Weak key".to_string(), false));
            }
            let used = self.key_used_by(k.id).iter().filter(|(_, ident)| !*ident).count();
            let when = match (k.added, k.generated) {
                (0, _) => String::new(),
                (t, true) => format!("Generated · {}", key_info::date(t)),
                (t, false) => format!("Imported · {}", key_info::date(t)),
            };
            let (sub, sub_mono) = if info.fingerprint.is_empty() { (k.path.clone(), true) } else { (key_info::short_fingerprint(&info.fingerprint), true) };
            rows.push(Row { id: k.id, title: k.label.clone(), tag: info.type_label(), warn: info.weak || info.protected == Some(false), badges, sub, sub_mono, used, when, identity: false });
        }
        rows
    }

    fn identity_rows(&self) -> Vec<Row> {
        let q = self.kc.search.trim().to_lowercase();
        let mut rows = vec![];
        for i in self.vault.identities.iter().filter(|i| self.in_vault("identity", i.id)) {
            if !q.is_empty() && !format!("{} {}", i.label, i.username).to_lowercase().contains(&q) {
                continue;
            }
            let mut badges = vec![];
            if let Some(k) = i.key.and_then(|k| self.vault.keys.iter().find(|x| x.id == k)) {
                badges.push((Icon::Key, k.label.clone(), true));
            }
            if !i.password.is_empty() {
                badges.push((Icon::Lock, "Password".to_string(), true));
            }
            let used = self.identity_used_by(i.id).len();
            rows.push(Row { id: i.id, title: i.label.clone(), tag: i.username.clone(), badges, sub: if i.username.is_empty() { "No username".into() } else { format!("Signs in as {}", i.username) }, sub_mono: false, used, when: String::new(), warn: false, identity: true });
        }
        rows
    }

    pub fn screen_keychain(&mut self, ui: &mut egui::Ui) {
        let mut act: Option<Act> = None;
        if self.demo && !self.kc.inited {
            self.kc.inited = true;
        }
        self.keychain_header(ui, &mut act);
        ui.add_space(10.0);
        ui.add_space(2.0);

        let mut rows = self.key_rows();
        rows.extend(self.identity_rows());
        if self.kc.sel.is_none_or(|s| !rows.iter().any(|r| r.id == s && r.identity == (self.kc.tab == 1))) {
            if let Some(r) = rows.first() {
                self.kc.sel = Some(r.id);
                self.kc.tab = r.identity as usize;
            } else {
                self.kc.sel = None;
            }
        }
        let detail = self.kc.sel.is_some() && matches!(self.editor, Editor::None);

        let total = ui.available_width();
        let list_w = if detail { (total - PANEL_W - 20.0).max(380.0) } else { total };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            ui.allocate_ui_with_layout(Vec2::new(list_w, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(list_w);
                egui::ScrollArea::vertical().id_salt("keychain-list").auto_shrink(false).show(ui, |ui| {
                    ui.set_width(list_w);
                    self.keychain_list(ui, &rows, &mut act);
                });
            });
            if detail {
                ui.allocate_ui_with_layout(Vec2::new(PANEL_W.min(ui.available_width()), ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                    egui::ScrollArea::vertical().id_salt("keychain-detail").auto_shrink(false).show(ui, |ui| self.keychain_detail(ui, &mut act));
                });
            }
        });

        // a key file dropped on the window goes to the import window to be named
        let (dropped, over) = ui.ctx().input(|i| (i.raw.dropped_files.iter().find_map(|f| Some(f.path()).filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf())), !i.raw.hovered_files.is_empty()));
        let _ = over;
        if let Some(p) = dropped {
            if self.key_import.is_none() && matches!(self.editor, Editor::None) {
                let label = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                self.key_import = Some(KeyImport { path: p.to_string_lossy().to_string(), label, ..Default::default() });
            }
        }
        if self.demo && std::env::var("SSHTUP_OPEN_IMPORT").is_ok() && self.key_import.is_none() {
            act = Some(Act::ImportFile);
        }
        self.key_import_window(ui.ctx());
        self.keychain_dialogs(ui.ctx());
        self.keychain_act(act);
    }

    fn keychain_header(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Keychain").size(24.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // New Key, with the other ways to add one in its menu
                let label = ui.painter().layout_no_wrap("+  New Key".into(), FontId::proportional(13.0), Color32::WHITE);
                ui.spacing_mut().item_spacing.x = 0.0;
                let (arrow_rect, arrow) = ui.allocate_exact_size(Vec2::new(32.0, 38.0), Sense::click());
                let (main_rect, main) = ui.allocate_exact_size(Vec2::new(label.size().x + 30.0, 38.0), Sense::click());
                let ctl = |r: &egui::Response| if r.hovered() { BLUE().linear_multiply(1.15) } else { BLUE() };
                ui.painter().rect_filled(main_rect, CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, ctl(&main));
                ui.painter().rect_filled(arrow_rect, CornerRadius { nw: 0, sw: 0, ne: 8, se: 8 }, ctl(&arrow));
                ui.painter().line_segment([arrow_rect.left_top() + Vec2::new(0.0, 7.0), arrow_rect.left_bottom() - Vec2::new(0.0, 7.0)], Stroke::new(1.0, Color32::from_white_alpha(70)));
                ui.painter().galley(Pos2::new(main_rect.left() + 15.0, main_rect.center().y - label.size().y / 2.0), label, Color32::WHITE);
                crate::icons::paint(ui.painter(), arrow_rect.center() + Vec2::new(1.0, 0.0), 12.0, Icon::ChevDown, Color32::WHITE);
                if main.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *act = Some(Act::NewKey);
                }
                egui::Popup::menu(&arrow.on_hover_cursor(egui::CursorIcon::PointingHand)).gap(6.0).align(egui::RectAlign::BOTTOM_END).show(|ui| {
                    ui.set_min_width(200.0);
                    ui.spacing_mut().button_padding = Vec2::new(12.0, 7.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if icon_text_button(ui, Icon::Key, "Generate a new key").clicked() {
                        *act = Some(Act::NewKey);
                        ui.close();
                    }
                    if icon_text_button(ui, Icon::Download, "Import from file").clicked() {
                        *act = Some(Act::ImportFile);
                        ui.close();
                    }
                    if icon_text_button(ui, Icon::User, "New identity").clicked() {
                        *act = Some(Act::NewIdentity);
                        ui.close();
                    }
                });
                ui.add_space(10.0);
                let hint = "Search keys, identities…";
                crate::settings_ui::search_field(ui, &mut self.kc.search, 285.0, "keychain-search", hint);
            });
        });
    }

    fn keychain_list(&mut self, ui: &mut egui::Ui, rows: &[Row], act: &mut Option<Act>) {
        if rows.is_empty() {
            let empty = self.vault.identities.is_empty() && self.vault.keys.is_empty();
            if !self.kc.search.trim().is_empty() && !empty {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| label_muted(ui, "Nothing matches the search."));
            } else {
                ui.add_space(6.0);
            }
        } else {
            let w = ui.available_width();
            let (frame, _) = ui.allocate_exact_size(Vec2::new(w, ROW_H * rows.len() as f32), Sense::hover());
            ui.painter().rect(frame, 10, CARD(), Stroke::new(1.0, BORDER()), StrokeKind::Inside);
            for (n, row) in rows.iter().enumerate() {
                let r = Rect::from_min_size(frame.min + Vec2::new(0.0, ROW_H * n as f32), Vec2::new(w, ROW_H));
                self.keychain_row(ui, r, row, n == 0, n + 1 == rows.len(), act);
            }
            ui.add_space(12.0);
        }
    }

    fn keychain_row(&mut self, ui: &mut egui::Ui, r: Rect, row: &Row, first: bool, last: bool, act: &mut Option<Act>) {
        let resp = ui.interact(r.shrink2(Vec2::new(1.0, 0.0)), egui::Id::new(("keychain-row", row.identity, row.id)), Sense::click());
        let on = self.kc.sel == Some(row.id) && (self.kc.tab == 1) == row.identity;
        let radius = CornerRadius { nw: if first { 9 } else { 0 }, ne: if first { 9 } else { 0 }, sw: if last { 9 } else { 0 }, se: if last { 9 } else { 0 } };
        let p = ui.painter();
        if on {
            p.rect_filled(r.shrink(1.0), radius, BLUE().linear_multiply(0.13));
        } else if resp.hovered() {
            p.rect_filled(r.shrink(1.0), radius, CARD_HOVER().linear_multiply(0.7));
        }
        if !last {
            p.line_segment([Pos2::new(r.left() + 1.0, r.bottom()), Pos2::new(r.right() - 1.0, r.bottom())], Stroke::new(1.0, BORDER()));
        }
        // the icon
        let accent = if row.warn { AMBER() } else { BLUE() };
        let ib = Rect::from_center_size(Pos2::new(r.left() + 36.0, r.center().y), Vec2::splat(42.0));
        p.rect_filled(ib, 10, accent.linear_multiply(0.16));
        crate::icons::paint(p, ib.center(), 19.0, if !row.identity { Icon::Key } else { Icon::User }, accent);
        // the right-hand column
        let rx = r.right() - 18.0;
        let used = if row.used == 0 { "Not used yet".to_string() } else if row.identity || row.used > 1 { format!("Used by {} host{}", row.used, if row.used == 1 { "" } else { "s" }) } else { "Used by 1 host".to_string() };
        let ug = p.layout_no_wrap(used, FontId::proportional(12.5), if row.used == 0 { MUTED() } else { TEXT() });
        let ug_w = ug.size().x;
        p.galley(Pos2::new(rx - ug_w, r.center().y - 19.0), ug, MUTED());
        let wg = p.layout_no_wrap(row.when.clone(), FontId::proportional(12.0), MUTED());
        let wg_w = wg.size().x;
        p.galley(Pos2::new(rx - wg_w, r.center().y + 2.0), wg, MUTED());
        let avail_right = rx - ug_w.max(wg_w) - 16.0;
        // title, tag, badges
        let x0 = r.left() + 72.0;
        let tg = p.layout_no_wrap(row.title.clone(), FontId::proportional(14.5), TEXT());
        let mut x = x0 + tg.size().x;
        p.galley(Pos2::new(x0, r.center().y - 21.0), tg, TEXT());
        if !row.tag.is_empty() {
            let g = p.layout_no_wrap(row.tag.clone(), FontId::monospace(11.0), MUTED());
            let pill = Rect::from_min_size(Pos2::new(x + 10.0, r.center().y - 20.0), Vec2::new(g.size().x + 14.0, 19.0));
            if pill.right() < avail_right {
                p.rect_filled(pill, 5, PILL());
                p.galley(Pos2::new(pill.center().x - g.size().x / 2.0, pill.center().y - g.size().y / 2.0), g, MUTED());
                x = pill.right();
            }
        }
        for (icon, text, ok) in &row.badges {
            let col = if *ok { GREEN() } else { AMBER() };
            let g = p.layout_no_wrap(text.clone(), FontId::proportional(11.5), col);
            let mut pill = Rect::from_min_size(Pos2::new(x + 8.0, r.center().y - 20.0), Vec2::new(g.size().x + 30.0, 19.0));
            // too wide: keep only the icon
            let compact = pill.right() > avail_right;
            if compact {
                pill.set_width(24.0);
                if pill.right() > avail_right {
                    break;
                }
            }
            p.rect_filled(pill, 5, col.linear_multiply(0.14));
            if compact {
                crate::icons::paint(p, pill.center(), 11.0, *icon, col);
            } else {
                crate::icons::paint(p, Pos2::new(pill.left() + 11.0, pill.center().y), 11.0, *icon, col);
                p.galley(Pos2::new(pill.left() + 20.0, pill.center().y - g.size().y / 2.0), g, col);
            }
            x = pill.right();
        }
        // the line under the title
        let sub_font = if row.sub_mono { FontId::monospace(11.5) } else { FontId::proportional(12.5) };
        p.text(Pos2::new(x0, r.center().y + 10.0), Align2::LEFT_CENTER, &row.sub, sub_font, MUTED());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            self.kc.sel = Some(row.id);
            self.kc.tab = row.identity as usize;
        }
        if resp.double_clicked() {
            *act = Some(Act::Edit(row.identity, row.id));
        }
    }

    fn keychain_detail(&mut self, ui: &mut egui::Ui, act: &mut Option<Act>) {
        let Some(id) = self.kc.sel else { return };
        let identity = self.kc.tab == 1;
        egui::Frame::new().fill(CARD()).stroke(Stroke::new(1.0, BORDER())).corner_radius(12).inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            if identity {
                let Some(i) = self.vault.identities.iter().find(|x| x.id == id).cloned() else { return };
                detail_header(ui, Icon::User, BLUE(), &i.label, &format!("{}{}", if i.username.is_empty() { "No username".to_string() } else { i.username.clone() }, self.team_tag("identity", id)));
                ui.add_space(6.0);
                section_label(ui, "SIGN-IN");
                let key = i.key.and_then(|k| self.vault.keys.iter().find(|x| x.id == k)).map(|k| k.label.clone());
                let text = match (&key, i.password.is_empty()) {
                    (Some(k), false) => format!("Key {k} and a saved password"),
                    (Some(k), true) => format!("Key {k}"),
                    (None, false) => "A saved password".to_string(),
                    (None, true) => "Nothing saved yet".to_string(),
                };
                ui.label(RichText::new(text).size(12.5).color(TEXT()));
                section_label(ui, "USED BY");
                used_by(ui, &self.identity_used_by(id), "No host uses this identity yet.");
            } else {
                let Some(k) = self.vault.keys.iter().find(|x| x.id == id).cloned() else { return };
                let info = self.key_info(&k);
                let mut sub = info.type_label();
                if k.added != 0 {
                    if !sub.is_empty() {
                        sub.push_str(" · ");
                    }
                    sub.push_str(&format!("{} {}", if k.generated { "Generated" } else { "Imported" }, key_info::date(k.added)));
                }
                sub.push_str(&self.team_tag("key", id));
                let warn = info.weak || info.protected == Some(false);
                detail_header(ui, Icon::Key, if warn { AMBER() } else { BLUE() }, &k.label, &sub);
                ui.add_space(6.0);
                let mut notes: Vec<&str> = vec![];
                if info.protected == Some(false) {
                    notes.push("This key has no passphrase. Anyone who gets the file can use it. You can add one with ssh-keygen -p.");
                }
                if info.weak {
                    notes.push("This key is too short to be safe. Make a new one, and use ED25519 or RSA of 3072 bits or more.");
                }
                if info.protected.is_none() && !k.path.trim().is_empty() {
                    notes.push("The key file could not be read from its path.");
                }
                for n in notes {
                    warning_box(ui, n);
                }
                section_label(ui, "FINGERPRINT");
                if info.fingerprint.is_empty() {
                    ui.label(RichText::new("Not available: there is no public key for this key.").size(12.0).color(MUTED()));
                } else {
                    ui.add(egui::Label::new(RichText::new(&info.fingerprint).monospace().size(12.0).color(TEXT())).wrap());
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    section_label(ui, "PUBLIC KEY");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !info.public.is_empty() && ui.add(egui::Button::new(RichText::new("Copy").size(12.0)).corner_radius(6).min_size(Vec2::new(0.0, 26.0))).clicked() {
                            ui.ctx().copy_text(info.public.clone());
                        }
                    });
                });
                if !info.public.is_empty() {
                    egui::Frame::new().fill(WELL()).stroke(Stroke::new(1.0, BORDER())).corner_radius(8).inner_margin(Margin::same(12)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(RichText::new(&info.public).monospace().size(11.5).color(TEXT())).wrap());
                    });
                }
                ui.add_space(4.0);
                section_label(ui, "USED BY");
                used_by(ui, &self.key_used_by(id), "No host uses this key yet.");
            }
            ui.add_space(6.0);
            let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0, BORDER());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if flat_button(ui, "Edit").clicked() {
                    *act = Some(Act::Edit(identity, id));
                }
                if flat_button(ui, "Rename").clicked() {
                    *act = Some(Act::Rename(identity, id));
                }
                if !identity && flat_button(ui, "Export…").clicked() {
                    *act = Some(Act::Export(id));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if danger_button(ui, "Delete").clicked() {
                        *act = Some(Act::Delete(identity, id));
                    }
                });
            });
        });
    }

    fn keychain_act(&mut self, act: Option<Act>) {
        let Some(act) = act else { return };
        match act {
            Act::NewKey => {
                self.editor = Editor::Key(SshKey::default());
                self.editor_new = true;
            }
            Act::NewIdentity => {
                self.kc.tab = 1;
                self.editor = Editor::Identity(Identity::default());
                self.editor_new = true;
            }
            Act::ImportFile => {
                if self.key_import.is_none() {
                    self.key_import = Some(KeyImport::default());
                }
            }
            Act::Edit(identity, id) => {
                if identity {
                    if let Some(i) = self.vault.identities.iter().find(|x| x.id == id) {
                        self.editor = Editor::Identity(i.clone());
                    }
                } else if let Some(k) = self.vault.keys.iter().find(|x| x.id == id) {
                    self.editor = Editor::Key(k.clone());
                }
                self.editor_new = false;
            }
            Act::Rename(identity, id) => {
                let name = if identity { self.vault.identities.iter().find(|x| x.id == id).map(|x| x.label.clone()) } else { self.vault.keys.iter().find(|x| x.id == id).map(|x| x.label.clone()) };
                self.kc.rename = name.map(|n| (identity, id, n));
            }
            Act::Export(id) => {
                if let Some(k) = self.vault.keys.iter().find(|x| x.id == id) {
                    let name: String = k.label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
                    self.kc.export = Some(KeyExport { id, private: false, dest: format!("~/Downloads/{name}.pub") });
                }
            }
            Act::Delete(identity, id) => self.confirm = Some(if identity { Del::Identity(id) } else { Del::Key(id) }),
        }
    }

    /// The Rename and Export windows.
    fn keychain_dialogs(&mut self, ctx: &egui::Context) {
        if let Some((identity, id, mut name)) = self.kc.rename.take() {
            let (mut ok, mut cancel) = (false, false);
            let (_, closed) = app_modal(ctx, "keychain_rename", "Rename", |ui| {
                ui.set_width(340.0);
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).margin(Vec2::new(8.0, 7.0)));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ok = true;
                }
                modal_buttons(ui, |ui| {
                    if primary_button(ui, "Rename").clicked() {
                        ok = true;
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if ok && !name.trim().is_empty() {
                let n = name.trim().to_string();
                if identity {
                    if let Some(x) = self.vault.identities.iter_mut().find(|x| x.id == id) {
                        x.label = n;
                    }
                } else if let Some(x) = self.vault.keys.iter_mut().find(|x| x.id == id) {
                    x.label = n;
                }
                self.dirty = true;
            } else if !(cancel || closed || ok) {
                self.kc.rename = Some((identity, id, name));
            }
        }
        if let Some(mut ex) = self.kc.export.take() {
            let (mut go, mut cancel) = (false, false);
            let (_, closed) = app_modal(ctx, "keychain_export", "Export key", |ui| {
                ui.set_width(400.0);
                label_muted(ui, "What to save");
                ui.horizontal(|ui| {
                    if ui.radio(!ex.private, "Public key").clicked() && ex.private {
                        ex.private = false;
                        if !ex.dest.ends_with(".pub") {
                            ex.dest.push_str(".pub");
                        }
                    }
                    if ui.radio(ex.private, "Private key").clicked() && !ex.private {
                        ex.private = true;
                        if let Some(s) = ex.dest.strip_suffix(".pub") {
                            ex.dest = s.to_string();
                        }
                    }
                });
                if ex.private {
                    ui.add_space(4.0);
                    ui.label(RichText::new("The private key lets anyone sign in as you. Keep the copy somewhere safe.").size(12.0).color(AMBER()));
                }
                ui.add_space(6.0);
                text_field(ui, "Save as", &mut ex.dest, "~/Downloads/key.pub", false);
                modal_buttons(ui, |ui| {
                    if primary_button(ui, "Export").clicked() {
                        go = true;
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if go {
                match self.export_key(&ex) {
                    Ok(msg) => self.notify(msg, GREEN()),
                    Err(e) => {
                        self.notify(e, RED());
                        self.kc.export = Some(ex);
                    }
                }
            } else if !(cancel || closed) {
                self.kc.export = Some(ex);
            }
        }
    }

    fn export_key(&mut self, ex: &KeyExport) -> Result<String, String> {
        let k = self.vault.keys.iter().find(|x| x.id == ex.id).cloned().ok_or("The key is gone")?;
        let dest = std::path::PathBuf::from(expand_home(ex.dest.trim()));
        if ex.dest.trim().is_empty() {
            return Err("Choose where to save the file".into());
        }
        if dest.exists() {
            return Err("A file with that name already exists. Choose another name.".into());
        }
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create the folder: {e}"))?;
        }
        if ex.private {
            let bytes = std::fs::read(expand_home(&k.path)).map_err(|e| format!("Cannot read the key file: {e}"))?;
            std::fs::write(&dest, bytes).map_err(|e| format!("Cannot save the file: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600));
            }
        } else {
            let info = key_info::inspect(&k.path, &k.public_key);
            if info.public.is_empty() {
                return Err("This key has no public key to export".into());
            }
            std::fs::write(&dest, format!("{}\n", info.public)).map_err(|e| format!("Cannot save the file: {e}"))?;
        }
        Ok(format!("Saved {}", dest.display()))
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).strong().color(MUTED()));
}

fn detail_header(ui: &mut egui::Ui, icon: Icon, accent: Color32, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
        ui.painter().rect_filled(r, 12, accent.linear_multiply(0.16));
        crate::icons::paint(ui.painter(), r.center(), 21.0, icon, accent);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(title).size(18.0).strong().color(TEXT())).truncate());
            ui.label(RichText::new(sub).size(12.5).color(MUTED()));
        });
    });
}

fn warning_box(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().fill(AMBER().linear_multiply(0.12)).stroke(Stroke::new(1.0, AMBER().linear_multiply(0.45))).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            crate::icons::paint(ui.painter(), r.center() + Vec2::new(0.0, 2.0), 16.0, Icon::Alert, AMBER());
            ui.add(egui::Label::new(RichText::new(text).size(12.5).color(pick(0xF0D59A, 0x7A4A06))).wrap());
        });
    });
}

/// Chips for the hosts (and identities) that use something.
fn used_by(ui: &mut egui::Ui, items: &[(String, bool)], none: &str) {
    if items.is_empty() {
        ui.label(RichText::new(none).size(12.5).color(MUTED()));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for (label, identity) in items {
            let g = ui.painter().layout_no_wrap(label.clone(), FontId::proportional(12.5), TEXT());
            let (r, _) = ui.allocate_exact_size(Vec2::new(g.size().x + 30.0, 26.0), Sense::hover());
            ui.painter().rect_filled(r, 13, PILL());
            ui.painter().circle_filled(Pos2::new(r.left() + 12.0, r.center().y), 3.0, if *identity { BLUE() } else { GREEN() });
            ui.painter().galley(Pos2::new(r.left() + 22.0, r.center().y - g.size().y / 2.0), g, TEXT());
        }
    });
}

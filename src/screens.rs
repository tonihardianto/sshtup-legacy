//! The pages of the main window and the edit panel that slides in on the right.

use crate::app::{expand_home, App, Del, Editor, Screen};
use crate::model::*;
use crate::schemes::SCHEMES;
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, FontId, Frame, Margin, Pos2, Rect, RichText, Sense, Vec2};

/// The "Import from file" window of the Keychain.
#[derive(Default)]
pub struct KeyImport {
    pub path: String,
    pub label: String,
    pub passphrase: String,
    pub picking: Option<std::sync::mpsc::Receiver<Option<String>>>,
}

/// Asks the system for a file with its own open dialog (blocks; run it on a thread).
fn pick_file_dialog() -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let out = if cfg!(target_os = "macos") {
        let ssh = format!("{home}/.ssh");
        let loc = if std::path::Path::new(&ssh).is_dir() { format!(" default location (POSIX file \"{ssh}\")") } else { String::new() };
        std::process::Command::new("osascript").args(["-e", &format!("POSIX path of (choose file with prompt \"Choose a private key file\"{loc} with invisibles)")]).output().ok()?
    } else if cfg!(target_os = "windows") {
        let ps = "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.OpenFileDialog; if ($d.ShowDialog() -eq 'OK') { $d.FileName }";
        std::process::Command::new("powershell").args(["-NoProfile", "-Command", ps]).output().ok()?
    } else {
        match std::process::Command::new("zenity").args(["--file-selection", "--title=Choose a private key file"]).output() {
            Ok(o) => o,
            Err(_) => std::process::Command::new("kdialog").arg("--getopenfilename").arg(format!("{home}/.ssh")).output().ok()?,
        }
    };
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub struct KhRow {
    pub host: String,
    pub kind: String,
    pub fingerprint: String,
}

enum EdAct {
    None,
    Save,
    Cancel,
    Connect,
    GenKey,
}

enum HAct {
    Sftp(u64),
    Pin(u64),
    Connect(u64),
    Edit(u64),
    NewHost,
    Dup(u64),
    Del(u64),
    OpenGroup(Option<u64>),
    EditGroup(u64),
    NewGroup,
    Quick(String),
    ImportConfig,
    LocalTerminal,
    JoinShared,
}

/// A clickable card with a badge, a title, a sub line and an optional hot spot on the right.
#[allow(clippy::too_many_arguments)]
pub(crate) fn item_card(ui: &mut egui::Ui, size: Vec2, title: &str, sub: &str, glyph: &str, color: Color32, dot: Option<Color32>, right: &str, right_w: f32, selected: bool) -> (egui::Response, egui::Response) {
    let c = card(ui, size, selected);
    let p = ui.painter().clone();
    let bs = (size.y - 16.0).min(40.0);
    let badge_rect = Rect::from_min_size(c.rect.min + Vec2::new(12.0, (size.y - bs) / 2.0), Vec2::splat(bs));
    badge(&p, badge_rect, glyph, color);
    if let Some(d) = dot {
        p.circle_filled(Pos2::new(badge_rect.right() - 1.0, badge_rect.top() + 1.0), 5.5, CARD());
        status_dot(&p, Pos2::new(badge_rect.right() - 1.0, badge_rect.top() + 1.0), d);
    }
    let tx = badge_rect.right() + 12.0;
    let right_x = c.rect.right() - right_w - 14.0;
    let clip = Rect::from_min_max(Pos2::new(tx, c.rect.top()), Pos2::new(right_x.max(tx + 10.0), c.rect.bottom()));
    let pc = p.with_clip_rect(clip);
    if sub.is_empty() {
        pc.text(Pos2::new(tx, c.rect.center().y), Align2::LEFT_CENTER, title, FontId::proportional(14.0), TEXT());
    } else {
        pc.text(Pos2::new(tx, c.rect.center().y - 8.0), Align2::LEFT_CENTER, title, FontId::proportional(14.0), TEXT());
        pc.text(Pos2::new(tx, c.rect.center().y + 9.0), Align2::LEFT_CENTER, sub, FontId::proportional(11.5), MUTED());
    }
    let rr = Rect::from_center_size(Pos2::new(c.rect.right() - 10.0 - right_w / 2.0, c.rect.center().y), Vec2::new(right_w, 26.0));
    let rresp = ui.interact(rr, c.resp.id.with("right"), Sense::click());
    if !right.is_empty() {
        if rresp.hovered() {
            p.rect_filled(rr, 6.0, BORDER());
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        p.text(rr.center(), Align2::CENTER_CENTER, right, FontId::proportional(12.5), if rresp.hovered() { TEXT() } else { MUTED() });
    }
    (c.resp, rresp)
}

fn combo_id(ui: &mut egui::Ui, salt: &str, cur: &mut Option<u64>, items: &[(u64, String)], none: &str) {
    let text = cur.and_then(|c| items.iter().find(|i| i.0 == c)).map(|i| i.1.clone()).unwrap_or_else(|| none.to_string());
    egui::ComboBox::from_id_salt(salt).width(ui.available_width()).selected_text(text).show_ui(ui, |ui| {
        ui.selectable_value(cur, None, none);
        for (id, l) in items {
            ui.selectable_value(cur, Some(*id), l.clone());
        }
    });
}

pub(crate) fn page_header(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(22.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
    ui.add_space(8.0);
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(18.0).strong());
        ui.add_space(4.0);
        ui.label(RichText::new(text).size(13.0).color(MUTED()));
    });
}

fn parse_tags(s: &str) -> Vec<String> {
    s.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if t.is_empty() { "key".into() } else { t }
}

/// The right-hand panel the form lives in; it slides in and out.
fn editor_panel_shell() -> egui::Panel {
    egui::Panel::right("editor").exact_size(340.0).resizable(false).frame(Frame::new().fill(CARD()).inner_margin(Margin::same(16)).stroke(egui::Stroke::new(1.0, BORDER())))
}

fn editor_item_id(ed: &Editor) -> u64 {
    match ed {
        Editor::Host(x) => x.id,
        Editor::Group(x) => x.id,
        Editor::Key(x) => x.id,
        Editor::Identity(x) => x.id,
        Editor::Forward(x) => x.id,
        Editor::Snippet(x) => x.id,
        Editor::None => 0,
    }
}

/// The kind used in the sync state's item ids.
fn editor_kind(ed: &Editor) -> Option<&'static str> {
    Some(match ed {
        Editor::Host(_) => "host",
        Editor::Group(_) => "group",
        Editor::Key(_) => "key",
        Editor::Identity(_) => "identity",
        Editor::Forward(_) => "forward",
        Editor::Snippet(_) => "snippet",
        Editor::None => return None,
    })
}

/// The line under the form title: what is being edited.
fn editor_subtitle(ed: &Editor, is_new: bool) -> String {
    if is_new {
        return "Not saved yet".into();
    }
    match ed {
        Editor::Host(h) => format!("{} · {}", h.display_label(), if h.protocol == Protocol::Ssh { "SSH" } else { "Telnet" }),
        Editor::Group(g) => g.name.clone(),
        Editor::Key(k) => k.label.clone(),
        Editor::Identity(i) => i.label.clone(),
        Editor::Forward(f) => f.label.clone(),
        Editor::Snippet(s) => s.display_label(),
        Editor::None => String::new(),
    }
}

/// True once Save was pressed with a required field empty, so fields start showing their errors.
fn tried_save(ui: &egui::Ui) -> bool {
    ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("editor-tried-save"))).unwrap_or(false)
}

/// A form can't be saved while one of its required fields is empty.
fn missing_required(ed: &Editor) -> bool {
    match ed {
        Editor::Host(h) => h.address.trim().is_empty(),
        Editor::Group(g) => g.name.trim().is_empty(),
        Editor::Key(k) => k.label.trim().is_empty(),
        Editor::Identity(i) => i.label.trim().is_empty(),
        Editor::Forward(f) => f.host.is_none(),
        Editor::Snippet(s) => s.script.trim().is_empty(),
        Editor::None => false,
    }
}

impl App {
    // ------------------------------------------------------- edit helpers --

    pub fn edit_host(&mut self, h: Host, is_new: bool) {
        self.edit_a = h.port.map(|p| p.to_string()).unwrap_or_default();
        self.edit_b = h.tags.join(", ");
        self.editor = Editor::Host(h);
        self.editor_new = is_new;
    }

    fn gen_key(&mut self, k: &mut SshKey) {
        let path = if k.path.trim().is_empty() {
            let d = data_dir().join("keys");
            let _ = std::fs::create_dir_all(&d);
            d.join(sanitize(&k.label)).to_string_lossy().to_string()
        } else {
            expand_home(&k.path)
        };
        if std::path::Path::new(&path).exists() {
            self.notify("A file with that path already exists - choose another path".into(), AMBER());
            return;
        }
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let out = std::process::Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-f", &path, "-N", &k.passphrase, "-C", &k.label]).output();
        match out {
            Ok(o) if o.status.success() => {
                k.path = path.clone();
                k.generated = true;
                k.added = now_secs();
                k.public_key = std::fs::read_to_string(format!("{}.pub", path)).unwrap_or_default().trim().to_string();
                self.notify("New key created".into(), GREEN());
            }
            Ok(o) => self.notify(format!("ssh-keygen failed: {}", String::from_utf8_lossy(&o.stderr).trim()), RED()),
            Err(e) => self.notify(format!("Could not run ssh-keygen: {}", e), RED()),
        }
    }

    fn commit_editor(&mut self, ed: Editor) -> Option<u64> {
        let new = self.editor_new;
        if !new && editor_kind(&ed).map(|k| self.item_read_only(k, editor_item_id(&ed))).unwrap_or(false) {
            self.notify("You are a viewer in this team and cannot change its items.".into(), AMBER());
            return None;
        }
        let (a, b) = (self.edit_a.clone(), self.edit_b.clone());
        match ed {
            Editor::None => None,
            Editor::Host(mut h) => {
                h.port = a.trim().parse().ok();
                h.tags = parse_tags(&b);
                if new {
                    h.id = self.vault.new_id();
                    self.vault.hosts.push(h.clone());
                    self.sync_place_new("host", h.id);
                } else if let Some(x) = self.vault.hosts.iter_mut().find(|x| x.id == h.id) {
                    *x = h.clone();
                }
                self.dirty = true;
                Some(h.id)
            }
            Editor::Group(mut g) => {
                if new {
                    g.id = self.vault.new_id();
                    self.vault.groups.push(g.clone());
                    self.sync_place_new("group", g.id);
                } else if let Some(x) = self.vault.groups.iter_mut().find(|x| x.id == g.id) {
                    *x = g.clone();
                }
                self.dirty = true;
                Some(g.id)
            }
            Editor::Key(mut k) => {
                if new {
                    if k.added == 0 {
                        k.added = now_secs();
                    }
                    k.id = self.vault.new_id();
                    self.vault.keys.push(k.clone());
                    self.sync_place_new("key", k.id);
                } else if let Some(x) = self.vault.keys.iter_mut().find(|x| x.id == k.id) {
                    *x = k.clone();
                }
                self.dirty = true;
                Some(k.id)
            }
            Editor::Identity(mut i) => {
                if new {
                    i.id = self.vault.new_id();
                    self.vault.identities.push(i.clone());
                    self.sync_place_new("identity", i.id);
                } else if let Some(x) = self.vault.identities.iter_mut().find(|x| x.id == i.id) {
                    *x = i.clone();
                }
                self.dirty = true;
                Some(i.id)
            }
            Editor::Forward(mut f) => {
                if new {
                    f.id = self.vault.new_id();
                    self.vault.forwards.push(f.clone());
                    self.sync_place_new("forward", f.id);
                } else if let Some(x) = self.vault.forwards.iter_mut().find(|x| x.id == f.id) {
                    *x = f.clone();
                }
                self.dirty = true;
                Some(f.id)
            }
            Editor::Snippet(mut s) => {
                if new {
                    s.id = self.vault.new_id();
                    self.vault.snippets.push(s.clone());
                    self.sync_place_new("snippet", s.id);
                } else if let Some(x) = self.vault.snippets.iter_mut().find(|x| x.id == s.id) {
                    *x = s.clone();
                }
                self.dirty = true;
                Some(s.id)
            }
        }
    }

    // --------------------------------------------------------- edit panel --

    pub fn editor_panel(&mut self, ui: &mut egui::Ui) {
        let snap_id = egui::Id::new("editor-snapshot");
        let live = !matches!(self.editor, Editor::None);
        // While the panel slides shut the form is already gone, so it is drawn from the last frame it had.
        let (mut ed, is_new) = if live {
            (std::mem::replace(&mut self.editor, Editor::None), self.editor_new)
        } else {
            self.sync_rt.ui.new_scope = None;
            match ui.ctx().data(|d| d.get_temp::<(Editor, bool)>(snap_id)) {
                Some(s) => s,
                None => {
                    // closed: still ask the panel every frame, so its next opening has something to slide from
                    let mut open = false;
                    editor_panel_shell().show_collapsible(ui, &mut open, |_| {});
                    return;
                }
            }
        };
        let mut act = EdAct::None;
        if live {
            ui.ctx().data_mut(|d| d.insert_temp(snap_id, (ed.clone(), is_new)));
            // a different form than last frame: forget any "tried to save" error state
            use std::hash::{Hash, Hasher};
            let mut hs = std::collections::hash_map::DefaultHasher::new();
            std::mem::discriminant(&ed).hash(&mut hs);
            editor_item_id(&ed).hash(&mut hs);
            is_new.hash(&mut hs);
            let key = hs.finish();
            let key_id = egui::Id::new("editor-key");
            let ctx = ui.ctx();
            if ctx.data(|d| d.get_temp::<u64>(key_id)) != Some(key) {
                ctx.data_mut(|d| {
                    d.insert_temp(key_id, key);
                    d.insert_temp(egui::Id::new("editor-tried-save"), false);
                });
            }
        }
        let read_only = !is_new && editor_kind(&ed).map(|k| self.item_read_only(k, editor_item_id(&ed))).unwrap_or(false);
        let (title, can_connect) = match &ed {
            Editor::Host(_) => (if is_new { "New Host" } else { "Edit Host" }, true),
            Editor::Group(_) => (if is_new { "New Group" } else { "Edit Group" }, false),
            Editor::Key(_) => (if is_new { "New Key" } else { "Edit Key" }, false),
            Editor::Identity(_) => (if is_new { "New Identity" } else { "Edit Identity" }, false),
            Editor::Forward(_) => (if is_new { "New Port Forwarding" } else { "Edit Port Forwarding" }, false),
            Editor::Snippet(_) => (if is_new { "New Snippet" } else { "Edit Snippet" }, false),
            Editor::None => ("", false),
        };
        let mut expanded = live;
        let prev_anim = ui.style().animation_time;
        ui.style_mut().animation_time = 0.3;
        let shown = editor_panel_shell().show_collapsible(ui, &mut expanded, |ui| {
                ui.horizontal(|ui| {
                    let (tile, _) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                    ui.painter().rect_filled(tile, 8.0, BLUE().linear_multiply(0.18));
                    let tile_icon = match &ed {
                        Editor::Host(_) | Editor::None => crate::icons::Icon::Host,
                        Editor::Group(_) => crate::icons::Icon::Group,
                        Editor::Key(_) => crate::icons::Icon::Key,
                        Editor::Identity(_) => crate::icons::Icon::User,
                        Editor::Forward(_) => crate::icons::Icon::Forward,
                        Editor::Snippet(_) => crate::icons::Icon::Snippet,
                    };
                    crate::icons::paint(ui.painter(), tile.center(), 17.0, tile_icon, BLUE());
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(RichText::new(title).size(15.0).strong());
                        ui.label(RichText::new(editor_subtitle(&ed, is_new)).size(11.5).color(MUTED()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (cr, cresp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                        if cresp.hovered() {
                            ui.painter().rect_filled(cr, 6.0, BORDER());
                        }
                        crate::icons::paint(ui.painter(), cr.center(), 12.0, crate::icons::Icon::Close, MUTED());
                        if cresp.clicked() {
                            act = EdAct::Cancel;
                        }
                    });
                });
                ui.add_space(6.0);
                egui::Panel::bottom("editor_buttons").show_separator_line(false).resizable(false).frame(Frame::new().inner_margin(Margin { top: 8, ..Default::default() })).show(ui, |ui| {
                    ui.separator();
                    ui.add_space(2.0);
                    // right-aligned: Save is the rightmost (primary), Save & connect sits to its left
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if read_only {
                            // a viewer may open the host but not change anything
                            if can_connect && primary_button(ui, "Connect").clicked() {
                                act = EdAct::Connect;
                            } else if !can_connect && primary_button(ui, "Close").clicked() {
                                act = EdAct::Cancel;
                            }
                        } else {
                            if primary_button(ui, "Save").clicked() {
                                act = EdAct::Save;
                            }
                            if can_connect && flat_button(ui, "Save & connect").clicked() {
                                act = EdAct::Connect;
                            }
                        }
                    });
                });
                egui::ScrollArea::vertical().auto_shrink(false).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_enabled_ui(!read_only, |ui| self.editor_fields(ui, &mut ed, &mut act));
                    if is_new {
                        match &ed {
                            Editor::Host(x) => self.sync_new_scope_row(ui, "host", x.group),
                            Editor::Group(x) => self.sync_new_scope_row(ui, "group", x.parent),
                            Editor::Key(_) => self.sync_new_scope_row(ui, "key", None),
                            Editor::Identity(_) => self.sync_new_scope_row(ui, "identity", None),
                            Editor::Forward(_) => self.sync_new_scope_row(ui, "forward", None),
                            Editor::Snippet(_) => self.sync_new_scope_row(ui, "snippet", None),
                            Editor::None => {}
                        }
                    } else {
                        match &ed {
                            Editor::Host(x) => self.sync_share_row(ui, "host", x.id),
                            Editor::Group(x) => self.sync_share_row(ui, "group", x.id),
                            Editor::Key(x) => self.sync_share_row(ui, "key", x.id),
                            Editor::Identity(x) => self.sync_share_row(ui, "identity", x.id),
                            Editor::Forward(x) => self.sync_share_row(ui, "forward", x.id),
                            Editor::Snippet(x) => self.sync_share_row(ui, "snippet", x.id),
                            Editor::None => {}
                        }
                    }
                });
        });
        ui.style_mut().animation_time = prev_anim;
        if shown.is_none() {
            // not on screen yet (just opened) or finished closing
            if live {
                self.editor = ed;
            } else {
                ui.ctx().data_mut(|d| d.remove::<(Editor, bool)>(snap_id));
            }
            return;
        }
        if !live {
            return;
        }
        let tried = egui::Id::new("editor-tried-save");
        let ctx = ui.ctx().clone();
        // a required field is empty: keep the form open and let the field show its error
        if matches!(act, EdAct::Save | EdAct::Connect) && missing_required(&ed) {
            ctx.data_mut(|d| d.insert_temp(tried, true));
            act = EdAct::None;
        }
        match act {
            EdAct::None => self.editor = ed,
            EdAct::Cancel => {
                ctx.data_mut(|d| d.insert_temp(tried, false));
            }
            EdAct::Save => {
                ctx.data_mut(|d| d.insert_temp(tried, false));
                self.commit_editor(ed);
            }
            EdAct::Connect if read_only => {
                if let Editor::Host(h) = &ed {
                    self.connect_host(h.id);
                }
                self.editor = Editor::None;
            }
            EdAct::Connect => {
                ctx.data_mut(|d| d.insert_temp(tried, false));
                if let Some(id) = self.commit_editor(ed) {
                    self.connect_host(id);
                }
            }
            EdAct::GenKey => {
                if let Editor::Key(k) = &mut ed {
                    let mut kk = k.clone();
                    self.gen_key(&mut kk);
                    *k = kk;
                }
                self.editor = ed;
            }
        }
    }

    fn editor_fields(&mut self, ui: &mut egui::Ui, ed: &mut Editor, act: &mut EdAct) {
        let keys: Vec<(u64, String)> = self.vault.keys.iter().map(|k| (k.id, k.label.clone())).collect();
        let idents: Vec<(u64, String)> = self.vault.identities.iter().map(|k| (k.id, k.label.clone())).collect();
        let groups: Vec<(u64, String)> = self.vault.groups.iter().map(|g| (g.id, g.name.clone())).collect();
        let snippets: Vec<(u64, String)> = self.vault.snippets.iter().map(|s| (s.id, s.display_label())).collect();
        let hosts: Vec<(u64, String)> = self.vault.hosts.iter().map(|h| (h.id, h.display_label())).collect();
        match ed {
            Editor::Host(h) => {
                use crate::icons::Icon;
                let tried = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("editor-tried-save"))).unwrap_or(false);
                form_section(ui, "General");
                text_field_ex(ui, "Label", false, None, Some(Icon::Pencil), &mut h.label, "My server", false);
                let addr_err = (tried && h.address.trim().is_empty()).then_some("Enter a hostname or IP address.");
                text_field_ex(ui, "Address", true, addr_err, Some(Icon::Host), &mut h.address, "Hostname or IP address", false);
                let hint = h.effective_port().to_string();
                let edit_a = &mut self.edit_a;
                split_row(
                    ui,
                    Some(84.0),
                    |ui| {
                        field(ui, "Protocol", |ui| {
                            egui::ComboBox::from_id_salt("proto").width(ui.available_width()).selected_text(if h.protocol == Protocol::Ssh { "SSH" } else { "Telnet" }).show_ui(ui, |ui| {
                                ui.selectable_value(&mut h.protocol, Protocol::Ssh, "SSH");
                                ui.selectable_value(&mut h.protocol, Protocol::Telnet, "Telnet");
                            });
                        });
                    },
                    |ui| text_field(ui, "Port", edit_a, &hint, false),
                );
                field(ui, "Group", |ui| combo_id(ui, "hgroup", &mut h.group, &groups, "(none)"));

                form_section(ui, "Authentication");
                text_field_ex(ui, "Username", false, None, Some(Icon::User), &mut h.username, "root", false);
                text_field_ex(ui, "Password", false, None, Some(Icon::Lock), &mut h.password, "Optional", true);
                if h.protocol == Protocol::Ssh {
                    split_row(
                        ui,
                        None,
                        |ui| field(ui, "Key", |ui| combo_id(ui, "hkey", &mut h.key, &keys, "(none)")),
                        |ui| field(ui, "Identity", |ui| combo_id(ui, "hid", &mut h.identity, &idents, "(none)")),
                    );
                }

                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new("Advanced").size(12.0).color(MUTED())).id_salt("host-advanced").default_open(false).show_unindented(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if h.protocol == Protocol::Ssh {
                        let jumps: Vec<(u64, String)> = hosts.iter().filter(|x| x.0 != h.id).cloned().collect();
                        field(ui, "Jump host", |ui| combo_id(ui, "hjump", &mut h.jump_host, &jumps, "(none)"));
                        field(ui, "Run snippet after connecting", |ui| combo_id(ui, "hsn", &mut h.startup_snippet, &snippets, "(none)"));
                        ui.horizontal(|ui| {
                            toggle(ui, &mut h.agent_forwarding);
                            ui.label("Agent forwarding");
                        });
                        ui.add_space(4.0);
                    }
                    text_field(ui, "Tags (comma separated)", &mut self.edit_b, "web, prod", false);
                    field(ui, "Color scheme", |ui| {
                        let cur = h.color_scheme.clone().unwrap_or_else(|| "(app default)".into());
                        egui::ComboBox::from_id_salt("hscheme").width(ui.available_width()).selected_text(cur).show_ui(ui, |ui| {
                            ui.selectable_value(&mut h.color_scheme, None, "(app default)");
                            for s in SCHEMES {
                                ui.selectable_value(&mut h.color_scheme, Some(s.name.to_string()), s.name);
                            }
                        });
                    });
                });
            }
            Editor::Group(g) => {
                let tried = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("editor-tried-save"))).unwrap_or(false);
                let err = (tried && g.name.trim().is_empty()).then_some("Enter a group name.");
                text_field_ex(ui, "Name", true, err, Some(crate::icons::Icon::Pencil), &mut g.name, "Group name", false);
                let others: Vec<(u64, String)> = groups.iter().filter(|x| x.0 != g.id).cloned().collect();
                field(ui, "Parent group", |ui| combo_id(ui, "gparent", &mut g.parent, &others, "(top level)"));
            }
            Editor::Key(k) => {
                use crate::icons::Icon;
                let tried = tried_save(ui);
                if self.editor_new {
                    if let Some(p) = key_drop_zone(ui) {
                        if k.label.trim().is_empty() {
                            k.label = p.rsplit('/').next().unwrap_or("").to_string();
                        }
                        k.path = p;
                        k.public_key.clear();
                    }
                    ui.add_space(2.0);
                }
                form_section(ui, "Key");
                let err = (tried && k.label.trim().is_empty()).then_some("Enter a label.");
                text_field_ex(ui, "Label", true, err, Some(Icon::Pencil), &mut k.label, "id_ed25519", false);
                text_field_ex(ui, "Private key file", false, None, Some(Icon::File), &mut k.path, "~/.ssh/id_ed25519", false);
                text_field_ex(ui, "Passphrase", false, None, Some(Icon::Lock), &mut k.passphrase, "Optional", true);
                ui.add_space(4.0);
                ui.checkbox(&mut k.sync_contents, "Also sync this key file to my other devices (encrypted)");
                ui.add_space(4.0);
                if flat_button(ui, "Generate new ed25519 key").on_hover_text("Creates the key file with ssh-keygen using the path and passphrase above").clicked() {
                    *act = EdAct::GenKey;
                }
                if k.public_key.is_empty() && !k.path.is_empty() {
                    if let Ok(s) = std::fs::read_to_string(format!("{}.pub", expand_home(&k.path))) {
                        k.public_key = s.trim().to_string();
                    }
                }
                if !k.public_key.is_empty() {
                    form_section(ui, "Public key");
                    egui::Frame::new().fill(BG_WINDOW()).stroke(egui::Stroke::new(1.0, BORDER())).corner_radius(6).inner_margin(Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(RichText::new(&k.public_key).monospace().size(10.5)).wrap());
                    });
                    if flat_button(ui, "Copy public key").clicked() {
                        ui.ctx().copy_text(k.public_key.clone());
                    }
                }
            }
            Editor::Identity(i) => {
                use crate::icons::Icon;
                let tried = tried_save(ui);
                form_section(ui, "Identity");
                let err = (tried && i.label.trim().is_empty()).then_some("Enter a label.");
                text_field_ex(ui, "Label", true, err, Some(Icon::Pencil), &mut i.label, "Work account", false);
                form_section(ui, "Authentication");
                text_field_ex(ui, "Username", false, None, Some(Icon::User), &mut i.username, "root", false);
                text_field_ex(ui, "Password", false, None, Some(Icon::Lock), &mut i.password, "Optional", true);
                field(ui, "Key", |ui| combo_id(ui, "ikey", &mut i.key, &keys, "(none)"));
            }
            Editor::Forward(f) => {
                use crate::icons::Icon;
                let tried = tried_save(ui);
                form_section(ui, "General");
                text_field_ex(ui, "Label", false, None, Some(Icon::Pencil), &mut f.label, "Optional", false);
                field(ui, "Type", |ui| {
                    egui::ComboBox::from_id_salt("ftype").width(ui.available_width()).selected_text(f.kind.name()).show_ui(ui, |ui| {
                        for t in [PfType::Local, PfType::Remote, PfType::Dynamic] {
                            ui.selectable_value(&mut f.kind, t, t.name());
                        }
                    });
                });
                field_label(ui, "Host", true);
                combo_id(ui, "fhost", &mut f.host, &hosts, "(choose a host)");
                if tried && f.host.is_none() {
                    ui.label(RichText::new("Choose a host.").size(11.5).color(RED()));
                }
                form_section(ui, if f.kind == PfType::Remote { "Listen on the server" } else { "Listen on this device" });
                let port_w = 84.0;
                split_row(
                    ui,
                    Some(port_w),
                    |ui| text_field_ex(ui, "Bind address", false, None, Some(Icon::Host), &mut f.bound_address, "127.0.0.1", false),
                    |ui| field(ui, "Port", |ui| {
                        ui.add_sized([ui.available_width(), 34.0], egui::DragValue::new(&mut f.local_port).range(0..=65535));
                    }),
                );
                if f.kind != PfType::Dynamic {
                    form_section(ui, "Forward to");
                    split_row(
                        ui,
                        Some(port_w),
                        |ui| text_field_ex(ui, "Destination host", false, None, Some(Icon::Host), &mut f.hostname, "localhost", false),
                        |ui| field(ui, "Port", |ui| {
                            ui.add_sized([ui.available_width(), 34.0], egui::DragValue::new(&mut f.remote_port).range(0..=65535));
                        }),
                    );
                }
            }
            Editor::Snippet(s) => {
                use crate::icons::Icon;
                let tried = tried_save(ui);
                form_section(ui, "Snippet");
                text_field_ex(ui, "Label", false, None, Some(Icon::Pencil), &mut s.label, "Optional", false);
                field_label(ui, "Script", true);
                let script_missing = tried && s.script.trim().is_empty();
                multiline_box(ui, egui::Id::new("snippet-script"), &mut s.script, "ls -la", 8, script_missing);
                if script_missing {
                    ui.label(RichText::new("Enter a script.").size(11.5).color(RED()));
                }
                ui.add_space(2.0);
                text_field_ex(ui, "Package", false, None, Some(Icon::Folder), &mut s.package, "Optional", false);
            }
            Editor::None => {}
        }
    }

    // -------------------------------------------------------------- hosts --

    pub fn screen_hosts(&mut self, ui: &mut egui::Ui) {
        let mut acts: Vec<HAct> = vec![];
        let q = self.search.trim().to_lowercase();
        // one row: search, grid/list switch, and New Host with its menu
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
            self.focus_search = true;
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let w = (ui.available_width() - 216.0).max(180.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 38.0), Sense::hover());
            let hint = if cfg!(target_os = "macos") { "Cmd K" } else { "Ctrl K" };
            let hint_galley = ui.painter().layout_no_wrap(hint.into(), FontId::monospace(11.5), MUTED());
            let hint_w = hint_galley.size().x + 14.0;
            let inner = Rect::from_min_max(rect.min + Vec2::new(40.0, 0.0), rect.max - Vec2::new(hint_w + 18.0, 0.0));
            let search_id = egui::Id::new("hosts-search");
            let focused = ui.memory(|m| m.has_focus(search_id));
            ui.painter().rect(rect, 8.0, CARD(), egui::Stroke::new(if focused { 1.5 } else { 1.0 }, if focused { BLUE() } else { BORDER() }), egui::StrokeKind::Inside);
            let mut edit_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
            let r = edit_ui.add(egui::TextEdit::singleline(&mut self.search).id(egui::Id::new("hosts-search")).frame(Frame::NONE).hint_text("Search by name, user or IP, or type ssh user@host -p 22").desired_width(inner.width()).margin(Vec2::new(0.0, 0.0)).vertical_align(egui::Align::Center).min_size(Vec2::new(0.0, 38.0)));
            if std::mem::take(&mut self.focus_search) {
                r.request_focus();
            }
            crate::icons::paint(ui.painter(), Pos2::new(rect.left() + 20.0, rect.center().y), 15.0, crate::icons::Icon::Search, MUTED());
            let chip = Rect::from_center_size(Pos2::new(rect.right() - 12.0 - hint_w / 2.0, rect.center().y), Vec2::new(hint_w, 22.0));
            ui.painter().rect(chip, 5.0, pick(0x1B1E30, 0xE9ECF2), egui::Stroke::new(1.0, BORDER()), egui::StrokeKind::Inside);
            ui.painter().galley(Pos2::new(chip.left() + 7.0, chip.center().y - hint_galley.size().y / 2.0), hint_galley, MUTED());

            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter && !q.is_empty() {
                let matching: Vec<u64> = self.vault.hosts.iter().filter(|h| h.label.to_lowercase().contains(&q) || h.address.to_lowercase().contains(&q)).map(|h| h.id).collect();
                if matching.len() == 1 {
                    acts.push(HAct::Connect(matching[0]));
                } else if matching.is_empty() {
                    acts.push(HAct::Quick(self.search.trim().to_string()));
                }
            }

            // grid / list switch (the active one is filled)
            let (sw, _) = ui.allocate_exact_size(Vec2::new(70.0, 38.0), Sense::hover());
            ui.painter().rect(sw, 8.0, CARD(), egui::Stroke::new(1.0, BORDER()), egui::StrokeKind::Inside);
            for (i, (icon, grid_mode, tip)) in [(crate::icons::Icon::Group, true, "Grid"), (crate::icons::Icon::List, false, "List")].into_iter().enumerate() {
                let cell = Rect::from_min_size(sw.min + Vec2::new(4.0 + i as f32 * 31.0, 4.0), Vec2::new(30.0, 30.0));
                let on = self.vault.settings.grid_layout == grid_mode;
                let resp = ui.interact(cell, egui::Id::new(("hosts-layout", i)), Sense::click()).on_hover_text(tip);
                if on || resp.hovered() {
                    ui.painter().rect_filled(cell, 6.0, if on { BORDER() } else { CARD_HOVER() });
                }
                crate::icons::paint(ui.painter(), cell.center(), 14.0, icon, if on { TEXT() } else { MUTED() });
                if resp.clicked() && !on {
                    self.vault.settings.grid_layout = grid_mode;
                    self.dirty = true;
                }
            }

            // New Host, with the rarer actions in its menu
            let label = ui.painter().layout_no_wrap("+  New Host".into(), FontId::proportional(13.0), Color32::WHITE);
            ui.spacing_mut().item_spacing.x = 0.0;
            let (main_rect, main) = ui.allocate_exact_size(Vec2::new(label.size().x + 30.0, 38.0), Sense::click());
            let (arrow_rect, arrow) = ui.allocate_exact_size(Vec2::new(32.0, 38.0), Sense::click());
            let ctl = |r: &egui::Response, base: Color32| if r.hovered() { base.linear_multiply(1.15) } else { base };
            ui.painter().rect_filled(main_rect, egui::CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, ctl(&main, BLUE()));
            ui.painter().rect_filled(arrow_rect, egui::CornerRadius { nw: 0, sw: 0, ne: 8, se: 8 }, ctl(&arrow, BLUE()));
            ui.painter().line_segment([arrow_rect.left_top() + Vec2::new(0.0, 7.0), arrow_rect.left_bottom() - Vec2::new(0.0, 7.0)], egui::Stroke::new(1.0, Color32::from_white_alpha(70)));
            ui.painter().galley(Pos2::new(main_rect.left() + 15.0, main_rect.center().y - label.size().y / 2.0), label, Color32::WHITE);
            crate::icons::paint(ui.painter(), arrow_rect.center() + Vec2::new(1.0, 0.0), 12.0, crate::icons::Icon::ChevDown, Color32::WHITE);
            if main.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                acts.push(HAct::NewHost);
            }
            egui::Popup::menu(&arrow.on_hover_cursor(egui::CursorIcon::PointingHand)).gap(6.0).align(egui::RectAlign::BOTTOM_END).show(|ui| {
                ui.set_min_width(190.0);
                ui.spacing_mut().button_padding = Vec2::new(12.0, 7.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                use crate::icons::Icon;
                let entries = [
                    (Icon::FolderPlus, "New Group", HAct::NewGroup),
                    (Icon::Upload, "Import ~/.ssh/config", HAct::ImportConfig),
                ];
                for (icon, text, a) in entries {
                    if icon_text_button(ui, icon, text).clicked() {
                        acts.push(a);
                        ui.close();
                    }
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if icon_text_button(ui, crate::icons::Icon::Terminal, "Local terminal").clicked() {
                acts.push(HAct::LocalTerminal);
            }
            if icon_text_button(ui, crate::icons::Icon::Users, "Join shared session").clicked() {
                acts.push(HAct::JoinShared);
            }
        });
        ui.add_space(8.0);
        self.join_window(ui.ctx());

        // breadcrumb: only inside a group, where it is the way back up (at the top it would be a label with no use)
        if self.cur_group.is_some() {
            ui.horizontal(|ui| {
                if ui.add(egui::Label::new(RichText::new("All hosts").size(13.0).color(BLUE())).sense(Sense::click())).clicked() {
                    acts.push(HAct::OpenGroup(None));
                }
                let path: Vec<(u64, String)> = self.vault.group_path(self.cur_group).iter().map(|g| (g.id, g.name.clone())).collect();
                for (id, name) in path {
                    label_muted(ui, "›");
                    if ui.add(egui::Label::new(RichText::new(name).size(13.0).color(BLUE())).sense(Sense::click())).clicked() {
                        acts.push(HAct::OpenGroup(Some(id)));
                    }
                }
                if q.is_empty() && self.cur_group.is_some() {
                    if let Some(g) = self.cur_group {
                        if ui.small_button("edit group").clicked() {
                            acts.push(HAct::EditGroup(g));
                        }
                    }
                }
            });
            ui.add_space(6.0);
        }

        let looks_quick = !q.is_empty() && (q.contains('@') || q.starts_with("ssh "));
        if looks_quick {
            let (r, _) = item_card(ui, Vec2::new(ui.available_width().min(420.0), 56.0), &format!("Connect to {}", self.search.trim()), "Quick connect", "icon:terminal", GREEN(), None, "", 0.0, false);
            if r.clicked() {
                acts.push(HAct::Quick(self.search.trim().to_string()));
            }
            ui.add_space(8.0);
        }

        let grid = self.vault.settings.grid_layout;
        let live_ids: std::collections::HashSet<u64> = self.tabs.iter().filter(|t| t.sess.is_alive()).filter_map(|t| t.sess.spec.host_id).collect();
        let sort = |v: &mut Vec<Host>, by: &str| match by {
            "Name" => v.sort_by_key(|h| h.display_label().to_lowercase()),
            _ => v.sort_by_key(|h| std::cmp::Reverse(h.last_used)),
        };
        // a pinned host is listed once, in Pinned; searching lists every match in one place
        let mut hosts: Vec<Host> = self
            .vault
            .hosts
            .iter()
            .filter(|h| self.in_vault("host", h.id))
                        .filter(|h| if q.is_empty() { h.group == self.cur_group && !h.pinned } else { h.label.to_lowercase().contains(&q) || h.address.to_lowercase().contains(&q) || h.tags.iter().any(|t| t.to_lowercase().contains(&q)) })
            .cloned()
            .collect();
        sort(&mut hosts, &self.vault.settings.sort_by);
        let mut pinned: Vec<Host> = if q.is_empty() { self.vault.hosts.iter().filter(|h| h.pinned && self.in_vault("host", h.id)).cloned().collect() } else { vec![] };
        sort(&mut pinned, &self.vault.settings.sort_by);
        let groups: Vec<Group> = if q.is_empty() { self.vault.groups.iter().filter(|g| g.parent == self.cur_group && self.in_vault("group", g.id)).cloned().collect() } else { vec![] };

        if hosts.is_empty() && pinned.is_empty() && groups.is_empty() && !looks_quick {
            if self.vault.hosts.is_empty() && self.vault.groups.is_empty() {
                empty_state(ui, "No hosts yet", "Add your first host with \"+ New Host\", or type user@address above and press Enter.");
            } else {
                empty_state(ui, "Nothing here", "No hosts match.");
            }
            self.apply_host_acts(acts);
            return;
        }

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            let avail = ui.available_width();
            let cols = if grid { ((avail + 10.0) / 270.0).floor().max(1.0) } else { 1.0 };
            let w = ((avail - 10.0 * (cols - 1.0)) / cols).floor() - 1.0;
            let size = Vec2::new(w, if grid { 64.0 } else { 54.0 });
            if !pinned.is_empty() {
                section_title(ui, "Pinned");
                self.host_cards(ui, &pinned, w, &live_ids, &mut acts);
            }
            if !groups.is_empty() {
                section_title(ui, "Groups");
                ui.horizontal_wrapped(|ui| {
                    for g in &groups {
                        let n = self.vault.hosts.iter().filter(|h| h.group == Some(g.id)).count();
                        let (c, e) = item_card(ui, size, &g.name, &format!("{} host{}", n, if n == 1 { "" } else { "s" }), "icon:group", icon_color(&g.name), None, "Edit", 40.0, false);
                        c.context_menu(|ui| {
                            if icon_text_button(ui, crate::icons::Icon::Pencil, "Edit").clicked() {
                                acts.push(HAct::EditGroup(g.id));
                                ui.close();
                            }
                            self.group_menu(ui, "group", g.id);
                            self.move_menu(ui, "group", g.id);
                        });
                        if e.clicked() {
                            acts.push(HAct::EditGroup(g.id));
                        } else if c.clicked() {
                            acts.push(HAct::OpenGroup(Some(g.id)));
                        }
                    }
                });
            }
            if !hosts.is_empty() {
                section_title(ui, "Hosts");
                self.host_cards(ui, &hosts, w, &live_ids, &mut acts);
            }
        });
        self.apply_host_acts(acts);
    }

    /// Groups an item can be moved into (not its own group, and for a group not itself or anything inside it).
    pub(crate) fn group_targets(&self, kind: &str, id: u64) -> Vec<(u64, String)> {
        let cur = if kind == "host" { self.vault.host(Some(id)).and_then(|h| h.group) } else { self.vault.groups.iter().find(|g| g.id == id).and_then(|g| g.parent) };
        let mut banned = vec![];
        if kind == "group" {
            banned.push(id);
            let mut i = 0;
            while i < banned.len() {
                let b = banned[i];
                let kids: Vec<u64> = self.vault.groups.iter().filter(|g| g.parent == Some(b) && !banned.contains(&g.id)).map(|g| g.id).collect();
                banned.extend(kids);
                i += 1;
            }
        }
        let mut v: Vec<(u64, String)> = self.vault.groups.iter().filter(|g| Some(g.id) != cur && !banned.contains(&g.id) && !self.item_read_only("group", g.id)).map(|g| (g.id, g.name.clone())).collect();
        v.sort_by_key(|a| a.1.to_lowercase());
        v
    }

    /// "Move to group" for a group's menu.
    fn group_menu(&self, ui: &mut egui::Ui, kind: &str, id: u64) {
        let targets = self.group_targets(kind, id);
        let top = self.vault.groups.iter().find(|g| g.id == id).map(|g| g.parent.is_some()).unwrap_or(false);
        icon_menu_button(ui, crate::icons::Icon::Group, "Move to group", |ui| {
            if targets.is_empty() && !top {
                label_muted(ui, "No other groups");
            }
            if top && submenu_item(ui, "No group").clicked() {
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("group_req"), (kind.to_string(), id, None::<u64>)));
                ui.close();
            }
            for (gid, name) in targets {
                if submenu_item(ui, &name).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("group_req"), (kind.to_string(), id, Some(gid))));
                    ui.close();
                }
            }
        });
    }

    /// Carries out a "Move to group" chosen in a menu. An item moved into a team's group joins that team.
    pub(crate) fn group_move_pump(&mut self, ctx: &egui::Context) {
        let Some((kind, id, target)) = ctx.data_mut(|d| d.remove_temp::<(String, u64, Option<u64>)>(egui::Id::new("group_req"))) else { return };
        if self.item_read_only(&kind, id) {
            return;
        }
        if kind == "host" {
            if let Some(h) = self.vault.hosts.iter_mut().find(|h| h.id == id) {
                h.group = target;
            }
        } else if let Some(g) = self.vault.groups.iter_mut().find(|g| g.id == id) {
            g.parent = target;
        }
        self.dirty = true;
        if target.is_some() {
            self.sync_place_new(&kind, id);
        }
    }

    fn host_cards(&self, ui: &mut egui::Ui, hosts: &[Host], w: f32, live_ids: &std::collections::HashSet<u64>, acts: &mut Vec<HAct>) {
        use crate::hostcard::{host_card, CardAct, HostCardIn};
        ui.horizontal_wrapped(|ui| {
            for h in hosts {
                let color = h.group.and_then(|g| self.vault.groups.iter().find(|x| x.id == g)).map(|g| icon_color(&g.name)).unwrap_or_else(|| icon_color(&h.address));
                let team = self.team_tag("host", h.id).trim().trim_start_matches('·').trim().to_string();
                let targets = self.move_targets("host", h.id);
                let c = HostCardIn { host: h, user: h.resolved_username(&self.vault.identities), color, live: live_ids.contains(&h.id), team, moves: targets.iter().map(|t| t.1.clone()).collect(), groups: self.group_targets("host", h.id), in_group: h.group.is_some() };
                match host_card(ui, w, &c) {
                    CardAct::MoveGroup(g) => {
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("group_req"), ("host".to_string(), h.id, g)));
                    }
                    CardAct::Move(i) => {
                        if let Some(t) = targets.get(i) {
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("move_req"), ("host".to_string(), h.id, t.0.clone())));
                        }
                    }
                    CardAct::None => {}
                    CardAct::Open => acts.push(HAct::Connect(h.id)),
                    CardAct::Sftp => acts.push(HAct::Sftp(h.id)),
                    CardAct::Edit => acts.push(HAct::Edit(h.id)),
                    CardAct::Duplicate => acts.push(HAct::Dup(h.id)),
                    CardAct::Pin => acts.push(HAct::Pin(h.id)),
                    CardAct::Delete => acts.push(HAct::Del(h.id)),
                }
            }
        });
    }

    /// A small window asking for a session code; Join opens the shared terminal in a new tab.
    fn join_window(&mut self, ctx: &egui::Context) {
        if !self.collab.show_join {
            return;
        }
        let mut join = false;
        let mut cancel = false;
        let (_, closed) = app_modal(ctx, "join_modal", "Join shared session", |ui| {
            ui.set_width(320.0);
            label_muted(ui, "Enter the session code the host gave you.");
            ui.add_space(6.0);
            let r = input_box(ui, egui::Id::new("join-code"), &mut self.collab.join_code, "e.g. abcd-2345", Some(crate::icons::Icon::Users), false, false);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                join = true;
            }
            modal_buttons(ui, |ui| {
                if primary_button(ui, "Join").clicked() {
                    join = true;
                }
                if flat_button(ui, "Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        cancel |= closed;
        if join {
            self.collab.show_join = false;
            self.collab_join();
        } else if cancel {
            self.collab.show_join = false;
        }
    }

    /// Adds the hosts listed in ~/.ssh/config (skipping wildcard entries and ones already saved).
    pub(crate) fn import_ssh_config(&mut self) {
        let path = expand_home("~/.ssh/config");
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.notify("No ~/.ssh/config found".into(), AMBER());
            return;
        };
        struct Entry {
            names: Vec<String>,
            hostname: String,
            user: String,
            port: Option<u16>,
            key: String,
        }
        let mut entries: Vec<Entry> = vec![];
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
                Some((k, v)) => (k.to_lowercase(), v.trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim().trim_matches('"').to_string()),
                None => continue,
            };
            match k.as_str() {
                "host" => entries.push(Entry { names: v.split_whitespace().map(|s| s.to_string()).collect(), hostname: String::new(), user: String::new(), port: None, key: String::new() }),
                "hostname" => if let Some(e) = entries.last_mut() { e.hostname = v },
                "user" => if let Some(e) = entries.last_mut() { e.user = v },
                "port" => if let Some(e) = entries.last_mut() { e.port = v.parse().ok() },
                "identityfile" => if let Some(e) = entries.last_mut() { if e.key.is_empty() { e.key = v } },
                _ => {}
            }
        }
        let (mut added, mut skipped) = (0, 0);
        for e in entries {
            for name in e.names.iter().filter(|n| !n.contains(['*', '?', '!'])) {
                let address = if e.hostname.is_empty() { name.clone() } else { e.hostname.clone() };
                if self.vault.hosts.iter().any(|h| h.label == *name && h.address == address) {
                    skipped += 1;
                    continue;
                }
                let key = if e.key.is_empty() {
                    None
                } else {
                    match self.vault.keys.iter().find(|k| k.path == e.key) {
                        Some(k) => Some(k.id),
                        None => {
                            let id = self.vault.new_id();
                            let label = e.key.rsplit('/').next().unwrap_or("key").to_string();
                            self.vault.keys.push(SshKey { id, label, path: e.key.clone(), ..Default::default() });
                            Some(id)
                        }
                    }
                };
                let id = self.vault.new_id();
                self.vault.hosts.push(Host { id, label: name.clone(), address, group: self.cur_group, protocol: Protocol::Ssh, port: e.port, username: e.user.clone(), key, ..Default::default() });
                added += 1;
            }
        }
        if added > 0 {
            self.dirty = true;
        }
        let msg = if added == 0 && skipped == 0 { "No hosts found in ~/.ssh/config".to_string() } else { format!("Imported {added} host(s){}", if skipped > 0 { format!(", {skipped} already saved") } else { String::new() }) };
        self.notify(msg, if added > 0 { GREEN() } else { AMBER() });
    }

    fn apply_host_acts(&mut self, acts: Vec<HAct>) {
        for a in acts {
            match a {
                HAct::Connect(id) => {
                    self.connect_host(id);
                }
                HAct::Sftp(id) => {
                    if let Some(h) = self.vault.host(Some(id)).cloned() {
                        self.screen = Screen::Sftp;
                        let ctx = self.ctx.clone();
                        self.sftp_open(&h, &ctx);
                    }
                }
                HAct::Pin(id) => {
                    if let Some(h) = self.vault.hosts.iter_mut().find(|h| h.id == id) {
                        h.pinned = !h.pinned;
                        self.dirty = true;
                    }
                }
                HAct::Edit(id) => {
                    if let Some(h) = self.vault.host(Some(id)).cloned() {
                        self.edit_host(h, false);
                    }
                }
                HAct::NewHost => {
                    let h = Host { group: self.cur_group, ..Default::default() };
                    self.edit_host(h, true);
                }
                HAct::Dup(id) => {
                    if let Some(mut h) = self.vault.host(Some(id)).cloned() {
                        h.id = self.vault.new_id();
                        h.label = format!("{} copy", h.display_label());
                        self.vault.hosts.push(h);
                        self.dirty = true;
                    }
                }
                HAct::Del(id) => self.confirm = Some(Del::Host(id)),
                HAct::OpenGroup(g) => {
                    self.cur_group = g;
                    self.search.clear();
                }
                HAct::EditGroup(id) => {
                    if let Some(g) = self.vault.groups.iter().find(|g| g.id == id).cloned() {
                        self.editor = Editor::Group(g);
                        self.editor_new = false;
                    }
                }
                HAct::NewGroup => {
                    self.editor = Editor::Group(Group { parent: self.cur_group, ..Default::default() });
                    self.editor_new = true;
                }
                HAct::Quick(t) => {
                    let spec = self.spec_for_quick(&t);
                    self.open_session(spec);
                }
                HAct::ImportConfig => self.confirm_import = true,
                HAct::LocalTerminal => {
                    let spec = self.local_spec();
                    self.open_session(spec);
                }
                HAct::JoinShared => self.collab.show_join = true,
            }
        }
    }

    // ----------------------------------------------------------- keychain --

    /// Window for importing a private key from a file: the file is copied into the app's own key folder.
    pub(crate) fn key_import_window(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.key_import.take() else { return };
        if let Some(rx) = &st.picking {
            match rx.try_recv() {
                Ok(p) => {
                    if let Some(p) = p {
                        if st.label.trim().is_empty() {
                            st.label = p.rsplit('/').next().unwrap_or("").to_string();
                        }
                        st.path = p;
                    }
                    st.picking = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
                Err(_) => st.picking = None,
            }
        }
        let (mut browse, mut go, mut cancel) = (false, false, false);
        let picking = st.picking.is_some();
        let (_, closed) = app_modal(ctx, "import_key_modal", "Import key from file", |ui| {
            ui.set_width(380.0);
            label_muted(ui, "Pick a private key file. A copy is kept in the app, so the original can stay where it is.");
            ui.add_space(8.0);
            field_label(ui, "File", false);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 90.0).max(100.0);
                ui.allocate_ui(Vec2::new(w, 34.0), |ui| {
                    input_box(ui, egui::Id::new("import-key-path"), &mut st.path, "~/.ssh/id_ed25519", Some(crate::icons::Icon::File), false, false);
                });
                ui.add_enabled_ui(!picking, |ui| {
                    if flat_button(ui, if picking { "Waiting…" } else { "Browse…" }).clicked() {
                        browse = true;
                    }
                });
            });
            ui.add_space(6.0);
            text_field(ui, "Label", &mut st.label, "Same as the file name", false);
            text_field(ui, "Passphrase", &mut st.passphrase, "Optional", true);
            modal_buttons(ui, |ui| {
                if primary_button(ui, "Import").clicked() {
                    go = true;
                }
                if flat_button(ui, "Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        cancel |= closed;
        if browse {
            let (tx, rx) = std::sync::mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(pick_file_dialog());
                c.request_repaint();
            });
            st.picking = Some(rx);
        }
        if cancel {
            return;
        }
        if go {
            match self.import_key_file(&st) {
                Ok(msg) => {
                    self.notify(msg, GREEN());
                    return;
                }
                Err(e) => self.notify(e, RED()),
            }
        }
        self.key_import = Some(st);
    }

    fn import_key_file(&mut self, st: &KeyImport) -> Result<String, String> {
        let src = expand_home(st.path.trim());
        if src.is_empty() {
            return Err("Choose a key file first".into());
        }
        let bytes = std::fs::read(&src).map_err(|e| format!("Cannot read the file: {e}"))?;
        let text = String::from_utf8_lossy(&bytes);
        if !text.contains("PRIVATE KEY") {
            return Err("That file does not look like a private key".into());
        }
        let name = src.rsplit('/').next().unwrap_or("key").to_string();
        let dir = data_dir().join("keys");
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create the key folder: {e}"))?;
        let base = sanitize(&name);
        let mut dest = dir.join(&base);
        let mut n = 2;
        while dest.exists() {
            dest = dir.join(format!("{base}-{n}"));
            n += 1;
        }
        std::fs::write(&dest, &bytes).map_err(|e| format!("Cannot save the key: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600));
        }
        let dest_s = dest.to_string_lossy().to_string();
        let public_key = std::fs::read_to_string(format!("{src}.pub")).map(|s| s.trim().to_string()).unwrap_or_default();
        if !public_key.is_empty() {
            let _ = std::fs::write(format!("{dest_s}.pub"), format!("{public_key}\n"));
        }
        let label = if st.label.trim().is_empty() { name } else { st.label.trim().to_string() };
        let id = self.vault.new_id();
        self.vault.keys.push(SshKey { id, label: label.clone(), path: dest_s, passphrase: st.passphrase.clone(), public_key, sync_contents: false, added: now_secs(), generated: false });
        self.dirty = true;
        Ok(format!("Imported key {label}"))
    }

    // --------------------------------------------------- port forwarding --

    pub fn screen_forwarding(&mut self, ui: &mut egui::Ui) {
        let mut new_rule = false;
        let mut toggle_id: Option<u64> = None;
        let mut edit: Option<PortForward> = None;
        page_header(ui, "Port Forwarding", |ui| {
            if primary_button(ui, "+ New Rule").clicked() {
                new_rule = true;
            }
        });
        if self.vault.forwards.is_empty() {
            empty_state(ui, "No rules yet", "A rule forwards a local port through one of your hosts (local, remote or SOCKS).");
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            let w = ui.available_width().min(760.0) - 1.0;
            for f in self.vault.forwards.iter().filter(|f| self.in_vault("forward", f.id)) {
                let hl = self.host_label(f.host);
                let live = self.forwards_live.contains_key(&f.id);
                let (c, r) = item_card(ui, Vec2::new(w, 60.0), &f.display_label(&hl), &if f.label.is_empty() { f.kind.name().to_string() } else { f.describe(&hl) }, "icon:forward", icon_color(f.kind.name()), if live { Some(GREEN()) } else { None }, if live { "Stop" } else { "Start" }, 56.0, false);
                c.context_menu(|ui| {
                    if icon_text_button(ui, crate::icons::Icon::Pencil, "Edit").clicked() {
                        edit = Some(f.clone());
                        ui.close();
                    }
                    self.move_menu(ui, "forward", f.id);
                });
                if r.clicked() {
                    toggle_id = Some(f.id);
                } else if c.clicked() {
                    edit = Some(f.clone());
                }
            }
        });
        if let Some(id) = toggle_id {
            if self.forwards_live.contains_key(&id) {
                self.stop_forward(id);
            } else {
                self.start_forward(id);
            }
        }
        if new_rule {
            self.editor = Editor::Forward(PortForward { local_port: 8080, hostname: "localhost".into(), remote_port: 80, bound_address: "127.0.0.1".into(), ..Default::default() });
            self.editor_new = true;
        } else if let Some(f) = edit {
            self.editor = Editor::Forward(f);
            self.editor_new = false;
        }
    }

    // ----------------------------------------------------------- snippets --

    pub fn screen_snippets(&mut self, ui: &mut egui::Ui) {
        let mut new_s = false;
        let mut run: Option<u64> = None;
        let mut edit: Option<Snippet> = None;
        page_header(ui, "Snippets", |ui| {
            if primary_button(ui, "+ New Snippet").clicked() {
                new_s = true;
            }
        });
        if self.vault.snippets.is_empty() {
            empty_state(ui, "No snippets yet", "Save commands you run often and send them to an open terminal with one click.");
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);
            let w = ui.available_width().min(760.0) - 1.0;
            for s in self.vault.snippets.iter().filter(|s| self.in_vault("snippet", s.id)) {
                let first = s.script.lines().next().unwrap_or("").to_string();
                let (c, r) = item_card(ui, Vec2::new(w, 60.0), &s.display_label(), &format!("{}{}", first, self.team_tag("snippet", s.id)), "icon:snippet", icon_color(&s.label), None, "Run", 48.0, false);
                c.context_menu(|ui| {
                    if icon_text_button(ui, crate::icons::Icon::Pencil, "Edit").clicked() {
                        edit = Some(s.clone());
                        ui.close();
                    }
                    self.move_menu(ui, "snippet", s.id);
                });
                if r.clicked() {
                    run = Some(s.id);
                } else if c.clicked() {
                    edit = Some(s.clone());
                }
            }
        });
        if let Some(id) = run {
            let script = self.vault.snippets.iter().find(|s| s.id == id).map(|s| s.script.clone());
            if let (Some(script), Some(t)) = (script, self.tabs.last_mut()) {
                t.sess.send_line(&script);
                t.focus_next = true;
                let tid = t.id;
                self.active = Some(tid);
                if let Some(s) = self.vault.snippets.iter_mut().find(|s| s.id == id) {
                    s.last_used = now_secs();
                }
                self.dirty = true;
            } else {
                self.notify("Open a terminal first, then run the snippet".into(), AMBER());
            }
        }
        if new_s {
            self.editor = Editor::Snippet(Snippet::default());
            self.editor_new = true;
        } else if let Some(s) = edit {
            self.editor = Editor::Snippet(s);
            self.editor_new = false;
        }
    }

    // -------------------------------------------------------- known hosts --

    fn load_known_hosts(&self) -> Vec<KhRow> {
        let p = data_dir().join("known_hosts");
        let Ok(s) = std::fs::read_to_string(&p) else { return vec![] };
        let mut rows: Vec<KhRow> = s
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let mut it = l.split_whitespace();
                let host = it.next().unwrap_or("").to_string();
                let kind = it.next().unwrap_or("").to_string();
                KhRow { host, kind, fingerprint: String::new() }
            })
            .collect();
        if let Ok(o) = std::process::Command::new("ssh-keygen").arg("-l").arg("-f").arg(&p).output() {
            let t = String::from_utf8_lossy(&o.stdout).to_string();
            let fps: Vec<String> = t.lines().map(|l| l.split_whitespace().nth(1).unwrap_or("").to_string()).collect();
            if fps.len() == rows.len() {
                for (r, f) in rows.iter_mut().zip(fps) {
                    r.fingerprint = f;
                }
            }
        }
        rows
    }

    pub fn screen_known_hosts(&mut self, ui: &mut egui::Ui) {
        if self.kh.is_none() {
            self.kh = Some(self.load_known_hosts());
        }
        let mut refresh = false;
        let mut del: Option<usize> = None;
        page_header(ui, "Known Hosts", |ui| {
            if flat_button(ui, "Refresh").clicked() {
                refresh = true;
            }
        });
        let rows = self.kh.as_ref().unwrap();
        if rows.is_empty() {
            empty_state(ui, "No known hosts", "Server fingerprints are remembered here the first time you connect.");
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let w = ui.available_width().min(860.0) - 1.0;
            for (i, r) in rows.iter().enumerate() {
                let (_, x) = item_card(ui, Vec2::new(w, 56.0), &r.host, &format!("{}  {}", r.kind, r.fingerprint), "icon:shield", icon_color(&r.host), None, "Remove", 64.0, false);
                if x.clicked() {
                    del = Some(i);
                }
            }
        });
        if let Some(i) = del {
            self.confirm = Some(Del::KnownHost(i));
            self.kh = None;
        }
        if refresh {
            self.kh = None;
        }
    }

    // --------------------------------------------------------------- logs --

    pub fn screen_logs(&mut self, ui: &mut egui::Ui) {
        if let Some((path, text)) = &self.log_view {
            let mut back = false;
            let mut del = false;
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            ui.horizontal(|ui| {
                if flat_button(ui, "← Back").clicked() {
                    back = true;
                }
                ui.label(RichText::new(name).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if danger_button(ui, "Delete log").clicked() {
                        del = true;
                    }
                });
            });
            ui.add_space(6.0);
            egui::ScrollArea::both().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text.as_str()).monospace().size(12.0)).extend());
            });
            let p = path.clone();
            if back {
                self.log_view = None;
            }
            if del {
                self.confirm = Some(Del::Log(p));
            }
            return;
        }
        let dir = data_dir().join("logs");
        let mut files: Vec<(std::path::PathBuf, u64, u64)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let m = e.metadata().ok()?;
                        if !m.is_file() {
                            return None;
                        }
                        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
                        Some((e.path(), m.len(), t))
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by_key(|f| std::cmp::Reverse(f.2));
        files.truncate(300);
        page_header(ui, "Logs", |ui| {
            label_muted(ui, "Every terminal session is recorded here");
        });
        if files.is_empty() {
            empty_state(ui, "No session logs yet", "Connect to a host or open a local terminal and its output will be saved.");
        }
        let mut open: Option<std::path::PathBuf> = None;
        let mut del: Option<std::path::PathBuf> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let w = ui.available_width().min(860.0) - 1.0;
            for (p, size, t) in &files {
                let name = p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let title = name.split_once("__").map(|x| x.1.to_string()).unwrap_or(name.clone());
                let (c, x) = item_card(ui, Vec2::new(w, 56.0), &title, &format!("{}  ·  {}", fmt_time(*t, self.tz), fmt_size(*size)), "icon:logs", icon_color(&title), None, "Delete", 56.0, false);
                if x.clicked() {
                    del = Some(p.clone());
                } else if c.clicked() {
                    open = Some(p.clone());
                }
            }
        });
        if let Some(p) = open {
            let mut bytes = std::fs::read(&p).unwrap_or_default();
            if crate::vaultcrypt::is_encrypted_log(&bytes) {
                bytes = match self.log_key().map(|k| crate::vaultcrypt::read_log(&bytes, &k)) {
                    Some(Ok(b)) => b,
                    Some(Err(_)) => b"[This log could not be decrypted: it is damaged or was made with another key.]".to_vec(),
                    None => b"[This log is encrypted. Unlock the vault to read it.]".to_vec(),
                };
            }
            let start = bytes.len().saturating_sub(300_000);
            let text = strip_ansi(&String::from_utf8_lossy(&bytes[start..]));
            self.log_view = Some((p, text));
        }
        if let Some(p) = del {
            self.confirm = Some(Del::Log(p));
        }
    }
}

/// "Drop a key file here to fill in the path": returns the path of a file dropped on it.
fn key_drop_zone(ui: &mut egui::Ui) -> Option<String> {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), egui::Sense::hover());
    let over = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let col = if over { BLUE() } else { BORDER() };
    if over {
        ui.painter().rect_filled(r, 8, BLUE().linear_multiply(0.08));
    }
    let rr = r.shrink(0.5);
    let c = [rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()];
    for pair in c.windows(2) {
        ui.painter().extend(egui::Shape::dashed_line(&[pair[0], pair[1]], egui::Stroke::new(1.0, col), 5.0, 4.0));
    }
    let g = ui.painter().layout_no_wrap("Drop a key file here to import it".into(), FontId::proportional(12.5), MUTED());
    let x0 = r.center().x - (24.0 + g.size().x) / 2.0;
    crate::icons::paint(ui.painter(), Pos2::new(x0 + 7.0, r.center().y), 14.0, crate::icons::Icon::Download, MUTED());
    ui.painter().galley(Pos2::new(x0 + 24.0, r.center().y - g.size().y / 2.0), g, MUTED());
    ui.ctx().input(|i| i.raw.dropped_files.iter().find_map(|f| Some(f.path()).filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_string_lossy().to_string())))
}

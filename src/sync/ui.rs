//! Settings → Sync: create an account or sign in, see the status, sign out.

use super::{can_enable, from_hex32, now_ms, prepare_vault, to_hex, SyncConfig};
use crate::app::App;
use crate::model::{save_vault, vault_path};
use crate::ui::*;
use eframe::egui::{self, RichText, Vec2};
use std::sync::mpsc::{channel, Receiver};
use sync_client::account;
use sync_client::api::Api;
use sync_client::SyncError;

pub struct Done {
    pub recovery: Option<String>,
    pub session: account::Session,
    pub server: String,
    pub email: String,
    pub allow_private_http: bool,
}

#[derive(Default)]
pub struct SyncUi {
    pub server: String,
    pub email: String,
    pub password: String,
    pub invite: String,
    pub allow_http: bool,
    pub message: Option<(String, bool)>,
    pub working: Option<Receiver<Result<Done, SyncError>>>,
    /// shown once after creating an account
    pub recovery: Option<String>,
    pub recovery_saved: bool,
    /// forgot-password form
    pub show_recover: bool,
    pub recovery_in: String,
    pub new_password: String,
    pub op: Option<Receiver<OpResult>>,
    pub devices: Option<Vec<serde_json::Value>>,
    pub team_name: String,
    pub member_email: String,
    pub member_team: i64,
    /// the "Add member" field is open in the card of `member_team`
    pub add_open: bool,
    /// the role the next person gets: "admin", "editor" or "viewer" (empty: editor)
    pub member_role: String,
    /// someone about to be added to a team: waits for the owner to confirm the fingerprint
    pub candidate: Option<(i64, sync_client::teams::MemberLookup)>,
    /// (item id, scope, warning) waiting for a yes before a secret is shared with a team
    pub pending_move: Option<(String, String, String)>,
    /// a move started from a list menu that needs a yes: (kind, id, scope, warning)
    pub move_ask: Option<(String, u64, String, String)>,
    /// where a new item will live, chosen in its form (None until the form first draws; "u" is only me)
    pub new_scope: Option<String>,
    /// (team id, user id, email) waiting for a yes before someone is removed
    pub confirm_remove: Option<(i64, i64, String)>,
    /// (team id, name) waiting for a yes before the team is deleted
    pub confirm_delete_team: Option<(i64, String)>,
}

/// What a "Move to ..." target is called inside the "Move to team" submenu: the team's name, or Personal.
pub fn target_name(label: &str) -> &str {
    label.strip_prefix("Move to team ").or_else(|| label.strip_prefix("Move to ")).unwrap_or(label)
}

/// The name shown for a role.
pub fn role_name(role: &str) -> &'static str {
    match sync_core::wire::roles::normalize(role) {
        "owner" => "Owner",
        "admin" => "Admin",
        "viewer" => "Viewer",
        _ => "Editor",
    }
}

/// The roles a person with `role` may give to others.
pub fn grantable(role: &str) -> Vec<&'static str> {
    use sync_core::wire::roles::can_grant;
    ["admin", "editor", "viewer"].into_iter().filter(|r| can_grant(role, r)).collect()
}

pub enum OpResult {
    Msg(String, bool),
    Devices(Vec<serde_json::Value>),
    Candidate(i64, sync_client::teams::MemberLookup),
    /// the server can delete teams: start moving this team's items back to Personal
    CloseTeam(i64),
}

fn field(ui: &mut egui::Ui, text: &mut String, hint: &str, password: bool) {
    ui.add(egui::TextEdit::singleline(text).password(password).hint_text(hint).desired_width(300.0).margin(Vec2::new(10.0, 8.0)));
}

/// The box each team (and the fingerprint) sits in.
fn team_frame() -> egui::Frame {
    egui::Frame::new().fill(CARD()).stroke(egui::Stroke::new(1.0, BORDER())).corner_radius(10).inner_margin(egui::Margin::symmetric(14, 12))
}

/// A thin line between rows of a team card.
fn divider(ui: &mut egui::Ui) {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 9.0), egui::Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1.0, BORDER()));
}

/// A small square button with only an icon; red on hover, the tip says what it does.
fn icon_button(ui: &mut egui::Ui, icon: crate::icons::Icon, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
    let hot = resp.hovered();
    if hot {
        ui.painter().rect_filled(r, 6.0, HOVER());
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    crate::icons::paint(ui.painter(), r.center(), 14.0, icon, if hot { RED() } else { MUTED() });
    resp.on_hover_text(tip)
}

/// An amber note with its buttons, for a step that needs a yes.
fn warn_box(ui: &mut egui::Ui, text: &str, buttons: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(AMBER().linear_multiply(0.10)).stroke(egui::Stroke::new(1.0, AMBER().linear_multiply(0.45))).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(text).size(12.0).color(TEXT()));
        ui.add_space(6.0);
        ui.horizontal(buttons);
    });
}

fn ago(ms: u64) -> String {
    let s = now_ms().saturating_sub(ms) / 1000;
    match s {
        0..=4 => "just now".into(),
        5..=59 => format!("{s} seconds ago"),
        60..=3599 => format!("{} minutes ago", s / 60),
        _ => format!("{} hours ago", s / 3600),
    }
}

impl App {
    pub fn settings_sync(&mut self, ui: &mut egui::Ui) {
        section_title(ui, "SYNC");
        if self.demo {
            ui.label(RichText::new("Not available in a demo run.").size(12.0).color(MUTED()));
            return;
        }
        let mut su = std::mem::take(&mut self.sync_rt.ui);
        self.sync_ui_inner(ui, &mut su);
        self.sync_rt.ui = su;
    }

    fn sync_ui_inner(&mut self, ui: &mut egui::Ui, su: &mut SyncUi) {
        // an account request that finished
        let mut finished = None;
        if let Some(rx) = &su.working {
            match rx.try_recv() {
                Ok(r) => finished = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
                Err(_) => finished = Some(Err(SyncError::Bad("the request stopped".into()))),
            }
        }
        if let Some(r) = finished {
            su.working = None;
            match r {
                Ok(done) => self.sync_adopt(su, done),
                Err(e) => su.message = Some((e.to_string(), false)),
            }
        }

        let mut op_done = None;
        if let Some(rx) = &su.op {
            match rx.try_recv() {
                Ok(r) => op_done = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
                Err(_) => op_done = Some(OpResult::Msg("The request stopped unexpectedly.".into(), false)),
            }
        }
        if let Some(r) = op_done {
            su.op = None;
            match r {
                OpResult::Msg(m, ok) => su.message = Some((m, ok)),
                OpResult::Devices(d) => su.devices = Some(d),
                OpResult::Candidate(t, v) => su.candidate = Some((t, v)),
                OpResult::CloseTeam(t) => {
                    self.close_team(t);
                    su.message = None;
                }
            }
        }

        if let Some(code) = su.recovery.clone() {
            ui.label(RichText::new("Your recovery key").strong().size(13.0).color(TEXT()));
            ui.label(RichText::new("It is the only way back in if you forget the sync password. It is shown once and the server cannot give it back.").size(12.0).color(AMBER()));
            ui.add_space(4.0);
            ui.label(RichText::new(code).monospace().size(14.0).color(TEXT()));
            ui.add_space(4.0);
            ui.checkbox(&mut su.recovery_saved, "I saved it somewhere safe");
            if su.recovery_saved && primary_button(ui, "Done").clicked() {
                su.recovery = None;
                su.recovery_saved = false;
            }
            ui.add_space(8.0);
        }

        if let Some(cfg) = self.vault.sync.clone() {
            ui.label(RichText::new(format!("Signed in as {} on {}", cfg.email, cfg.server)).size(12.5).color(GREEN()));
            let rt = &self.sync_rt;
            let status = match (&rt.last_error, rt.last_ok_ms) {
                (Some(e), _) => RichText::new(e).size(12.0).color(RED()),
                (None, Some(t)) => RichText::new(format!("Last sync {} ({})", ago(t), rt.summary)).size(12.0).color(MUTED()),
                (None, None) => RichText::new(if rt.rx.is_some() { "Syncing…" } else { "Waiting for the first sync" }).size(12.0).color(MUTED()),
            };
            ui.label(status);
            if !self.sync_rt.state.as_ref().map(|s| s.replaced.is_empty()).unwrap_or(true) {
                let n = self.sync_rt.state.as_ref().map(|s| s.replaced.len()).unwrap_or(0);
                ui.label(RichText::new(format!("{n} older version(s) were replaced by newer ones from other devices. They are kept, encrypted, on this computer.")).size(12.0).color(MUTED()));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Sync now").clicked() {
                    self.sync_rt.want_now = true;
                    self.sync_rt.stopped = false;
                }
                if danger_button(ui, "Sign out").clicked() {
                    self.sync_sign_out();
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Change the sync password").strong().size(12.5).color(TEXT()));
            field(ui, &mut su.new_password, "New sync password (8+ characters). Other devices will be signed out.", true);
            if ui.add_enabled(su.op.is_none() && !su.new_password.is_empty(), egui::Button::new("Change password")).clicked() {
                if let Some(key) = from_hex32(&cfg.sync_key) {
                    let (tx, rx) = channel();
                    su.op = Some(rx);
                    let pw = zeroize::Zeroizing::new(std::mem::take(&mut su.new_password));
                    let (server, token) = (cfg.server.clone(), cfg.token.clone());
                    std::thread::spawn(move || {
                        let r = account::change_password(&Api::new(&server, Some(token)), &key, &pw);
                        let _ = tx.send(match r {
                            Ok(()) => OpResult::Msg("Password changed. Your other devices must sign in again.".into(), true),
                            Err(e) => OpResult::Msg(e.to_string(), false),
                        });
                    });
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Devices").strong().size(12.5).color(TEXT()));
                if ui.add_enabled(su.op.is_none(), egui::Button::new("Refresh")).clicked() {
                    let (tx, rx) = channel();
                    su.op = Some(rx);
                    let (server, token) = (cfg.server.clone(), cfg.token.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(match Api::new(&server, Some(token)).devices() {
                            Ok(d) => OpResult::Devices(d),
                            Err(e) => OpResult::Msg(e.to_string(), false),
                        });
                    });
                }
            });
            let mut revoke: Option<i64> = None;
            if let Some(devs) = &su.devices {
                for d in devs {
                    ui.horizontal(|ui| {
                        let name = d["name"].as_str().unwrap_or("?");
                        let cur = d["current"].as_bool().unwrap_or(false);
                        ui.label(RichText::new(format!("{name}{}  ·  last seen {}", if cur { " (this computer)" } else { "" }, ago(d["last_seen"].as_u64().unwrap_or(0)))).size(12.0).color(MUTED()));
                        if !cur && ui.small_button("Sign out this device").clicked() {
                            revoke = d["id"].as_i64();
                        }
                    });
                }
            }
            self.sync_teams_ui(ui, su, &cfg);
            if let Some(id) = revoke {
                let (tx, rx) = channel();
                su.op = Some(rx);
                let (server, token) = (cfg.server.clone(), cfg.token.clone());
                std::thread::spawn(move || {
                    let api = Api::new(&server, Some(token));
                    let _ = tx.send(match api.revoke_device(id).and_then(|_| api.devices()) {
                        Ok(d) => OpResult::Devices(d),
                        Err(e) => OpResult::Msg(e.to_string(), false),
                    });
                });
            }
        } else {
            let encrypted = self.vault_key.is_some();
            ui.label(RichText::new("Keep your hosts, identities, keys and snippets in step on all your devices. Everything is encrypted on this computer before it is sent; the server only stores scrambled data.").size(12.0).color(MUTED()));
            if !encrypted {
                ui.add_space(4.0);
                ui.label(RichText::new("Turn on the master password first (Security, below). Sync keeps its keys inside the encrypted vault.").size(12.0).color(AMBER()));
            }
            ui.add_space(6.0);
            let busy = su.working.is_some();
            ui.add_enabled_ui(encrypted && !busy, |ui| {
                field(ui, &mut su.server, "Server address, e.g. https://sync.example.com", false);
                field(ui, &mut su.email, "Email", false);
                field(ui, &mut su.password, "Sync password (8+ characters, not your master password)", true);
                field(ui, &mut su.invite, "Invite code (only to create an account)", false);
                ui.checkbox(&mut su.allow_http, "This network is private (allow http://)");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let register = primary_button(ui, "Create account").clicked();
                    let login = ui.button("Sign in").clicked();
                    if register || login {
                        self.sync_start_account(su, register);
                    }
                });
            });
            if busy {
                ui.label(RichText::new("Working…").size(12.0).color(MUTED()));
            }
            ui.add_space(6.0);
            if ui.small_button(if su.show_recover { "Hide password recovery" } else { "Forgot the sync password?" }).clicked() {
                su.show_recover = !su.show_recover;
            }
            if su.show_recover {
                field(ui, &mut su.recovery_in, "Recovery key (the one shown when you made the account)", false);
                field(ui, &mut su.new_password, "New sync password (8+ characters)", true);
                if ui.add_enabled(su.op.is_none(), egui::Button::new("Reset the password")).clicked() {
                    let server = su.server.trim().trim_end_matches('/').to_string();
                    if let Err(e) = can_enable(self.vault_key.is_some(), &server, su.allow_http) {
                        su.message = Some((e, false));
                    } else {
                        let (tx, rx) = channel();
                        su.op = Some(rx);
                        let (email, rk) = (su.email.trim().to_lowercase(), su.recovery_in.clone());
                        let pw = zeroize::Zeroizing::new(std::mem::take(&mut su.new_password));
                        std::thread::spawn(move || {
                            let r = account::recover(&Api::new(&server, None), &email, &rk, &pw);
                            let _ = tx.send(match r {
                                Ok(()) => OpResult::Msg("Password reset. Every device was signed out; sign in with the new password.".into(), true),
                                Err(e) => OpResult::Msg(e.to_string(), false),
                            });
                        });
                    }
                }
            }
        }
        if let Some((m, ok)) = &su.message {
            ui.add_space(4.0);
            ui.label(RichText::new(m).size(12.0).color(if *ok { GREEN() } else { RED() }));
        }
    }

    fn sync_start_account(&mut self, su: &mut SyncUi, register: bool) {
        let server = su.server.trim().trim_end_matches('/').to_string();
        if let Err(e) = can_enable(self.vault_key.is_some(), &server, su.allow_http) {
            su.message = Some((e, false));
            return;
        }
        let email = su.email.trim().to_lowercase();
        if email.is_empty() || su.password.is_empty() || (register && su.invite.trim().is_empty()) {
            su.message = Some((if register { "Fill in the address, email, password and invite code." } else { "Fill in the address, email and password." }.into(), false));
            return;
        }
        su.message = None;
        let (tx, rx) = channel();
        su.working = Some(rx);
        let pw = zeroize::Zeroizing::new(std::mem::take(&mut su.password));
        let invite = su.invite.clone();
        let allow = su.allow_http;
        let device = std::env::var("HOSTNAME").or_else(|_| std::env::var("HOST")).unwrap_or_else(|_| "this computer".into());
        std::thread::spawn(move || {
            let api = Api::new(&server, None);
            let res = (|| {
                let recovery = if register { Some(account::register(&api, &email, &pw, &invite)?) } else { None };
                let session = account::login(&api, &email, &pw, &device)?;
                Ok(Done { recovery, session, server: server.clone(), email: email.clone(), allow_private_http: allow })
            })();
            let _ = tx.send(res);
        });
    }

    fn sync_adopt(&mut self, su: &mut SyncUi, d: Done) {
        let migrated = match prepare_vault(&mut self.vault, &vault_path()) {
            Ok(n) => n,
            Err(e) => {
                su.message = Some((e, false));
                return;
            }
        };
        let s = &d.session.secrets;
        let cfg = SyncConfig {
            server: d.server,
            email: d.email,
            device_id: d.session.device_id,
            token: d.session.token,
            allow_private_http: d.allow_private_http,
            sync_key: to_hex(&*s.sync_key),
            x_secret: to_hex(&*s.x_secret),
            x_public: to_hex(&s.x_public),
            teams: vec![],
        };
        debug_assert!(from_hex32(&cfg.sync_key).is_some());
        let old = self.vault.sync.replace(cfg);
        if !save_vault(&self.vault, self.vault_key.as_ref()) {
            self.vault.sync = old;
            su.message = Some(("Could not save the vault, so sync was not turned on.".into(), false));
            return;
        }
        self.sync_rt.state = None;
        self.sync_rt.stopped = false;
        self.sync_rt.want_now = true;
        self.sync_rt.last_error = None;
        su.recovery = d.recovery;
        su.invite.clear();
        su.message = Some((if migrated > 0 { format!("Sync is on. {migrated} items were given new ids first; a copy of the old vault is next to it as vault.json.bak.") } else { "Sync is on.".to_string() }, true));
    }

    /// "Pa|Team name" tag for the lists, empty for personal items.
    /// The scope an item lives in: "u" (only me) or "t:<id>".
    pub fn item_scope(&self, kind: &str, id: u64) -> String {
        self.sync_rt.state.as_ref().and_then(|st| st.items.get(&super::state::item_id(kind, id))).map(|t| t.scope.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "u".into())
    }

    /// Whether the item shows in the vault picked in the sidebar. Everything shows when no vault is picked.
    pub fn in_vault(&self, kind: &str, id: u64) -> bool {
        match &self.cur_vault {
            None => true,
            Some(v) => self.item_scope(kind, id) == *v,
        }
    }

    /// Name of a vault scope for the sidebar and lists.
    pub fn vault_name(&self, scope: &str) -> String {
        match scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()) {
            Some(t) => self.vault.sync.as_ref().and_then(|c| c.teams.iter().find(|x| x.id == t)).map(|x| x.name.clone()).unwrap_or_else(|| "Team".into()),
            None => "Personal".into(),
        }
    }

    /// Picks the vault the lists show (None: everything). Leaves any open group, which belongs to the old vault.
    pub fn set_vault(&mut self, v: Option<String>) {
        if self.cur_vault != v {
            self.cur_vault = v;
            self.cur_group = None;
        }
    }

    pub fn team_tag(&self, kind: &str, id: u64) -> String {
        let Some(st) = &self.sync_rt.state else { return String::new() };
        let Some(t) = st.items.get(&super::state::item_id(kind, id)) else { return String::new() };
        let Some(team) = t.scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()) else { return String::new() };
        let name = self.vault.sync.as_ref().and_then(|c| c.teams.iter().find(|x| x.id == team)).map(|x| x.name.clone()).unwrap_or_else(|| "team".into());
        format!("  ·  Team {name}")
    }

    /// In the form of a NEW item: choose where it will live. Starts on the team of the group it is created in.
    /// The item lives in a team where this account is a viewer: it may be read and connected to, not changed.
    pub fn item_read_only(&self, kind: &str, id: u64) -> bool {
        let Some(cfg) = self.vault.sync.as_ref() else { return false };
        let Some(state) = self.sync_rt.state.as_ref() else { return false };
        let scope = state.items.get(&super::state::item_id(kind, id)).map(|t| t.scope.as_str()).unwrap_or("u");
        scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()).and_then(|t| cfg.teams.iter().find(|x| x.id == t)).map(|t| !t.can_write()).unwrap_or(false)
    }

    pub fn sync_new_scope_row(&mut self, ui: &mut egui::Ui, kind: &str, group: Option<u64>) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        if cfg.teams.is_empty() || self.sync_rt.state.is_none() {
            return;
        }
        if self.sync_rt.ui.new_scope.is_none() {
            let inherited = group
                .and_then(|g| self.sync_rt.state.as_ref().and_then(|s| s.items.get(&super::state::item_id("group", g))))
                .map(|t| t.scope.clone())
                .filter(|s| s.starts_with("t:") && matches!(kind, "host" | "group"));
            let from_vault = self.cur_vault.clone().filter(|v| v.starts_with("t:") && cfg.teams.iter().any(|t| super::teams::scope_of(t.id) == *v && t.can_write()));
            self.sync_rt.ui.new_scope = Some(inherited.or(from_vault).unwrap_or_else(|| "u".into()));
        }
        let cur = self.sync_rt.ui.new_scope.clone().unwrap_or_else(|| "u".into());
        let name_of = |scope: &str| -> String {
            match scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()) {
                Some(t) => format!("Team {}", cfg.teams.iter().find(|x| x.id == t).map(|x| x.name.as_str()).unwrap_or("?")),
                None => "Only me".to_string(),
            }
        };
        ui.add_space(8.0);
        ui.label(RichText::new("Who can see this").size(12.0).color(MUTED()));
        egui::ComboBox::from_id_salt("new_share_scope").width(ui.available_width() - 8.0).selected_text(name_of(&cur)).show_ui(ui, |ui| {
            if ui.selectable_label(cur == "u", "Only me").clicked() {
                self.sync_rt.ui.new_scope = Some("u".into());
            }
            for t in cfg.teams.iter().filter(|t| t.can_write()) {
                let sc = super::teams::scope_of(t.id);
                if ui.selectable_label(cur == sc, format!("Team {}", t.name)).clicked() {
                    self.sync_rt.ui.new_scope = Some(sc);
                }
            }
        });
        if cur != "u" {
            ui.label(RichText::new("Every team member can read saved passwords and keys on this item.").size(11.5).color(AMBER()));
        }
    }

    /// Where this item can be moved from its list menu: (scope, label). Empty without teams, or for a viewer.
    pub fn move_targets(&self, kind: &str, id: u64) -> Vec<(String, String)> {
        let Some(cfg) = self.vault.sync.as_ref() else { return vec![] };
        if cfg.teams.is_empty() || self.sync_rt.state.is_none() || self.item_read_only(kind, id) {
            return vec![];
        }
        let cur = self.item_scope(kind, id);
        let mut out = vec![];
        if cur != "u" {
            out.push(("u".to_string(), "Move to Personal".to_string()));
        }
        for t in cfg.teams.iter().filter(|t| t.can_write()) {
            let sc = super::teams::scope_of(t.id);
            if sc != cur {
                out.push((sc, format!("Move to team {}", t.name)));
            }
        }
        out
    }

    /// The "Move to ..." entries for a list menu. The choice is picked up by `sync_move_pump` on the next frame.
    pub fn move_menu(&self, ui: &mut egui::Ui, kind: &str, id: u64) {
        let targets = self.move_targets(kind, id);
        if targets.is_empty() {
            return;
        }
        ui.separator();
        icon_menu_button(ui, crate::icons::Icon::Users, "Move to team", |ui| {
            for (scope, label) in targets {
                if submenu_item(ui, target_name(&label)).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("move_req"), (kind.to_string(), id, scope)));
                    ui.close();
                }
            }
        });
    }

    /// Starts closing a team: every item in it moves back to Personal, and the team is deleted once the server has them.
    pub fn close_team(&mut self, team: i64) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        let scope = super::teams::scope_of(team);
        let vault = &self.vault;
        let Some(state) = self.sync_rt.state.as_mut() else { return };
        let ids: Vec<String> = state.items.iter().filter(|(_, t)| t.scope == scope && !t.deleted).map(|(k, _)| k.clone()).collect();
        let now = now_ms();
        for id in &ids {
            let _ = super::teams::move_to_scope(state, vault, id, "u", true, &cfg.device_id, now);
        }
        if self.cur_vault.as_deref() == Some(scope.as_str()) {
            self.cur_vault = None;
        }
        self.sync_rt.delete_team_after = Some((team, ids));
        self.sync_rt.want_now = true;
        self.dirty = true;
    }

    /// Moves an item (or a whole group) to another vault. A secret that becomes readable by a team waits for a yes.
    pub fn move_item(&mut self, kind: &str, id: u64, scope: &str, confirmed: bool) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        let item = super::state::item_id(kind, id);
        let vault = &self.vault;
        let Some(state) = self.sync_rt.state.as_mut() else { return };
        let res = if kind == "group" {
            super::teams::share_group(state, vault, id, scope, confirmed, &cfg.device_id, now_ms()).map(|n| n)
        } else {
            super::teams::move_to_scope(state, vault, &item, scope, confirmed, &cfg.device_id, now_ms()).map(|_| 1)
        };
        match res {
            Ok(n) => {
                self.sync_rt.ui.move_ask = None;
                self.sync_rt.want_now = true;
                self.dirty = true;
                let to = if scope == "u" { "Personal".to_string() } else { format!("team {}", self.vault_name(scope)) };
                self.notify(format!("{n} item{} moved to {to}", if n == 1 { "" } else { "s" }), GREEN());
            }
            Err(super::teams::MoveErr::NeedsConfirm(w)) => self.sync_rt.ui.move_ask = Some((kind.to_string(), id, scope.to_string(), w)),
            Err(_) => {}
        }
    }

    /// Runs once a frame: carries out a move chosen in a list menu, and asks before sharing secrets with a team.
    pub fn sync_move_pump(&mut self, ctx: &egui::Context) {
        if let Some((kind, id, scope)) = ctx.data_mut(|d| d.remove_temp::<(String, u64, String)>(egui::Id::new("move_req"))) {
            self.move_item(&kind, id, &scope, false);
        }
        let Some((kind, id, scope, warn)) = self.sync_rt.ui.move_ask.clone() else { return };
        let to = self.vault_name(&scope);
        let (act, closed) = app_modal(ctx, "move_ask_modal", "Share with the team?", |ui| {
            ui.set_width(340.0);
            label_muted(ui, &format!("Move to {to}."));
            ui.add_space(4.0);
            ui.label(RichText::new(&warn).size(12.5).color(AMBER()));
            let mut yes = false;
            let mut no = false;
            modal_buttons(ui, |ui| {
                yes = danger_button(ui, "Move it").clicked();
                no = flat_button(ui, "Cancel").clicked();
            });
            (yes, no)
        });
        if act.0 {
            self.move_item(&kind, id, &scope, true);
        } else if act.1 || closed {
            self.sync_rt.ui.move_ask = None;
        }
    }

    /// A new item was just saved: put it where the form said (or in the team of the shared group it landed in).
    pub fn sync_place_new(&mut self, kind: &str, id: u64) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        let chosen = self.sync_rt.ui.new_scope.take();
        let Some(state) = self.sync_rt.state.as_mut() else { return };
        let scope = match chosen {
            Some(sc) if sc == "u" => return,
            Some(sc) => {
                let _ = super::teams::move_to_scope(state, &self.vault, &super::state::item_id(kind, id), &sc, true, &cfg.device_id, now_ms());
                Some(sc)
            }
            None => super::teams::inherit_group_scope(state, &self.vault, kind, id, &cfg.device_id, now_ms()),
        };
        if let Some(scope) = scope {
            let name = scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()).and_then(|t| cfg.teams.iter().find(|x| x.id == t)).map(|x| x.name.clone()).unwrap_or_else(|| "the team".into());
            self.sync_rt.want_now = true;
            self.dirty = true;
            self.notify(format!("Shared with team {name}"), GREEN());
        }
    }

    /// In an item's editor: choose where the item lives (just me, or one of my teams).
    pub fn sync_share_row(&mut self, ui: &mut egui::Ui, kind: &str, id: u64) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        if cfg.teams.is_empty() || self.sync_rt.state.is_none() {
            return;
        }
        if self.item_read_only(kind, id) {
            ui.add_space(8.0);
            ui.label(RichText::new("You are a viewer in this team: you can connect, but not change or move this item.").size(11.5).color(MUTED()));
            return;
        }
        let item = super::state::item_id(kind, id);
        let cur = self.sync_rt.state.as_ref().and_then(|s| s.items.get(&item)).map(|t| t.scope.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "u".into());
        let name_of = |scope: &str| -> String {
            match scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()) {
                Some(t) => format!("Team {}", cfg.teams.iter().find(|x| x.id == t).map(|x| x.name.as_str()).unwrap_or("?")),
                None => "Only me".to_string(),
            }
        };
        ui.add_space(8.0);
        ui.label(RichText::new(if kind == "group" { "Who can see this group" } else { "Who can see this" }).size(12.0).color(MUTED()));
        if kind == "group" {
            let n = super::teams::group_items(&self.vault, id).len();
            ui.label(RichText::new(format!("Applies to the group, its sub-groups, their hosts and the keys and identities they use ({n} items). New hosts added to it follow.")).size(11.5).color(MUTED()));
        }
        let mut pick: Option<String> = None;
        egui::ComboBox::from_id_salt("share_scope").width(ui.available_width() - 8.0).selected_text(name_of(&cur)).show_ui(ui, |ui| {
            if ui.selectable_label(cur == "u", "Only me").clicked() {
                pick = Some("u".into());
            }
            for t in cfg.teams.iter().filter(|t| t.can_write()) {
                let sc = super::teams::scope_of(t.id);
                if ui.selectable_label(cur == sc, format!("Team {}", t.name)).clicked() {
                    pick = Some(sc);
                }
            }
        });
        let mut confirmed = false;
        if let Some((pid, psc, warn)) = self.sync_rt.ui.pending_move.clone() {
            if pid == item {
                ui.label(RichText::new(warn).size(12.0).color(AMBER()));
                ui.horizontal(|ui| {
                    if danger_button(ui, "Share it").clicked() {
                        pick = Some(psc.clone());
                        confirmed = true;
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        self.sync_rt.ui.pending_move = None;
                    }
                });
            }
        }
        if let Some(scope) = pick.filter(|s| *s != cur) {
            let vault = &self.vault;
            let Some(state) = self.sync_rt.state.as_mut() else { return };
            if kind == "group" {
                match super::teams::share_group(state, vault, id, &scope, confirmed, &cfg.device_id, now_ms()) {
                    Ok(n) => {
                        self.sync_rt.ui.pending_move = None;
                        self.sync_rt.want_now = true;
                        self.dirty = true;
                        self.notify(format!("{n} item{} moved", if n == 1 { "" } else { "s" }), GREEN());
                    }
                    Err(super::teams::MoveErr::NeedsConfirm(w)) => self.sync_rt.ui.pending_move = Some((item, scope, w)),
                    Err(_) => {}
                }
                return;
            }
            match super::teams::move_to_scope(state, vault, &item, &scope, confirmed, &cfg.device_id, now_ms()) {
                Ok(()) => {
                    self.sync_rt.ui.pending_move = None;
                    self.sync_rt.want_now = true;
                    self.dirty = true;
                }
                Err(super::teams::MoveErr::NeedsConfirm(w)) => self.sync_rt.ui.pending_move = Some((item, scope, w)),
                Err(_) => {}
            }
        }
    }

    fn sync_teams_ui(&mut self, ui: &mut egui::Ui, su: &mut SyncUi, cfg: &SyncConfig) {
        use sync_client::teams as tm;
        ui.add_space(14.0);
        ui.label(RichText::new("Teams").strong().size(13.0).color(TEXT()));
        ui.add_space(6.0);
        if let Some(xp) = from_hex32(&cfg.x_public) {
            team_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Your fingerprint").size(11.5).color(MUTED()));
                ui.label(RichText::new(tm::own_fingerprint(&xp)).monospace().size(13.0).color(TEXT()));
                ui.add_space(2.0);
                ui.label(RichText::new("A team owner sees this number when adding you. Read it to them (phone, chat) before they confirm.").size(11.5).color(MUTED()));
            });
            ui.add_space(10.0);
        }
        let (server, token) = (cfg.server.clone(), cfg.token.clone());
        for t in self.sync_rt.teams.clone() {
            team_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                // header: badge, name, role and size, delete on the right
                ui.horizontal(|ui| {
                    let (b, _) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                    let initial = t.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "T".into());
                    badge(ui.painter(), b, &initial, icon_color(&t.name.to_lowercase()));
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.add_space(1.0);
                        ui.label(RichText::new(&t.name).strong().size(14.0).color(TEXT()));
                        let n = t.members.len();
                        ui.label(RichText::new(format!("You are {}  ·  {n} member{}", role_name(&t.role), if n == 1 { "" } else { "s" })).size(11.5).color(MUTED()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_delete = t.owner && t.ready() && su.op.is_none() && self.sync_rt.delete_team_after.is_none();
                        if can_delete && icon_button(ui, crate::icons::Icon::Trash, "Delete team").clicked() {
                            su.confirm_delete_team = Some((t.id, t.name.clone()));
                        }
                    });
                });
                if self.sync_rt.delete_team_after.as_ref().map(|d| d.0) == Some(t.id) {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0));
                        ui.label(RichText::new("Moving the items back to Personal, then deleting the team…").size(11.5).color(MUTED()));
                    });
                }
                if !t.can_write() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("As a viewer you can connect and read, but not add or change anything. The team's key lets you open every item, so treat it like any other member would.").size(11.5).color(MUTED()));
                }
                if !t.ready() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(if t.owner { "The team key is being renewed after a removal." } else { "Waiting for the owner to renew the team key. Shared items are paused until then." }).size(11.5).color(AMBER()));
                }
                if let Some((tid, name)) = su.confirm_delete_team.clone().filter(|c| c.0 == t.id) {
                    ui.add_space(8.0);
                    warn_box(ui, &format!("Delete the team \"{name}\"? Every item in it goes back to your Personal vault. Other members lose the team and its items."), |ui| {
                        if danger_button(ui, "Delete team").clicked() {
                            su.confirm_delete_team = None;
                            // ask the server first: an older one cannot delete teams, and then nothing should move
                            let (tx, rx) = channel();
                            su.op = Some(rx);
                            let (s2, t2) = (server.clone(), token.clone());
                            std::thread::spawn(move || {
                                let _ = tx.send(match tm::can_delete_teams(&Api::new(&s2, Some(t2))) {
                                    Ok(true) => OpResult::CloseTeam(tid),
                                    Ok(false) => OpResult::Msg("This sync server cannot delete teams yet. Update it with deploy/update.sh, then try again.".into(), false),
                                    Err(e) => OpResult::Msg(e.to_string(), false),
                                });
                            });
                        }
                        if flat_button(ui, "Cancel").clicked() {
                            su.confirm_delete_team = None;
                        }
                    });
                }
                // members
                ui.add_space(8.0);
                for m in &t.members {
                    divider(ui);
                    ui.horizontal(|ui| {
                        let (a, _) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
                        avatar(ui.painter(), a.center(), 13.0, &m.email, icon_color(&m.email.to_lowercase()));
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&m.email).size(12.5).color(TEXT()));
                            ui.label(RichText::new(&m.fingerprint).monospace().size(10.5).color(MUTED()));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if t.owner && m.role != "owner" && t.ready() && t.stale_items == 0 && su.op.is_none() && icon_button(ui, crate::icons::Icon::Close, "Remove from team").clicked() {
                                su.confirm_remove = Some((t.id, m.user_id, m.email.clone()));
                            }
                            let can_edit_role = t.can_manage() && t.ready() && m.role != "owner" && !(m.role == "admin" && !t.owner);
                            if can_edit_role {
                                let mut pick = m.role.clone();
                                egui::ComboBox::from_id_salt(("member_role", t.id, m.user_id)).width(84.0).selected_text(role_name(&m.role)).show_ui(ui, |ui| {
                                    for r in grantable(&t.role) {
                                        ui.selectable_value(&mut pick, r.to_string(), role_name(r));
                                    }
                                });
                                if pick != m.role && su.op.is_none() {
                                    let (tx, rx) = channel();
                                    su.op = Some(rx);
                                    let (s2, t2, tid, uid, email, role) = (server.clone(), token.clone(), t.id, m.user_id, m.email.clone(), pick);
                                    self.sync_rt.want_now = true;
                                    std::thread::spawn(move || {
                                        let _ = tx.send(match tm::set_role(&Api::new(&s2, Some(t2)), tid, uid, &role) {
                                            Ok(()) => OpResult::Msg(format!("{email} is now {}.", role_name(&role).to_lowercase()), true),
                                            Err(e) => OpResult::Msg(e.to_string(), false),
                                        });
                                    });
                                }
                            } else {
                                ui.label(RichText::new(role_name(&m.role)).size(12.0).color(MUTED()));
                            }
                        });
                    });
                    if let Some((_, uid, email)) = su.confirm_remove.clone().filter(|c| c.0 == t.id && c.1 == m.user_id) {
                        ui.add_space(4.0);
                        warn_box(ui, &format!("Remove {email}? The team key is replaced and every shared item is sealed again, so they cannot read anything new. They keep what they already saw: change any password they could read."), |ui| {
                            if danger_button(ui, "Remove and renew the key").clicked() {
                                let (tx, rx) = channel();
                                su.op = Some(rx);
                                su.confirm_remove = None;
                                let (s2, t2, tv, xs) = (server.clone(), token.clone(), t.clone(), from_hex32(&cfg.x_secret));
                                self.sync_rt.want_now = true;
                                std::thread::spawn(move || {
                                    let r = match xs {
                                        Some(xs) => tm::remove_member_and_rotate(&Api::new(&s2, Some(t2)), &tv, uid, &xs),
                                        None => Err(SyncError::Bad("the account key on this device is damaged".into())),
                                    };
                                    let _ = tx.send(match r {
                                        Ok(()) => OpResult::Msg(format!("{email} was removed and the team key was renewed."), true),
                                        Err(e) => OpResult::Msg(e.to_string(), false),
                                    });
                                });
                            }
                            if flat_button(ui, "Cancel").clicked() {
                                su.confirm_remove = None;
                            }
                        });
                    }
                }
                // add a member, inside the team it is for
                if t.can_manage() && t.key.is_some() {
                    divider(ui);
                    ui.add_space(2.0);
                    let open = su.add_open && su.member_team == t.id;
                    if let Some((_, who)) = su.candidate.clone().filter(|c| c.0 == t.id) {
                        self.member_candidate_ui(ui, su, &server, &token, t.id, &t.role, who);
                    } else if open {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut su.member_email).hint_text("Email (they need an account on this server)").desired_width((ui.available_width() - 170.0).max(160.0)).margin(Vec2::new(10.0, 8.0)));
                            if ui.add_enabled(su.op.is_none() && !su.member_email.trim().is_empty(), egui::Button::new(RichText::new("Look up").size(12.5)).corner_radius(6).min_size(Vec2::new(0.0, 32.0))).clicked() {
                                let (tx, rx) = channel();
                                su.op = Some(rx);
                                let (s2, t2, team, email) = (server.clone(), token.clone(), t.id, su.member_email.clone());
                                std::thread::spawn(move || {
                                    let _ = tx.send(match tm::lookup(&Api::new(&s2, Some(t2)), team, &email) {
                                        Ok(v) => OpResult::Candidate(team, v),
                                        Err(e) => OpResult::Msg(e.to_string(), false),
                                    });
                                });
                            }
                            if flat_button(ui, "Cancel").clicked() {
                                su.add_open = false;
                                su.member_email.clear();
                            }
                        });
                    } else if icon_text_button(ui, crate::icons::Icon::User, "Add member").clicked() {
                        su.add_open = true;
                        su.member_team = t.id;
                        su.member_email.clear();
                        su.candidate = None;
                    }
                }
            });
            ui.add_space(10.0);
        }
        // create
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut su.team_name).hint_text("New team name").desired_width(260.0).margin(Vec2::new(10.0, 8.0)));
            if ui.add_enabled(su.op.is_none() && !su.team_name.trim().is_empty(), egui::Button::new(RichText::new("Create team").size(12.5)).corner_radius(6).min_size(Vec2::new(0.0, 32.0))).clicked() {
                if let Some(xp) = from_hex32(&cfg.x_public) {
                    let (tx, rx) = channel();
                    su.op = Some(rx);
                    let (s2, t2, name) = (server.clone(), token.clone(), std::mem::take(&mut su.team_name));
                    self.sync_rt.want_now = true;
                    std::thread::spawn(move || {
                        let _ = tx.send(match tm::create_team(&Api::new(&s2, Some(t2)), &xp, &name) {
                            Ok(_) => OpResult::Msg(format!("Team \"{name}\" created."), true),
                            Err(e) => OpResult::Msg(e.to_string(), false),
                        });
                    });
                }
            }
        });
    }

    /// The person found by "Look up": their fingerprint to compare, the role to give, and the final yes.
    #[allow(clippy::too_many_arguments)]
    fn member_candidate_ui(&mut self, ui: &mut egui::Ui, su: &mut SyncUi, server: &str, token: &str, team: i64, my_role: &str, who: sync_client::teams::MemberLookup) {
        use sync_client::teams as tm;
        ui.label(RichText::new(format!("{} has the fingerprint", who.email)).size(12.0).color(TEXT()));
        ui.label(RichText::new(&who.fingerprint).monospace().size(13.0).color(AMBER()));
        ui.label(RichText::new("Ask them to read the number in their own Settings → Sync. Add them only if it is the same.").size(11.5).color(MUTED()));
        ui.add_space(4.0);
        let options = grantable(my_role);
        if !options.contains(&su.member_role.as_str()) {
            su.member_role = "editor".into();
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Role").size(12.0).color(MUTED()));
            egui::ComboBox::from_id_salt("new_member_role").width(90.0).selected_text(role_name(&su.member_role)).show_ui(ui, |ui| {
                for r in options {
                    ui.selectable_value(&mut su.member_role, r.to_string(), role_name(r));
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if primary_button(ui, "The numbers match, add").clicked() {
                let key = self.sync_rt.teams.iter().find(|t| t.id == team).and_then(|t| t.key.clone());
                if let Some(key) = key {
                    let (tx, rx) = channel();
                    su.op = Some(rx);
                    let (s2, t2, who2, role) = (server.to_string(), token.to_string(), who.clone(), su.member_role.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send(match tm::add_member(&Api::new(&s2, Some(t2)), team, &who2, &key, &role) {
                            Ok(()) => OpResult::Msg(format!("{} is in the team. They get the key on their next sync.", who2.email), true),
                            Err(e) => OpResult::Msg(e.to_string(), false),
                        });
                    });
                    su.candidate = None;
                    su.add_open = false;
                    su.member_email.clear();
                    self.sync_rt.want_now = true;
                }
            }
            if flat_button(ui, "Cancel").clicked() {
                su.candidate = None;
            }
        });
    }

    pub fn sync_sign_out(&mut self) {
        if let Some(cfg) = self.vault.sync.take() {
            let (server, token) = (cfg.server, cfg.token);
            std::thread::spawn(move || {
                let _ = Api::new(&server, Some(token)).logout();
            });
        }
        let _ = std::fs::remove_file(super::state::state_path());
        self.sync_rt = Default::default();
        self.dirty = true;
        self.sync_rt.ui.message = Some(("Signed out. Your data stays on this computer.".into(), true));
    }
}


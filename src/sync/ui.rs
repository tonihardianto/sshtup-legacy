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
    /// someone about to be added to a team: waits for the owner to confirm the fingerprint
    pub candidate: Option<(i64, sync_client::teams::MemberLookup)>,
    /// (item id, scope, warning) waiting for a yes before a secret is shared with a team
    pub pending_move: Option<(String, String, String)>,
    /// where a new item will live, chosen in its form (None until the form first draws; "u" is only me)
    pub new_scope: Option<String>,
    /// (team id, user id, email) waiting for a yes before someone is removed
    pub confirm_remove: Option<(i64, i64, String)>,
}

pub enum OpResult {
    Msg(String, bool),
    Devices(Vec<serde_json::Value>),
    Candidate(i64, sync_client::teams::MemberLookup),
}

fn field(ui: &mut egui::Ui, text: &mut String, hint: &str, password: bool) {
    ui.add(egui::TextEdit::singleline(text).password(password).hint_text(hint).desired_width(300.0).margin(Vec2::new(10.0, 8.0)));
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
            ui.label(RichText::new("Not available in a demo run.").size(12.0).color(MUTED));
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
            }
        }

        if let Some(code) = su.recovery.clone() {
            ui.label(RichText::new("Your recovery key").strong().size(13.0).color(TEXT));
            ui.label(RichText::new("It is the only way back in if you forget the sync password. It is shown once and the server cannot give it back.").size(12.0).color(AMBER));
            ui.add_space(4.0);
            ui.label(RichText::new(code).monospace().size(14.0).color(TEXT));
            ui.add_space(4.0);
            ui.checkbox(&mut su.recovery_saved, "I saved it somewhere safe");
            if su.recovery_saved && primary_button(ui, "Done").clicked() {
                su.recovery = None;
                su.recovery_saved = false;
            }
            ui.add_space(8.0);
        }

        if let Some(cfg) = self.vault.sync.clone() {
            ui.label(RichText::new(format!("Signed in as {} on {}", cfg.email, cfg.server)).size(12.5).color(GREEN));
            let rt = &self.sync_rt;
            let status = match (&rt.last_error, rt.last_ok_ms) {
                (Some(e), _) => RichText::new(e).size(12.0).color(RED),
                (None, Some(t)) => RichText::new(format!("Last sync {} ({})", ago(t), rt.summary)).size(12.0).color(MUTED),
                (None, None) => RichText::new(if rt.rx.is_some() { "Syncing…" } else { "Waiting for the first sync" }).size(12.0).color(MUTED),
            };
            ui.label(status);
            if !self.sync_rt.state.as_ref().map(|s| s.replaced.is_empty()).unwrap_or(true) {
                let n = self.sync_rt.state.as_ref().map(|s| s.replaced.len()).unwrap_or(0);
                ui.label(RichText::new(format!("{n} older version(s) were replaced by newer ones from other devices. They are kept, encrypted, on this computer.")).size(12.0).color(MUTED));
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
            ui.label(RichText::new("Change the sync password").strong().size(12.5).color(TEXT));
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
                ui.label(RichText::new("Devices").strong().size(12.5).color(TEXT));
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
                        ui.label(RichText::new(format!("{name}{}  ·  last seen {}", if cur { " (this computer)" } else { "" }, ago(d["last_seen"].as_u64().unwrap_or(0)))).size(12.0).color(MUTED));
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
            ui.label(RichText::new("Keep your hosts, identities, keys and snippets in step on all your devices. Everything is encrypted on this computer before it is sent; the server only stores scrambled data.").size(12.0).color(MUTED));
            if !encrypted {
                ui.add_space(4.0);
                ui.label(RichText::new("Turn on the master password first (Security, below). Sync keeps its keys inside the encrypted vault.").size(12.0).color(AMBER));
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
                ui.label(RichText::new("Working…").size(12.0).color(MUTED));
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
            ui.label(RichText::new(m).size(12.0).color(if *ok { GREEN } else { RED }));
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
    pub fn team_tag(&self, kind: &str, id: u64) -> String {
        let Some(st) = &self.sync_rt.state else { return String::new() };
        let Some(t) = st.items.get(&super::state::item_id(kind, id)) else { return String::new() };
        let Some(team) = t.scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()) else { return String::new() };
        let name = self.vault.sync.as_ref().and_then(|c| c.teams.iter().find(|x| x.id == team)).map(|x| x.name.clone()).unwrap_or_else(|| "team".into());
        format!("  ·  Team {name}")
    }

    /// In the form of a NEW item: choose where it will live. Starts on the team of the group it is created in.
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
            self.sync_rt.ui.new_scope = Some(inherited.unwrap_or_else(|| "u".into()));
        }
        let cur = self.sync_rt.ui.new_scope.clone().unwrap_or_else(|| "u".into());
        let name_of = |scope: &str| -> String {
            match scope.strip_prefix("t:").and_then(|n| n.parse::<i64>().ok()) {
                Some(t) => format!("Team {}", cfg.teams.iter().find(|x| x.id == t).map(|x| x.name.as_str()).unwrap_or("?")),
                None => "Only me".to_string(),
            }
        };
        ui.add_space(8.0);
        ui.label(RichText::new("Who can see this").size(12.0).color(MUTED));
        egui::ComboBox::from_id_salt("new_share_scope").width(ui.available_width() - 8.0).selected_text(name_of(&cur)).show_ui(ui, |ui| {
            if ui.selectable_label(cur == "u", "Only me").clicked() {
                self.sync_rt.ui.new_scope = Some("u".into());
            }
            for t in &cfg.teams {
                let sc = super::teams::scope_of(t.id);
                if ui.selectable_label(cur == sc, format!("Team {}", t.name)).clicked() {
                    self.sync_rt.ui.new_scope = Some(sc);
                }
            }
        });
        if cur != "u" {
            ui.label(RichText::new("Every team member can read saved passwords and keys on this item.").size(11.5).color(AMBER));
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
            self.notify(format!("Shared with team {name}"), GREEN);
        }
    }

    /// In an item's editor: choose where the item lives (just me, or one of my teams).
    pub fn sync_share_row(&mut self, ui: &mut egui::Ui, kind: &str, id: u64) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        if cfg.teams.is_empty() || self.sync_rt.state.is_none() {
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
        ui.label(RichText::new(if kind == "group" { "Who can see this group" } else { "Who can see this" }).size(12.0).color(MUTED));
        if kind == "group" {
            let n = super::teams::group_items(&self.vault, id).len();
            ui.label(RichText::new(format!("Applies to the group, its sub-groups, their hosts and the keys and identities they use ({n} items). New hosts added to it follow.")).size(11.5).color(MUTED));
        }
        let mut pick: Option<String> = None;
        egui::ComboBox::from_id_salt("share_scope").width(ui.available_width() - 8.0).selected_text(name_of(&cur)).show_ui(ui, |ui| {
            if ui.selectable_label(cur == "u", "Only me").clicked() {
                pick = Some("u".into());
            }
            for t in &cfg.teams {
                let sc = super::teams::scope_of(t.id);
                if ui.selectable_label(cur == sc, format!("Team {}", t.name)).clicked() {
                    pick = Some(sc);
                }
            }
        });
        let mut confirmed = false;
        if let Some((pid, psc, warn)) = self.sync_rt.ui.pending_move.clone() {
            if pid == item {
                ui.label(RichText::new(warn).size(12.0).color(AMBER));
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
                        self.notify(format!("{n} item{} moved", if n == 1 { "" } else { "s" }), GREEN);
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
        ui.add_space(8.0);
        ui.label(RichText::new("Teams").strong().size(12.5).color(TEXT));
        if let Some(xp) = from_hex32(&cfg.x_public) {
            ui.label(RichText::new(format!("Your fingerprint: {}", tm::own_fingerprint(&xp))).monospace().size(11.5).color(MUTED));
            ui.label(RichText::new("The owner of a team sees this number when adding you. Read it to them (phone, chat) before they confirm.").size(11.5).color(MUTED));
        }
        let (server, token) = (cfg.server.clone(), cfg.token.clone());
        for t in self.sync_rt.teams.clone() {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Team {}{}", t.name, if t.owner { " (you own it)" } else { "" })).size(12.5).color(TEXT));
            if !t.ready() {
                ui.label(RichText::new(if t.owner { "The team key is being renewed after a removal." } else { "Waiting for the owner to renew the team key. Shared items are paused until then." }).size(11.5).color(AMBER));
            }
            for m in &t.members {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("   {} — {}", m.email, m.fingerprint)).monospace().size(11.0).color(MUTED));
                    if t.owner && m.role != "owner" && t.ready() && t.stale_items == 0 && ui.add_enabled(su.op.is_none(), egui::Button::new("Remove").small()).clicked() {
                        su.confirm_remove = Some((t.id, m.user_id, m.email.clone()));
                    }
                });
            }
            if let Some((tid, uid, email)) = su.confirm_remove.clone().filter(|c| c.0 == t.id) {
                ui.label(RichText::new(format!("Remove {email}? The team key is replaced and every shared item is sealed again, so they cannot read anything new. They keep what they already saw: change any password they could read.")).size(12.0).color(AMBER));
                ui.horizontal(|ui| {
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
                let _ = tid;
            }
        }
        // add a member
        let owned: Vec<_> = self.sync_rt.teams.iter().filter(|t| t.owner && t.key.is_some()).cloned().collect();
        if !owned.is_empty() {
            ui.add_space(4.0);
            if !owned.iter().any(|t| t.id == su.member_team) {
                su.member_team = owned[0].id;
            }
            egui::ComboBox::from_id_salt("invite_team").selected_text(owned.iter().find(|t| t.id == su.member_team).map(|t| t.name.clone()).unwrap_or_default()).show_ui(ui, |ui| {
                for t in &owned {
                    ui.selectable_value(&mut su.member_team, t.id, &t.name);
                }
            });
            field(ui, &mut su.member_email, "Email of the person to add (they need an account on this server)", false);
            if ui.add_enabled(su.op.is_none() && !su.member_email.trim().is_empty(), egui::Button::new("Look up")).clicked() {
                let (tx, rx) = channel();
                su.op = Some(rx);
                let (s2, t2, team, email) = (server.clone(), token.clone(), su.member_team, su.member_email.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(match tm::lookup(&Api::new(&s2, Some(t2)), team, &email) {
                        Ok(v) => OpResult::Candidate(team, v),
                        Err(e) => OpResult::Msg(e.to_string(), false),
                    });
                });
            }
        }
        if let Some((team, who)) = su.candidate.clone() {
            ui.label(RichText::new(format!("{} has the fingerprint:", who.email)).size(12.0).color(TEXT));
            ui.label(RichText::new(&who.fingerprint).monospace().size(13.0).color(AMBER));
            ui.label(RichText::new("Ask them to read the number in their own Settings → Sync. Add them only if it is the same.").size(12.0).color(MUTED));
            ui.horizontal(|ui| {
                if primary_button(ui, "The numbers match, add to the team").clicked() {
                    let key = self.sync_rt.teams.iter().find(|t| t.id == team).and_then(|t| t.key.clone());
                    if let Some(key) = key {
                        let (tx, rx) = channel();
                        su.op = Some(rx);
                        let (s2, t2, who2) = (server.clone(), token.clone(), who.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(match tm::add_member(&Api::new(&s2, Some(t2)), team, &who2, &key) {
                                Ok(()) => OpResult::Msg(format!("{} is in the team. They get the key on their next sync.", who2.email), true),
                                Err(e) => OpResult::Msg(e.to_string(), false),
                            });
                        });
                        su.candidate = None;
                        su.member_email.clear();
                        self.sync_rt.want_now = true;
                    }
                }
                if flat_button(ui, "Cancel").clicked() {
                    su.candidate = None;
                }
            });
        }
        // create
        ui.add_space(4.0);
        field(ui, &mut su.team_name, "New team name", false);
        if ui.add_enabled(su.op.is_none() && !su.team_name.trim().is_empty(), egui::Button::new("Create team")).clicked() {
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


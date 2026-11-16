//! Collaboration: shared-terminal sessions. The team list shown here comes from Sync.
//! Shared terminals go through the relay built into your Sync server and need a Sync account;
//! the terminal traffic is end-to-end encrypted, the server only passes it along.

use crate::app::{App, Screen};
use crate::collab_net::{start_host, HostLink, Status};
use crate::model::*;
use crate::screens::page_header;
use crate::ui::*;
use eframe::egui::{self, Frame, Margin, RichText, Vec2};

/// One terminal tab that is being shared.
#[derive(Clone)]
pub struct Share {
    pub tab_id: u64,
    /// Local-only code, used when no relay is set up.
    pub local_code: String,
    pub can_type: bool,
    pub started: u64,
    pub link: Option<HostLink>,
}

impl Share {
    pub fn code(&self) -> String {
        match &self.link {
            Some(l) => l.state.lock().unwrap().code.clone(),
            None => self.local_code.clone(),
        }
    }
    /// What to show under the terminal's name, and whether it is good news.
    pub fn status_line(&self) -> (String, bool) {
        let mode = if self.can_type { "guests can type" } else { "view only" };
        match &self.link {
            None => (format!("Code {} · {} · local only, nobody can join", self.local_code, mode), false),
            Some(l) => {
                let s = l.state.lock().unwrap();
                match s.status {
                    Status::Connecting => ("Connecting to the relay…".into(), true),
                    Status::Failed => (format!("Relay problem: {}", s.error), false),
                    Status::Live => {
                        let who = if s.guests.is_empty() { "no guests yet".to_string() } else { format!("{} here: {}", s.guests.len(), s.guests.iter().map(|g| g.1.clone()).collect::<Vec<_>>().join(", ")) };
                        (format!("Code {} · {} · {}", s.code, mode, who), true)
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct CollabState {
    pub shares: Vec<Share>,
    pub join_code: String,
    /// The small "Join shared session" window opened from the Hosts page.
    pub show_join: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CollabAct {
    None,
    Start,
    Stop,
    ToggleType,
    CopyCode,
    OpenScreen,
}

/// The entries of a terminal's Collaboration menu (toolbar button or right-click on its tab).
pub fn collab_items(ui: &mut egui::Ui, shared: Option<&Share>) -> CollabAct {
    let mut act = CollabAct::None;
    ui.set_min_width(200.0);
    match shared {
        None => {
            if ui.button("Share this terminal").clicked() {
                act = CollabAct::Start;
                ui.close();
            }
        }
        Some(s) => {
            label_muted(ui, &format!("Session code  {}", s.code()));
            if ui.button("Copy session code").clicked() {
                act = CollabAct::CopyCode;
                ui.close();
            }
            if ui.button(if s.can_type { "Make view only" } else { "Let guests type" }).clicked() {
                act = CollabAct::ToggleType;
                ui.close();
            }
            if ui.button("Stop sharing").clicked() {
                act = CollabAct::Stop;
                ui.close();
            }
        }
    }
    ui.separator();
    if ui.button("Open Collaboration").clicked() {
        act = CollabAct::OpenScreen;
        ui.close();
    }
    act
}

fn make_code(seed: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).hash(&mut h);
    let mut n = h.finish();
    const AL: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut s = String::new();
    for i in 0..8 {
        if i == 4 {
            s.push('-');
        }
        s.push(AL[(n % AL.len() as u64) as usize] as char);
        n /= AL.len() as u64;
    }
    s
}

impl App {
    /// The small "Collaboration" menu in a terminal's toolbar.
    pub fn collab_menu(&mut self, ui: &mut egui::Ui, tab_id: u64) {
        let shared = self.collab.shares.iter().find(|s| s.tab_id == tab_id).cloned();
        let label = if shared.is_some() { "Sharing" } else { "Collaboration" };
        let mut act = CollabAct::None;
        ui.menu_button(RichText::new(label).color(if shared.is_some() { GREEN } else { TEXT }), |ui| {
            act = collab_items(ui, shared.as_ref());
        });
        self.collab_apply(ui, tab_id, act);
    }

    pub fn collab_apply(&mut self, ui: &egui::Ui, tab_id: u64, act: CollabAct) {
        match act {
            CollabAct::None => {}
            CollabAct::Start => self.collab_start(tab_id),
            CollabAct::Stop => {
                self.collab.shares.retain(|s| s.tab_id != tab_id);
                self.notify("Stopped sharing this terminal".into(), AMBER);
            }
            CollabAct::ToggleType => {
                if let Some(s) = self.collab.shares.iter_mut().find(|s| s.tab_id == tab_id) {
                    s.can_type = !s.can_type;
                    if let Some(l) = &s.link {
                        l.set_can_type(s.can_type);
                    }
                }
            }
            CollabAct::CopyCode => {
                if let Some(s) = self.collab.shares.iter().find(|s| s.tab_id == tab_id) {
                    ui.ctx().copy_text(s.code());
                    self.notify("Session code copied".into(), GREEN);
                }
            }
            CollabAct::OpenScreen => {
                self.screen = crate::app::Screen::Collaboration;
                self.active = None;
            }
        }
    }

    fn collab_start(&mut self, tab_id: u64) {
        if self.collab.shares.iter().any(|s| s.tab_id == tab_id) {
            return;
        }
        let Some(cfg) = self.vault.sync.clone() else {
            self.notify("Turn on Sync first: shared terminals use your Sync account and the relay of your Sync server.".into(), AMBER);
            return;
        };
        let link = self.tabs.iter().find(|t| t.id == tab_id).map(|t| start_host(cfg.server, cfg.token, t.sess.parser.clone(), t.sess.writer_handle(), t.sess.shared.clone(), self.ctx.clone()));
        let msg = if link.is_some() { "Sharing started. Guests join with the session code" } else { "That terminal is gone" };
        self.collab.shares.push(Share { tab_id, local_code: make_code(tab_id), can_type: false, started: now_secs(), link });
        self.notify(msg.into(), if self.collab.shares.last().map(|s| s.link.is_some()).unwrap_or(false) { GREEN } else { AMBER });
    }

    pub fn collab_join(&mut self) {
        let Some(cfg) = self.vault.sync.clone() else {
            self.notify("Turn on Sync first: joining a shared terminal needs an account on the same Sync server as the host.".into(), AMBER);
            return;
        };
        let share = self.collab.join_code.trim().to_string();
        let Some((code, _)) = crate::wire::parse_share(&share) else {
            self.notify("Paste the whole session code, including the part after the ~ sign".into(), RED);
            return;
        };
        // a one-time ticket from the sync server; the guest process shows it to the relay
        let pass = match sync_client::api::Api::new(&cfg.server, Some(cfg.token)).relay_pass() {
            Ok(p) => p,
            Err(e) => {
                self.notify(crate::sync::runtime::describe(&e), RED);
                return;
            }
        };
        let Ok(exe) = std::env::current_exe() else {
            self.notify("Could not start the guest view".into(), RED);
            return;
        };
        let name = std::env::var("USER").unwrap_or_else(|_| "guest".into());
        let mut spec = self.local_spec();
        spec.title = format!("Shared {code}");
        spec.program = exe.to_string_lossy().into_owned();
        spec.args = vec!["--relay-join".into(), pass.addr, share, name, pass.ticket];
        self.collab.join_code.clear();
        self.open_session(spec);
    }

    pub fn screen_collab(&mut self, ui: &mut egui::Ui) {
        // forget shares of closed terminals
        let open: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        self.collab.shares.retain(|s| open.contains(&s.tab_id));

        let mut open_sync = false;
        let mut start: Option<u64> = None;
        let mut stop: Option<u64> = None;
        let mut copy: Option<String> = None;
        let mut type_toggle: Option<u64> = None;

        page_header(ui, "Collaboration", |_| {});

        let tabs: Vec<(u64, String)> = self.tabs.iter().map(|t| (t.id, t.title.clone())).collect();
        let synced_for_relay = self.vault.sync.is_some();
        let mut join_go = false;
        if !self.collab.shares.is_empty() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let w = ui.available_width().min(760.0) - 1.0;
            ui.set_max_width(w);
            ui.spacing_mut().item_spacing = Vec2::splat(10.0);

            // honest note
            Frame::new().fill(AMBER.linear_multiply(0.12)).stroke(egui::Stroke::new(1.0, AMBER.linear_multiply(0.5))).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(w - 26.0);
                if synced_for_relay {
                    ui.label(RichText::new("Relay: your Sync server").strong().size(13.0).color(GREEN));
                    ui.label(RichText::new("Shared terminals go through the relay of your Sync server and are end-to-end encrypted: the server cannot read them. Guests need an account on the same server.").size(12.0).color(MUTED));
                } else {
                    ui.label(RichText::new("Sync is off").strong().size(13.0).color(AMBER));
                    ui.label(RichText::new("Shared terminals use the relay of your Sync server. Turn on Sync in Settings, then share a terminal.").size(12.0).color(MUTED));
                }
            });

            // teams come from Sync: one list, one place to manage them
            section_title(ui, "TEAMS");
            let synced = self.vault.sync.is_some();
            if !synced {
                label_muted(ui, "Teams are part of Sync. Turn on Sync to create a team and share hosts with it.");
            } else if self.sync_rt.teams.is_empty() {
                label_muted(ui, "No teams yet. Create one in Settings → Sync.");
            }
            for t in &self.sync_rt.teams {
                Frame::new().fill(CARD).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.label(RichText::new(format!("{}{}", t.name, if t.owner { "  ·  you own it" } else { "" })).size(14.0));
                    for m in &t.members {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                            let initial = m.email.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                            badge(ui.painter(), r, &initial, icon_color(&m.email));
                            ui.label(RichText::new(&m.email).size(12.5));
                            ui.label(RichText::new(&m.role).size(11.5).color(MUTED));
                        });
                    }
                });
            }
            if synced && flat_button(ui, "Manage teams in Settings → Sync").clicked() {
                open_sync = true;
            }

            // join someone else's session
            ui.add_space(6.0);
            section_title(ui, "JOIN A SHARED TERMINAL");
            Frame::new().fill(CARD).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(w - 24.0);
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.collab.join_code).hint_text("Session code, e.g. abcd-2345").desired_width(260.0).margin(Vec2::new(8.0, 7.0)));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if primary_button(ui, "Join").clicked() || enter {
                        join_go = true;
                    }
                });
            });

            // shared sessions
            ui.add_space(6.0);
            section_title(ui, "SHARED TERMINALS");
            if tabs.is_empty() {
                label_muted(ui, "Open a terminal from Hosts, then share it here or from its toolbar.");
            }
            for (id, title) in &tabs {
                let share = self.collab.shares.iter().find(|s| s.tab_id == *id).cloned();
                Frame::new().fill(CARD).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                        badge(ui.painter(), r, "icon:terminal", icon_color(title));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(RichText::new(title).size(14.0));
                            match &share {
                                None => ui.label(RichText::new("Not shared").size(11.5).color(MUTED)),
                                Some(s) => {
                                    let (t, good) = s.status_line();
                                    ui.label(RichText::new(t).size(11.5).color(if good { GREEN } else { AMBER }))
                                }
                            };
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &share {
                            None => {
                                if flat_button(ui, "Start sharing").clicked() {
                                    start = Some(*id);
                                }
                            }
                            Some(s) => {
                                if flat_button(ui, "Stop").clicked() {
                                    stop = Some(*id);
                                }
                                if flat_button(ui, "Copy code").clicked() {
                                    copy = Some(s.code());
                                }
                                let mut can = s.can_type;
                                ui.label(RichText::new("Guests can type").size(12.0).color(MUTED));
                                if toggle(ui, &mut can).changed() {
                                    type_toggle = Some(*id);
                                }
                            }
                        });
                    });
                });
            }
        });

        if join_go {
            self.collab_join();
        }
        if open_sync {
            self.screen = Screen::Settings;
        }
        if let Some(id) = start {
            self.collab_start(id);
        }
        if let Some(id) = stop {
            self.collab.shares.retain(|s| s.tab_id != id);
        }
        if let Some(id) = type_toggle {
            if let Some(s) = self.collab.shares.iter_mut().find(|s| s.tab_id == id) {
                s.can_type = !s.can_type;
                if let Some(l) = &s.link {
                    l.set_can_type(s.can_type);
                }
            }
        }
        if let Some(c) = copy {
            ui.ctx().copy_text(c);
            self.notify("Session code copied".into(), GREEN);
        }
    }
}

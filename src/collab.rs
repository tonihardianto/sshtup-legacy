//! Collaboration: shared-terminal sessions. The team list shown here comes from Sync.
//! Shared terminals go through the relay built into your Sync server and need a Sync account;
//! the terminal traffic is end-to-end encrypted, the server only passes it along.

use crate::app::{App, Screen};
use crate::collab_net::{guest_color, start_host, GuestInfo, HostLink, Status};
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
    pub fn guests(&self) -> Vec<GuestInfo> {
        self.link.as_ref().map(|l| l.state.lock().unwrap().guests.clone()).unwrap_or_default()
    }
    /// What to show under the terminal's name, and whether it is good news.
    pub fn status_line(&self) -> (String, bool) {
        match &self.link {
            None => (format!("Code {} · local only, nobody can join", self.local_code), false),
            Some(l) => {
                let s = l.state.lock().unwrap();
                match s.status {
                    Status::Connecting => ("Connecting to the relay…".into(), true),
                    Status::Failed => (format!("Relay problem: {}", s.error), false),
                    Status::Live => {
                        let who = if s.guests.is_empty() {
                            "no guests yet".to_string()
                        } else {
                            let typing = s.guests.iter().filter(|g| g.can_type).count();
                            format!("{} here: {} · {} can type", s.guests.len(), s.guests.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", "), typing)
                        };
                        (format!("Code {} · {}", s.code, who), true)
                    }
                }
            }
        }
    }
}

impl CollabState {
    /// The people in a shared terminal, for the avatars on its tab: the host first, then the guests.
    pub fn people(&self, tab_id: u64, me: String) -> Vec<(String, egui::Color32)> {
        let Some(s) = self.shares.iter().find(|s| s.tab_id == tab_id) else { return vec![] };
        let Some(l) = &s.link else { return vec![] };
        let st = l.state.lock().unwrap();
        if st.status != Status::Live {
            return vec![];
        }
        let mut v = vec![(me, BLUE())];
        for g in st.guests.iter().take(3) {
            let c = guest_color(g.gid);
            v.push((g.name.clone(), egui::Color32::from_rgb(c[0], c[1], c[2])));
        }
        v
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
    /// Let this guest type (true) or make them view only (false).
    GuestType(u64, bool),
    CopyCode,
    OpenScreen,
}

/// The entries of a terminal's Collaboration menu (the card on the terminal or right-click on its tab).
pub fn collab_items(ui: &mut egui::Ui, shared: Option<&Share>, can_share: bool) -> CollabAct {
    let mut act = CollabAct::None;
    ui.set_min_width(200.0);
    ui.spacing_mut().item_spacing.y = 3.0;
    match shared {
        None if !can_share => label_muted(ui, "Shared with you; it cannot be shared again"),
        None => {
            if crate::ui::icon_item(ui, crate::icons::Icon::Users, "Share this terminal", crate::ui::TEXT()).clicked() {
                act = CollabAct::Start;
                ui.close();
            }
        }
        Some(s) => {
            label_muted(ui, &format!("Session code  {}", s.code()));
            if crate::ui::icon_item(ui, crate::icons::Icon::Copy, "Copy session code", crate::ui::TEXT()).clicked() {
                act = CollabAct::CopyCode;
                ui.close();
            }
            for g in s.guests() {
                let label = format!("{} {}", if g.can_type { "✓" } else { "○" }, if g.can_type { format!("{} can type", g.name) } else { format!("{} · view only", g.name) });
                if ui.button(label).clicked() {
                    act = CollabAct::GuestType(g.gid, !g.can_type);
                    ui.close();
                }
            }
            if crate::ui::icon_item(ui, crate::icons::Icon::Stop, "Stop sharing", crate::ui::RED()).clicked() {
                act = CollabAct::Stop;
                ui.close();
            }
        }
    }
    ui.separator();
    if crate::ui::icon_item(ui, crate::icons::Icon::Group, "Open Collaboration", crate::ui::TEXT()).clicked() {
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
    /// How this person is shown to the others in a shared terminal: the name they gave, else the system user.
    pub fn my_name(&self, fallback: &str) -> String {
        let n = self.vault.settings.user_name.trim();
        if !n.is_empty() {
            return n.to_string();
        }
        std::env::var("USER").unwrap_or_else(|_| fallback.into())
    }

    pub fn collab_apply(&mut self, ui: &egui::Ui, tab_id: u64, act: CollabAct) {
        match act {
            CollabAct::None => {}
            CollabAct::Start => self.collab_start(tab_id),
            CollabAct::Stop => {
                self.collab.shares.retain(|s| s.tab_id != tab_id);
                self.notify("Stopped sharing this terminal".into(), AMBER());
            }
            CollabAct::GuestType(gid, on) => {
                if let Some(l) = self.collab.shares.iter().find(|s| s.tab_id == tab_id).and_then(|s| s.link.as_ref()) {
                    l.set_guest_can_type(gid, on);
                }
            }
            CollabAct::CopyCode => {
                if let Some(s) = self.collab.shares.iter().find(|s| s.tab_id == tab_id) {
                    ui.ctx().copy_text(s.code());
                    self.notify("Session code copied".into(), GREEN());
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
        // a guest cannot hand the host's terminal on: the host never agreed to that person
        if self.tabs.iter().any(|t| t.id == tab_id && t.sess.spec.is_guest()) {
            self.notify("This terminal belongs to someone else's session, so it cannot be shared again.".into(), AMBER());
            return;
        }
        let Some(cfg) = self.vault.sync.clone() else {
            self.notify("Turn on Sync first: shared terminals use your Sync account and the relay of your Sync server.".into(), AMBER());
            return;
        };
        let link = self.tabs.iter().find(|t| t.id == tab_id).map(|t| start_host(cfg.server, cfg.token, t.sess.parser.clone(), t.sess.writer_handle(), t.sess.shared.clone(), self.ctx.clone(), self.my_name("host")));
        let msg = if link.is_some() { "Sharing started. Guests join with the session code" } else { "That terminal is gone" };
        self.collab.shares.push(Share { tab_id, local_code: make_code(tab_id), started: now_secs(), link });
        self.notify(msg.into(), if self.collab.shares.last().map(|s| s.link.is_some()).unwrap_or(false) { GREEN() } else { AMBER() });
    }

    pub fn collab_join(&mut self) {
        let Some(cfg) = self.vault.sync.clone() else {
            self.notify("Turn on Sync first: joining a shared terminal needs an account on the same Sync server as the host.".into(), AMBER());
            return;
        };
        let share = self.collab.join_code.trim().to_string();
        let Some((code, _)) = crate::wire::parse_share(&share) else {
            self.notify("Paste the whole session code, including the part after the ~ sign".into(), RED());
            return;
        };
        // a one-time ticket from the sync server; the guest process shows it to the relay
        let pass = match sync_client::api::Api::new(&cfg.server, Some(cfg.token)).relay_pass() {
            Ok(p) => p,
            Err(e) => {
                self.notify(crate::sync::runtime::describe(&e), RED());
                return;
            }
        };
        let Ok(exe) = std::env::current_exe() else {
            self.notify("Could not start the guest view".into(), RED());
            return;
        };
        let name = self.my_name("guest");
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
        let mut guest_toggle: Option<(u64, u64, bool)> = None;

        page_header(ui, "Collaboration", |_| {});

        let tabs: Vec<(u64, String)> = self.tabs.iter().filter(|t| t.pane_of.is_none()).map(|t| (t.id, t.title.clone())).collect();
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
            Frame::new().fill(AMBER().linear_multiply(0.12)).stroke(egui::Stroke::new(1.0, AMBER().linear_multiply(0.5))).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(w - 26.0);
                if synced_for_relay {
                    ui.label(RichText::new("Relay: your Sync server").strong().size(13.0).color(GREEN()));
                    ui.label(RichText::new("Shared terminals go through the relay of your Sync server and are end-to-end encrypted: the server cannot read them. Guests need an account on the same server.").size(12.0).color(MUTED()));
                } else {
                    ui.label(RichText::new("Sync is off").strong().size(13.0).color(AMBER()));
                    ui.label(RichText::new("Shared terminals use the relay of your Sync server. Turn on Sync in Settings, then share a terminal.").size(12.0).color(MUTED()));
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
                Frame::new().fill(CARD()).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.label(RichText::new(format!("{}{}", t.name, if t.owner { "  ·  you own it" } else { "" })).size(14.0));
                    for m in &t.members {
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                            let initial = m.email.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                            badge(ui.painter(), r, &initial, icon_color(&m.email));
                            ui.label(RichText::new(&m.email).size(12.5));
                            ui.label(RichText::new(&m.role).size(11.5).color(MUTED()));
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
            Frame::new().fill(CARD()).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
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
                label_muted(ui, "Open a terminal from Hosts, then share it here or from the card on the terminal.");
            }
            for (id, title) in &tabs {
                let share = self.collab.shares.iter().find(|s| s.tab_id == *id).cloned();
                Frame::new().fill(CARD()).corner_radius(10).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                        badge(ui.painter(), r, "icon:terminal", icon_color(title));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(RichText::new(title).size(14.0));
                            match &share {
                                None => ui.label(RichText::new("Not shared").size(11.5).color(MUTED())),
                                Some(s) => {
                                    let (t, good) = s.status_line();
                                    ui.label(RichText::new(t).size(11.5).color(if good { GREEN() } else { AMBER() }))
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
                            }
                        });
                    });
                    // who is here: one row per guest, with its own switch
                    if let Some(s) = &share {
                        for g in s.guests() {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                                let c = guest_color(g.gid);
                                avatar(ui.painter(), r.center(), 10.0, &g.name, egui::Color32::from_rgb(c[0], c[1], c[2]));
                                ui.label(RichText::new(&g.name).size(13.0));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let mut can = g.can_type;
                                    ui.label(RichText::new("Can type").size(12.0).color(MUTED()));
                                    if toggle(ui, &mut can).changed() {
                                        guest_toggle = Some((*id, g.gid, can));
                                    }
                                });
                            });
                        }
                    }
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
        if let Some((id, gid, on)) = guest_toggle {
            if let Some(l) = self.collab.shares.iter().find(|s| s.tab_id == id).and_then(|s| s.link.as_ref()) {
                l.set_guest_can_type(gid, on);
            }
        }
        if let Some(c) = copy {
            ui.ctx().copy_text(c);
            self.notify("Session code copied".into(), GREEN());
        }
    }
}

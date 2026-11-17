//! The window: tab strip, sidebar, terminal tabs, background sessions and the screenshot mode.

use crate::model::*;
use crate::schemes::{Scheme, SCHEMES};
use crate::term::{terminal_ui, PtySession, SessionSpec, TermOptions, TermViewState};
use crate::ui::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Pos2, RichText, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Hosts,
    Keychain,
    PortForwarding,
    Snippets,
    KnownHosts,
    Logs,
    Sftp,
    Collaboration,
    Settings,
}

impl Screen {
    pub const NAV: [Screen; 8] = [Screen::Hosts, Screen::Keychain, Screen::PortForwarding, Screen::Snippets, Screen::KnownHosts, Screen::Logs, Screen::Sftp, Screen::Collaboration];
    pub fn title(self) -> &'static str {
        match self {
            Screen::Hosts => "Hosts",
            Screen::Keychain => "Keychain",
            Screen::PortForwarding => "Port Forwarding",
            Screen::Snippets => "Snippets",
            Screen::KnownHosts => "Known Hosts",
            Screen::Logs => "Logs",
            Screen::Sftp => "SFTP",
            Screen::Collaboration => "Collaboration",
            Screen::Settings => "Settings",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Screen::Hosts => "icon:host",
            Screen::Keychain => "icon:key",
            Screen::PortForwarding => "icon:forward",
            Screen::Snippets => "icon:snippet",
            Screen::KnownHosts => "icon:shield",
            Screen::Logs => "icon:logs",
            Screen::Sftp => "icon:folder",
            Screen::Collaboration => "icon:users",
            Screen::Settings => "icon:settings",
        }
    }
    pub fn from_name(n: &str) -> Option<Screen> {
        let n = n.to_lowercase();
        Some(match n.as_str() {
            "hosts" => Screen::Hosts,
            "keychain" => Screen::Keychain,
            "pf" | "portforwarding" | "port-forwarding" => Screen::PortForwarding,
            "snippets" => Screen::Snippets,
            "knownhosts" | "known-hosts" => Screen::KnownHosts,
            "logs" => Screen::Logs,
            "sftp" => Screen::Sftp,
            "collab" | "collaboration" => Screen::Collaboration,
            "settings" => Screen::Settings,
            _ => return None,
        })
    }
}

pub enum Editor {
    None,
    Host(Host),
    Group(Group),
    Key(SshKey),
    Identity(Identity),
    Forward(PortForward),
    Snippet(Snippet),
}

#[derive(Clone)]
pub enum Del {
    Host(u64),
    Group(u64),
    Key(u64),
    Identity(u64),
    Forward(u64),
    Snippet(u64),
    KnownHost(usize),
    Log(PathBuf),
}

pub struct TermTab {
    pub id: u64,
    pub title: String,
    pub sess: PtySession,
    pub view: TermViewState,
    pub reconnect_at: Option<Instant>,
    pub focus_next: bool,
    /// The connection to the host is up (or has given up waiting); the "connecting" cover is gone.
    pub ready: bool,
    /// What is typed in the password window, and whether to keep it on the host.
    pub pw_text: String,
    pub pw_remember: bool,
}

pub struct ShotState {
    pub path: String,
    pub frames: u32,
    pub requested: bool,
    pub image: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
}

pub struct App {
    pub ctx: egui::Context,
    pub vault: Vault,
    pub dirty: bool,
    pub demo: bool,
    pub screen: Screen,
    pub tabs: Vec<TermTab>,
    pub active: Option<u64>,
    pub next_tab: u64,
    pub editor: Editor,
    pub editor_new: bool,
    pub search: String,
    pub cur_group: Option<u64>,
    pub selected: Option<u64>,
    pub forwards_live: HashMap<u64, PtySession>,
    pub toast: Option<(String, Instant, Color32)>,
    pub tz: i64,
    pub shot: Option<ShotState>,
    pub log_view: Option<(PathBuf, String)>,
    pub confirm: Option<Del>,
    pub confirm_import: bool,
    pub settings_tab: u8,
    pub host_picker: Option<String>,
    pub sftp: crate::sftp::SftpState,
    pub collab: crate::collab::CollabState,
    pub key_import: Option<crate::screens::KeyImport>,
    pub show_snippets_menu: bool,
    pub gen_key_label: String,
    pub edit_a: String,
    pub edit_b: String,
    pub kh: Option<Vec<crate::screens::KhRow>>,
    pub history: crate::autocomplete::History,
    pub hist_jobs: Vec<(u64, std::sync::mpsc::Receiver<Result<Vec<String>, String>>)>,
    /// Hosts whose history read was tried (or is running) in this run.
    pub hist_tried: std::collections::HashSet<u64>,
    /// Set while an encrypted vault has not been opened yet.
    pub lock: Option<crate::lock::LockState>,
    /// Present when the vault is stored encrypted; used to save every change.
    pub vault_key: Option<crate::vaultcrypt::VaultKey>,
    pub sec: crate::lock::SecurityUi,
    pub sync_rt: crate::sync::runtime::SyncRuntime,
}

pub fn find_scheme(name: &str) -> &'static Scheme {
    SCHEMES.iter().find(|s| s.name.eq_ignore_ascii_case(name)).unwrap_or(&SCHEMES[0])
}

fn local_tz_offset() -> i64 {
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let s = s.trim();
            if s.len() < 5 {
                return None;
            }
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let h: i64 = s[1..3].parse().ok()?;
            let m: i64 = s[3..5].parse().ok()?;
            Some(sign * (h * 3600 + m * 60))
        })
        .unwrap_or(0)
}

pub fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(h) = std::env::var("HOME") {
            return format!("{}/{}", h, rest);
        }
    }
    p.to_string()
}

pub struct SshParts {
    pub target: String,
    pub port: u16,
    pub key: Option<String>,
    pub jump: Option<String>,
    pub agent: bool,
    pub keepalive: u32,
    pub password: Option<String>,
    pub passphrase: Option<String>,
}

impl SshParts {
    pub fn known_hosts_file() -> String {
        data_dir().join("known_hosts").to_string_lossy().to_string()
    }
    /// Options shared by ssh and the background commands.
    pub fn ssh_opts(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "-o".into(),
            format!("ServerAliveInterval={}", self.keepalive),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", Self::known_hosts_file()),
        ];
        if let Some(k) = &self.key {
            a.push("-i".into());
            a.push(k.clone());
            a.push("-o".into());
            a.push("IdentitiesOnly=yes".into());
        }
        if let Some(j) = &self.jump {
            a.push("-J".into());
            a.push(j.clone());
        }
        a
    }
    pub fn ssh_args(&self) -> Vec<String> {
        let mut a = self.ssh_opts();
        if self.port != DEFAULT_SSH_PORT {
            a.push("-p".into());
            a.push(self.port.to_string());
        }
        if self.agent {
            a.push("-A".into());
        }
        a
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shot: Option<String>, start_screen: Option<String>, open_local: bool, open_host: Option<String>) -> Self {
        install_style(&cc.egui_ctx);
        let _ = std::fs::create_dir_all(data_dir());
        let (mut vault, mut lock) = match load_vault() {
            crate::model::Loaded::Ready(v) => (v, None),
            crate::model::Loaded::Locked(b) => (Vault::default(), Some(crate::lock::LockState::new(b))),
        };
        let demo = shot.is_some() && (lock.is_some() || vault.hosts.is_empty());
        if demo {
            lock = None;
        }
        if demo {
            vault = demo_vault();
        }
        let mut app = App {
            ctx: cc.egui_ctx.clone(),
            vault,
            dirty: false,
            demo,
            screen: start_screen.as_deref().and_then(Screen::from_name).unwrap_or(Screen::Hosts),
            tabs: vec![],
            active: None,
            next_tab: 1,
            editor: Editor::None,
            editor_new: false,
            search: String::new(),
            cur_group: None,
            selected: None,
            forwards_live: HashMap::new(),
            toast: None,
            tz: local_tz_offset(),
            shot: shot.map(|path| ShotState { path, frames: 0, requested: false, image: Arc::new(Mutex::new(None)) }),
            log_view: None,
            confirm: None,
            confirm_import: false,
            settings_tab: 0,
            host_picker: None,
            sftp: Default::default(),
            show_snippets_menu: false,
            gen_key_label: String::new(),
            edit_a: String::new(),
            edit_b: String::new(),
            kh: None,
            collab: Default::default(),
            key_import: None,
            history: crate::autocomplete::History::load(None),
            hist_jobs: vec![],
            hist_tried: Default::default(),
            lock,
            vault_key: None,
            sec: Default::default(),
            sync_rt: Default::default(),
        };
        if app.demo && std::env::var("SSHTUP_DEMO_AC").is_ok() {
            for c in ["git switch main", "git stash list", "git status -sb"] {
                app.history.add(0, c);
            }
            app.history.dirty = false;
        }
        if open_local {
            let spec = app.local_spec();
            app.open_session(spec);
        }
        if let Some(h) = open_host {
            let spec = app.spec_for_quick(&h);
            app.open_session(spec);
        }
        app
    }

    // ------------------------------------------------------------ sessions --

    pub fn local_spec(&self) -> SessionSpec {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        SessionSpec {
            title: "Local".into(),
            program: shell,
            args: vec!["-l".into()],
            term_type: self.vault.settings.terminal_type.clone(),
            scrollback: self.vault.settings.scrollback,
            ..Default::default()
        }
    }

    pub fn ssh_parts(&self, h: &Host) -> SshParts {
        let v = &self.vault;
        let ident = h.identity.and_then(|i| v.identities.iter().find(|x| x.id == i));
        let user = match ident {
            Some(i) if !i.username.is_empty() => i.username.clone(),
            _ => h.username.clone(),
        };
        let password = match ident {
            Some(i) if !i.password.is_empty() => Some(i.password.clone()),
            _ if !h.password.is_empty() => Some(h.password.clone()),
            _ => None,
        };
        let key_id = ident.and_then(|i| i.key).or(h.key);
        let key = key_id.and_then(|k| v.keys.iter().find(|x| x.id == k));
        let jump = v.host(h.jump_host).map(|j| {
            let ju = j.resolved_username(&v.identities);
            let addr = if ju.is_empty() { j.address.clone() } else { format!("{}@{}", ju, j.address) };
            if j.effective_port() == 22 { addr } else { format!("{}:{}", addr, j.effective_port()) }
        });
        SshParts {
            target: if user.is_empty() { h.address.clone() } else { format!("{}@{}", user, h.address) },
            port: h.effective_port(),
            key: key.map(|k| expand_home(&k.path)),
            jump,
            agent: h.agent_forwarding,
            keepalive: h.keep_alive.unwrap_or(v.settings.keep_alive),
            password,
            passphrase: key.and_then(|k| if k.passphrase.is_empty() { None } else { Some(k.passphrase.clone()) }),
        }
    }

    pub fn spec_for_host(&self, h: &Host) -> SessionSpec {
        let s = &self.vault.settings;
        let mut spec = SessionSpec {
            title: h.display_label(),
            host_id: Some(h.id),
            scheme: h.color_scheme.clone(),
            font_size: h.font_size,
            term_type: s.terminal_type.clone(),
            scrollback: s.scrollback,
            refused: self.connection_problem(h),
            ..Default::default()
        };
        match h.protocol {
            Protocol::Telnet => {
                spec.program = "telnet".into();
                spec.args = vec![h.address.clone(), h.effective_port().to_string()];
                spec.password = if h.password.is_empty() { None } else { Some(h.password.clone()) };
            }
            Protocol::Ssh => {
                let p = self.ssh_parts(h);
                spec.program = "ssh".into();
                spec.args = p.ssh_args();
                // `--` ends the options, so a host name can never be read as one
                spec.args.push("--".into());
                spec.args.push(p.target.clone());
                spec.password = p.password;
                spec.passphrase = p.passphrase;
                spec.is_ssh = true;
                spec.startup = self.vault.snippets.iter().find(|x| Some(x.id) == h.startup_snippet).map(|x| x.script.clone());
            }
        }
        spec
    }

    /// `user@host`, `user@host:port`, or a pasted `ssh user@host -p 2222` line.
    pub fn spec_for_quick(&self, text: &str) -> SessionSpec {
        let mut toks: Vec<&str> = text.split_whitespace().collect();
        if toks.first() == Some(&"ssh") {
            toks.remove(0);
        }
        let mut port: Option<u16> = None;
        let mut target = String::new();
        let mut i = 0;
        while i < toks.len() {
            if toks[i] == "-p" && i + 1 < toks.len() {
                port = toks[i + 1].parse().ok();
                i += 2;
                continue;
            }
            if !toks[i].starts_with('-') && target.is_empty() {
                target = toks[i].to_string();
            }
            i += 1;
        }
        let (user, hostport) = match target.split_once('@') {
            Some((u, h)) => (u.to_string(), h.to_string()),
            None => (String::new(), target.clone()),
        };
        let (addr, p2) = match hostport.rsplit_once(':') {
            Some((a, p)) if p.parse::<u16>().is_ok() => (a.trim_matches(|c| c == '[' || c == ']').to_string(), p.parse().ok()),
            _ => (hostport.clone(), None),
        };
        let h = Host { id: 0, address: addr.clone(), username: user, port: port.or(p2), label: addr, ..Default::default() };
        self.spec_for_host(&h)
    }

    pub fn spec_for_forward(&self, r: &PortForward) -> Option<SessionSpec> {
        let h = self.vault.host(r.host)?.clone();
        let p = self.ssh_parts(&h);
        let mut args = p.ssh_args();
        args.extend(["-N".into(), "-o".into(), "ExitOnForwardFailure=yes".into()]);
        let spec_str = match r.kind {
            PfType::Local => ("-L", format!("{}:{}:{}:{}", r.bound(), r.local_port, r.hostname, r.remote_port)),
            PfType::Remote => ("-R", format!("{}:{}:{}:{}", r.bound(), r.local_port, r.hostname, r.remote_port)),
            PfType::Dynamic => ("-D", format!("{}:{}", r.bound(), r.local_port)),
        };
        args.push(spec_str.0.into());
        args.push(spec_str.1);
        args.push("--".into());
        args.push(p.target);
        Some(SessionSpec {
            refused: self.connection_problem(&h),
            title: format!("forward {}", r.id),
            program: "ssh".into(),
            args,
            password: p.password,
            passphrase: p.passphrase,
            is_ssh: true,
            term_type: "xterm-256color".into(),
            scrollback: 200,
            ..Default::default()
        })
    }

    /// Names the first thing about this host that ssh could mistake for an option, if any.
    pub fn connection_problem(&self, h: &Host) -> Option<String> {
        let v = &self.vault;
        let mut checks: Vec<(&str, String)> = vec![("address", h.address.clone()), ("user name", h.resolved_username(&v.identities))];
        if let Some(j) = v.host(h.jump_host) {
            checks.push(("jump host address", j.address.clone()));
            checks.push(("jump host user name", j.resolved_username(&v.identities)));
        }
        checks
            .into_iter()
            .find(|(_, s)| crate::model::unsafe_conn_part(s))
            .map(|(what, _)| format!("The {what} starts with \"-\" or has control characters, so it was not used. Fix it in the host settings."))
    }

    /// The key that encrypts this session's log, when the vault is encrypted.
    pub fn log_key(&self) -> Option<crate::vaultcrypt::LogKey> {
        self.vault_key.as_ref()?;
        self.vault.log_key.as_deref().and_then(crate::vaultcrypt::LogKey::from_hex)
    }

    pub fn open_session(&mut self, mut spec: SessionSpec) {
        if let Some(why) = spec.refused.take() {
            self.notify(why, RED);
            return;
        }
        crate::model::tighten_known_hosts();
        spec.log_key = self.log_key();
        match PtySession::spawn(spec.clone(), self.ctx.clone(), 30, 100) {
            Ok(sess) => {
                let id = self.next_tab;
                self.next_tab += 1;
                self.tabs.push(TermTab { id, title: spec.title.clone(), sess, view: TermViewState::default(), reconnect_at: None, focus_next: true, ready: false, pw_text: String::new(), pw_remember: false });
                self.active = Some(id);
            }
            Err(e) => self.notify(format!("Could not start: {}", e), RED),
        }
    }

    pub fn connect_host(&mut self, id: u64) {
        let now = now_secs();
        if let Some(h) = self.vault.hosts.iter_mut().find(|h| h.id == id) {
            h.last_used = now;
        }
        self.dirty = true;
        if let Some(h) = self.vault.host(Some(id)).cloned() {
            let spec = self.spec_for_host(&h);
            self.open_session(spec);
        }
    }

    pub fn start_forward(&mut self, id: u64) {
        let Some(r) = self.vault.forwards.iter().find(|f| f.id == id).cloned() else { return };
        match self.spec_for_forward(&r) {
            Some(spec) if spec.refused.is_some() => self.notify(spec.refused.unwrap_or_default(), RED),
            Some(mut spec) => match {
                spec.log_key = self.log_key();
                PtySession::spawn(spec, self.ctx.clone(), 10, 80)
            } {
                Ok(s) => {
                    self.forwards_live.insert(id, s);
                    if let Some(f) = self.vault.forwards.iter_mut().find(|f| f.id == id) {
                        f.last_used = now_secs();
                    }
                    self.dirty = true;
                }
                Err(e) => self.notify(format!("Could not start rule: {}", e), RED),
            },
            None => self.notify("Choose a host for this rule first".into(), AMBER),
        }
    }

    pub fn stop_forward(&mut self, id: u64) {
        self.forwards_live.remove(&id);
    }

    pub fn notify(&mut self, msg: String, color: Color32) {
        self.toast = Some((msg, Instant::now(), color));
    }

    pub fn close_tab(&mut self, id: u64) {
        self.tabs.retain(|t| t.id != id);
        if self.active == Some(id) {
            self.active = self.tabs.last().map(|t| t.id);
        }
    }

    pub fn active_scheme(&self) -> &'static Scheme {
        find_scheme(&self.vault.settings.color_scheme)
    }

    pub fn host_label(&self, id: Option<u64>) -> String {
        self.vault.host(id).map(|h| h.display_label()).unwrap_or_else(|| "(no host)".into())
    }

    // ------------------------------------------------------------- deletes --

    pub fn do_delete(&mut self, d: Del) {
        let v = &mut self.vault;
        match d {
            Del::Host(id) => {
                v.hosts.retain(|h| h.id != id);
                for h in v.hosts.iter_mut() {
                    if h.jump_host == Some(id) {
                        h.jump_host = None;
                    }
                }
                v.forwards.retain(|f| f.host != Some(id));
            }
            Del::Group(id) => {
                let parent = v.groups.iter().find(|g| g.id == id).and_then(|g| g.parent);
                for g in v.groups.iter_mut().filter(|g| g.parent == Some(id)) {
                    g.parent = parent;
                }
                for h in v.hosts.iter_mut().filter(|h| h.group == Some(id)) {
                    h.group = parent;
                }
                v.groups.retain(|g| g.id != id);
                if self.cur_group == Some(id) {
                    self.cur_group = parent;
                }
            }
            Del::Key(id) => {
                v.keys.retain(|k| k.id != id);
                for h in v.hosts.iter_mut().filter(|h| h.key == Some(id)) {
                    h.key = None;
                }
                for i in v.identities.iter_mut().filter(|i| i.key == Some(id)) {
                    i.key = None;
                }
            }
            Del::Identity(id) => {
                v.identities.retain(|i| i.id != id);
                for h in v.hosts.iter_mut().filter(|h| h.identity == Some(id)) {
                    h.identity = None;
                }
            }
            Del::Forward(id) => {
                self.forwards_live.remove(&id);
                v.forwards.retain(|f| f.id != id);
            }
            Del::Snippet(id) => {
                v.snippets.retain(|s| s.id != id);
                for h in v.hosts.iter_mut().filter(|h| h.startup_snippet == Some(id)) {
                    h.startup_snippet = None;
                }
            }
            Del::KnownHost(i) => {
                let p = data_dir().join("known_hosts");
                if let Ok(s) = std::fs::read_to_string(&p) {
                    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).collect();
                    let kept: Vec<&str> = lines.iter().enumerate().filter(|(n, _)| *n != i).map(|(_, l)| *l).collect();
                    let _ = std::fs::write(p, kept.join("\n") + "\n");
                }
            }
            Del::Log(p) => {
                let _ = std::fs::remove_file(&p);
                if self.log_view.as_ref().map(|(q, _)| q == &p).unwrap_or(false) {
                    self.log_view = None;
                }
            }
        }
        self.editor = Editor::None;
        self.dirty = true;
    }

    // ------------------------------------------------------------- drawing --

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        let mut to_close = None;
        let mut to_open_local = false;
        let mut collab_act: Option<(u64, crate::collab::CollabAct)> = None;
        egui::Panel::top("tabs")
            .exact_size(42.0)
            .frame(Frame::new().fill(BG_WINDOW).inner_margin(Margin { left: 10, right: 10, top: 10, bottom: 0 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let (c, _, _) = tab_button(ui, self.screen.title(), self.active.is_none(), false, None, Some(self.screen.glyph()));
                    if c {
                        self.active = None;
                    }
                    let ids: Vec<(u64, String, Color32)> = self
                        .tabs
                        .iter()
                        .map(|t| (t.id, t.title.clone(), if t.sess.shared.alive.load(std::sync::atomic::Ordering::Relaxed) { GREEN } else { RED }))
                        .collect();
                    for (id, title, dot) in ids {
                        let people = self.collab.people(id);
                        let (c, x, resp) = tab_button_with(ui, &title, self.active == Some(id), true, Some(dot), None, &people);
                        let act = std::cell::Cell::new(crate::collab::CollabAct::None);
                        let sh = self.collab.shares.iter().find(|s| s.tab_id == id).cloned();
                        resp.context_menu(|ui| act.set(crate::collab::collab_items(ui, sh.as_ref())));
                        if act.get() != crate::collab::CollabAct::None {
                            collab_act = Some((id, act.get()));
                        }
                        if c {
                            self.active = Some(id);
                            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                                t.focus_next = true;
                            }
                        }
                        if x {
                            to_close = Some(id);
                        }
                    }
                    let plus = ui.add(egui::Button::new(RichText::new("+").size(16.0).color(MUTED)).fill(Color32::TRANSPARENT).stroke(egui::Stroke::NONE).min_size(Vec2::new(28.0, 28.0)));
                    if plus.on_hover_text("New session").clicked() {
                        to_open_local = true;
                    }
                });
            });
        if let Some((id, a)) = collab_act {
            self.collab_apply(ui, id, a);
        }
        if let Some(id) = to_close {
            self.close_tab(id);
        }
        if to_open_local {
            self.host_picker = Some(String::new());
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("sidebar")
            .exact_size(212.0)
            .frame(Frame::new().fill(BG_WINDOW).inner_margin(Margin { left: 12, right: 12, top: 14, bottom: 12 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                    badge(ui.painter(), r, "P", icon_color("personal"));
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new("Personal").strong().size(13.5));
                        ui.label(RichText::new("Vault").size(11.0).color(MUTED));
                    });
                });
                ui.add_space(14.0);
                for s in Screen::NAV {
                    self.nav_item(ui, s);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    self.nav_item(ui, Screen::Settings);
                });
            });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, s: Screen) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), egui::Sense::click());
        let selected = self.screen == s;
        if selected {
            ui.painter().rect_filled(rect, 8.0, CARD_HOVER);
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 8.0, CARD);
        }
        let col = if selected { TEXT } else { MUTED };
        if let Some(ic) = crate::icons::Icon::parse(s.glyph()) {
            crate::icons::paint(ui.painter(), Pos2::new(rect.left() + 20.0, rect.center().y), 16.0, ic, col);
        }
        ui.painter().text(Pos2::new(rect.left() + 40.0, rect.center().y), Align2::LEFT_CENTER, s.title(), FontId::proportional(13.5), col);
        if resp.clicked() {
            self.screen = s;
            self.editor = Editor::None;
            self.log_view = None;
            self.kh = None;
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    fn terminal_view(&mut self, ui: &mut egui::Ui, tab_id: u64) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            self.active = None;
            return;
        };
        let s = self.vault.settings.clone();
        let (scheme, fsize) = {
            let t = &self.tabs[idx];
            (find_scheme(t.sess.spec.scheme.as_deref().unwrap_or(&s.color_scheme)), t.sess.spec.font_size.unwrap_or(s.font_size))
        };
        let alive = self.tabs[idx].sess.is_alive();
        let mut reconnect = false;
        let mut close = false;
        let mut run_snippet: Option<String> = None;
        let mut save_pw: Option<(u64, String)> = None;
        let snippets = self.vault.snippets.clone();
        let mut new_cmds: Vec<String> = vec![];
        let ac_host = self.tabs[idx].sess.spec.host_id.unwrap_or(0);
        let (ac_hist, ac_snips): (Vec<String>, Vec<String>) = if s.autocomplete {
            (
                self.history.get(ac_host).iter().take(200).cloned().collect(),
                snippets.iter().map(|x| x.script.trim().to_string()).filter(|x| !x.is_empty() && !x.contains('\n')).collect(),
            )
        } else {
            (vec![], vec![])
        };
        let bell = self.tabs[idx].sess.shared.bell.load(std::sync::atomic::Ordering::Relaxed);

        Frame::new().fill(rgb_c(scheme.background)).show(ui, |ui| {
            // thin toolbar
            Frame::new().fill(BG_WINDOW).inner_margin(Margin::symmetric(12, 6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, status) = if alive { (GREEN, "Connected") } else { (RED, "Disconnected") };
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                    status_dot(ui.painter(), r.center(), dot);
                    ui.label(RichText::new(&self.tabs[idx].title).strong().size(13.0));
                    ui.label(RichText::new(status).size(12.0).color(MUTED));
                    if s.bell_sound && bell > 0 {
                        let (br, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                        crate::icons::paint(ui.painter(), br.center(), 13.0, crate::icons::Icon::Bell, AMBER);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if alive {
                            if flat_button(ui, "Disconnect").clicked() {
                                self.tabs[idx].sess.kill();
                            }
                        } else if primary_button(ui, "Reconnect").clicked() {
                            reconnect = true;
                        }
                        self.collab_menu(ui, tab_id);
                        ui.menu_button("Snippets", |ui| {
                            if snippets.is_empty() {
                                label_muted(ui, "No snippets yet");
                            }
                            for sn in &snippets {
                                if ui.button(sn.display_label()).clicked() {
                                    run_snippet = Some(sn.script.clone());
                                    ui.close();
                                }
                            }
                        });
                    });
                });
            });
            let opts = TermOptions { scheme, font_size: fsize, brighten_bold: s.brighten_bold_text, option_is_meta: s.option_is_meta, right_click_paste: s.right_click_to_paste, ac: if s.autocomplete { Some(crate::autocomplete::AcSource { history: &ac_hist, snippets: &ac_snips }) } else { None } };
            let focus = std::mem::take(&mut self.tabs[idx].focus_next);
            let tab = &mut self.tabs[idx];
            let area = ui.available_rect_before_wrap();
            let ap = tab.sess.auth_prompt().or_else(|| if force_password() && tab.sess.spec.host_id.is_some() { Some(crate::term::AuthPrompt { passphrase: false, wrong: false }) } else { None });
            // is the connection still being made? (a password the person has to type counts as being asked, not connecting)
            if ap.is_some() {
                tab.ready = true;
            } else if !tab.ready {
                let answered_now = tab.sess.shared.answered_at.lock().map(|t| t.is_some()).unwrap_or(false);
                let quiet_ms = if answered_now { 600 } else { 1200 };
                let quiet = tab.sess.shared.last_output.lock().map(|t| t.elapsed() > Duration::from_millis(quiet_ms)).unwrap_or(false);
                let answered = tab.sess.shared.answered_at.lock().map(|t| *t).unwrap_or(None);
                let login_ok = tab.sess.shared.login_seen.lock().map(|t| t.map(|a| a.elapsed() > Duration::from_millis(500)).unwrap_or(false)).unwrap_or(false);
                let got = tab.sess.shared.bytes.load(std::sync::atomic::Ordering::Relaxed) > 0;
                // give up waiting 25 s after the last thing that happened (start, or the password being sent)
                let waited = answered.map(|a| a.elapsed().as_secs()).unwrap_or_else(|| tab.sess.started.elapsed().as_secs());
                let forced = force_connecting();
                let shown = if answered.is_some() { login_ok && quiet } else { got && quiet };
                if (!alive && !forced) || (!forced && (shown || waited > 25)) || (!forced && tab.sess.spec.host_id.is_none()) {
                    tab.ready = true;
                }
            }
            let covered = ap.is_some() || !tab.ready;
            if covered {
                // nothing of the terminal shows (or takes keys) while a cover is on top
                ui.painter().rect_filled(area, 0.0, crate::term::rgb(opts.scheme.background));
                ui.allocate_rect(area, egui::Sense::hover());
            } else {
                terminal_ui(ui, egui::Id::new(("term", tab_id)), &mut tab.sess, &mut tab.view, &opts, focus);
                new_cmds.append(&mut tab.view.ac.new_commands);
            }
            if let Some(ap) = ap {
                // the host wants a password that is not saved (or was refused): ask in a window
                let host = tab.sess.spec.host_id;
                let remember = if host.is_some() && !ap.passphrase { Some(&mut tab.pw_remember) } else { None };
                match password_modal(ui.ctx(), area, &tab.title, ap.passphrase, ap.wrong, &mut tab.pw_text, remember) {
                    PwAnswer::Submit => {
                        let pw = std::mem::take(&mut tab.pw_text);
                        tab.sess.submit_auth(&pw);
                        if ap.passphrase {
                            tab.sess.spec.passphrase = Some(pw);
                        } else {
                            if tab.pw_remember {
                                save_pw = host.map(|h| (h, pw.clone()));
                            }
                            tab.sess.spec.password = Some(pw);
                        }
                        // back to "connecting" until the host has answered
                        tab.ready = false;
                        tab.focus_next = true;
                    }
                    PwAnswer::Cancel => {
                        tab.pw_text.clear();
                        tab.sess.submit_auth("");
                        tab.sess.kill();
                    }
                    PwAnswer::Waiting => {}
                }
            } else if !tab.ready {
                // cover while the host connection is being made
                let secs = tab.sess.started.elapsed().as_secs();
                if connecting_overlay(ui, area, &format!("Connecting to {}", tab.title), &connecting_hint(secs)) {
                    tab.sess.kill();
                    tab.ready = true;
                }
            }
        });

        // first connection to a host: read the host's own shell history for autocomplete
        if s.autocomplete && s.remote_history && !self.demo {
            let t = &self.tabs[idx];
            if t.ready && alive && t.sess.spec.is_ssh && t.sess.auth_prompt().is_none() && ac_host != 0 && !self.history.remote_read(ac_host) && !self.hist_tried.contains(&ac_host) {
                let typed = t.sess.spec.password.clone().or_else(|| t.sess.spec.passphrase.clone());
                self.hist_tried.insert(ac_host);
                let ctx = ui.ctx().clone();
                self.remote_history_start(ac_host, typed, &ctx);
            }
        }
        if !new_cmds.is_empty() && !self.demo {
            for c in new_cmds {
                self.history.add(ac_host, &c);
            }
        }
        if self.history.dirty && !self.demo {
            self.history.save();
        }
        if let Some((h, pw)) = save_pw {
            if let Some(host) = self.vault.hosts.iter_mut().find(|x| x.id == h) {
                host.password = pw;
                self.dirty = true;
            }
        }
        if let Some(sc) = run_snippet {
            self.tabs[idx].sess.send_line(&sc);
            self.tabs[idx].focus_next = true;
        }
        // lost connection: reconnect automatically (once the link had been up a while) if enabled
        if !alive {
            let t = &mut self.tabs[idx];
            let code = t.sess.exit_code();
            let stable = t.sess.started.elapsed() > Duration::from_secs(15);
            if s.autoreconnect && t.sess.spec.is_ssh && stable && code == Some(255) && t.reconnect_at.is_none() {
                t.reconnect_at = Some(Instant::now() + Duration::from_secs(2));
            }
            if let Some(at) = t.reconnect_at {
                if Instant::now() >= at {
                    reconnect = true;
                } else {
                    ui.ctx().request_repaint_after(Duration::from_millis(300));
                }
            }
        }
        if reconnect {
            let spec = self.tabs[idx].sess.spec.clone();
            match PtySession::spawn(spec, self.ctx.clone(), 30, 100) {
                Ok(sess) => {
                    let t = &mut self.tabs[idx];
                    t.sess = sess;
                    t.reconnect_at = None;
                    t.focus_next = true;
                    t.ready = false;
                }
                Err(e) => self.notify(format!("Reconnect failed: {}", e), RED),
            }
        }
        let _ = &mut close;
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        if let Some((msg, t, col)) = self.toast.clone() {
            if t.elapsed() > Duration::from_secs(4) {
                self.toast = None;
                return;
            }
            egui::Area::new(egui::Id::new("toast")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -24.0)).order(egui::Order::Foreground).show(ctx, |ui| {
                Frame::new().fill(CARD_HOVER).stroke(egui::Stroke::new(1.0, col)).corner_radius(8).inner_margin(Margin::symmetric(14, 9)).show(ui, |ui| {
                    ui.label(RichText::new(msg).size(13.0));
                });
            });
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if let Some(d) = self.confirm.clone() {
            let mut decided: Option<bool> = None;
            let m = egui::Modal::new(egui::Id::new("delete_modal")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::symmetric(28, 24))).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Delete").size(15.0).strong().color(TEXT));
                ui.add_space(6.0);
                ui.label("Delete this item? This can't be undone.");
                ui.add_space(18.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if danger_button(ui, "Delete").clicked() {
                        decided = Some(true);
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if m.should_close() {
                decided = Some(false);
            }
            match decided {
                Some(true) => {
                    self.confirm = None;
                    self.do_delete(d);
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }
        if self.host_picker.is_some() {
            let mut pick: Option<Option<u64>> = None; // Some(None) = local terminal
            let hosts: Vec<(u64, String, String)> = self.vault.hosts.iter().map(|h| (h.id, h.display_label(), format!("{}{}", if h.username.is_empty() { String::new() } else { format!("{}@", h.username) }, h.address))).collect();
            let mut query = self.host_picker.clone().unwrap_or_default();
            let m = egui::Modal::new(egui::Id::new("host_picker_modal")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::symmetric(28, 24))).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("New session").size(15.0).strong().color(TEXT));
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search hosts").desired_width(f32::INFINITY).margin(Vec2::new(10.0, 8.0)));
                if !r.has_focus() && query.is_empty() {
                    r.request_focus();
                }
                ui.add_space(8.0);
                let q = query.to_lowercase();
                egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    if q.is_empty() || "local terminal".contains(&q) {
                        if ui.add_sized([ui.available_width(), 34.0], egui::Button::new(RichText::new("Local terminal").size(13.0).color(TEXT)).fill(CARD_HOVER).stroke(egui::Stroke::NONE).corner_radius(6)).clicked() {
                            pick = Some(None);
                        }
                        ui.add_space(4.0);
                    }
                    let mut any = false;
                    for (id, label, sub) in hosts.iter().filter(|(_, l, a)| q.is_empty() || l.to_lowercase().contains(&q) || a.to_lowercase().contains(&q)) {
                        any = true;
                        let text = egui::RichText::new(format!("{label}   {sub}")).size(13.0).color(TEXT);
                        if ui.add_sized([ui.available_width(), 34.0], egui::Button::new(text).fill(CARD_HOVER).stroke(egui::Stroke::NONE).corner_radius(6)).clicked() {
                            pick = Some(Some(*id));
                        }
                        ui.add_space(4.0);
                    }
                    if !any && hosts.is_empty() {
                        ui.label(RichText::new("No saved hosts yet. Add one under Hosts.").size(12.0).color(MUTED));
                    }
                });
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if flat_button(ui, "Cancel").clicked() {
                        pick = Some(None);
                        query = "\u{0}cancel".into();
                    }
                });
            });
            if query == "\u{0}cancel" || m.should_close() {
                self.host_picker = None;
            } else {
                self.host_picker = Some(query);
                if let Some(p) = pick {
                    self.host_picker = None;
                    match p {
                        Some(id) => self.connect_host(id),
                        None => {
                            let spec = self.local_spec();
                            self.open_session(spec);
                        }
                    }
                }
            }
        }
        if self.confirm_import {
            let mut decided: Option<bool> = None;
            let m = egui::Modal::new(egui::Id::new("import_ssh_modal")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::symmetric(28, 24))).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new("Import SSH config").size(15.0).strong().color(TEXT));
                ui.add_space(6.0);
                ui.label("Import hosts from ~/.ssh/config?");
                ui.add_space(4.0);
                ui.label(RichText::new("Hosts that are not saved yet are added (wildcard entries and duplicates are skipped). Key files named by IdentityFile are added to Keys.").size(12.0).color(MUTED));
                ui.add_space(18.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if primary_button(ui, "Import").clicked() {
                        decided = Some(true);
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if m.should_close() {
                decided = Some(false);
            }
            match decided {
                Some(true) => {
                    self.confirm_import = false;
                    self.import_ssh_config();
                }
                Some(false) => self.confirm_import = false,
                None => {}
            }
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(sh) = self.shot.as_mut() else { return };
        ctx.request_repaint();
        sh.frames += 1;
        if sh.frames >= 120 && !sh.requested {
            sh.requested = true;
            let slot = sh.image.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                *slot.lock().unwrap() = Some(img);
            })));
        }
        let got = sh.image.lock().unwrap().take();
        if let Some(img) = got {
            let [w, h] = img.size;
            let raw: Vec<u8> = img.as_raw().to_vec();
            if let Some(parent) = std::path::Path::new(&sh.path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match image::save_buffer(&sh.path, &raw, w as u32, h as u32, image::ColorType::Rgba8) {
                Ok(_) => eprintln!("screenshot saved: {} ({}x{})", sh.path, w, h),
                Err(e) => eprintln!("screenshot failed: {}", e),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // safety net: never hang forever
        if sh.frames > 900 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

pub fn rgb_c(c: u32) -> Color32 {
    crate::term::rgb(c)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.lock_screen(ui) {
            return;
        }
        self.sync_poll(&ctx);
        // background rule sessions: drop dead ones' processes after reporting
        let dead: Vec<u64> = self.forwards_live.iter().filter(|(_, s)| !s.is_alive()).map(|(k, _)| *k).collect();
        for id in dead {
            let code = self.forwards_live.get(&id).and_then(|s| s.exit_code());
            let text = self.forwards_live.get(&id).map(|s| s.parser.lock().map(|p| p.screen().contents()).unwrap_or_default()).unwrap_or_default();
            let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
            self.forwards_live.remove(&id);
            self.notify(format!("Port forwarding stopped (exit {}) {}", code.map(|c| c.to_string()).unwrap_or("?".into()), last), AMBER);
        }
        if !self.forwards_live.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        self.tab_bar(ui);
        match self.active {
            Some(id) => {
                egui::CentralPanel::default().frame(Frame::new().fill(BG_CONTENT)).show(ui, |ui| self.terminal_view(ui, id));
            }
            None => {
                self.sidebar(ui);
                self.editor_panel(ui);
                let browser = self.screen == Screen::Sftp && self.sftp.host.is_some();
                let margin = if browser { Margin::ZERO } else { Margin::symmetric(24, 18) };
                egui::CentralPanel::default().frame(Frame::new().fill(BG_CONTENT).corner_radius(CornerRadius { nw: 12, ..Default::default() }).inner_margin(margin)).show(ui, |ui| {
                    match self.screen {
                        Screen::Hosts => self.screen_hosts(ui),
                        Screen::Keychain => self.screen_keychain(ui),
                        Screen::PortForwarding => self.screen_forwarding(ui),
                        Screen::Snippets => self.screen_snippets(ui),
                        Screen::KnownHosts => self.screen_known_hosts(ui),
                        Screen::Logs => self.screen_logs(ui),
                        Screen::Sftp => self.screen_sftp(ui),
                        Screen::Collaboration => self.screen_collab(ui),
                        Screen::Settings => self.screen_settings(ui),
                    }
                });
            }
        }
        self.remote_history_poll();
        self.toasts(&ctx);
        if self.dirty && !self.demo && self.lock.is_none() && !save_vault(&self.vault, self.vault_key.as_ref()) {
            self.notify("Could not save your changes".into(), RED);
        }
        self.dirty = false;
        self.handle_screenshot(&ctx);
    }
}


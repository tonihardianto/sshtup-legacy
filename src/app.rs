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
    /// The sidebar, in order; Known Hosts starts the "More" part.
    pub const NAV: [Screen; 8] = [Screen::Hosts, Screen::Keychain, Screen::PortForwarding, Screen::Snippets, Screen::Sftp, Screen::KnownHosts, Screen::Logs, Screen::Collaboration];
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

#[derive(Clone)]
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
    /// The session card was opened by a click and stays open until clicked again.
    pub card_pinned: bool,
    /// The connection to the host is up (or has given up waiting); the "connecting" cover is gone.
    pub ready: bool,
    /// What is typed in the password window, and whether to keep it on the host.
    pub pw_text: String,
    pub pw_remember: bool,
    /// Bells already answered (sound or flash), and when the last flash started.
    pub bell_seen: u64,
    pub bell_flash: Option<Instant>,
    /// This tab is the second pane of another tab (it has no tab of its own in the strip).
    pub pane_of: Option<u64>,
    /// A second terminal shown beside (or below) this one.
    pub split: Option<Split>,
}

/// A tab cut in two: the tab's own terminal and a second one.
#[derive(Clone, Copy)]
pub struct Split {
    pub second: u64,
    /// Side by side (true) or one above the other (false).
    pub side_by_side: bool,
    /// Share of the room the first terminal takes.
    pub ratio: f32,
    /// The keyboard goes to the second terminal.
    pub second_focused: bool,
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
    /// Set by Cmd/Ctrl+K: the Hosts search box takes the focus on the next frame.
    pub focus_search: bool,
    pub cur_group: Option<u64>,
    /// the vault the lists show: None is everything, "u" is Personal, "t:<id>" a team
    pub cur_vault: Option<String>,
    pub forwards_live: HashMap<u64, PtySession>,
    pub toast: Option<(String, Instant, Color32)>,
    pub updates: crate::update::Checker,
    pub tz: i64,
    pub shot: Option<ShotState>,
    pub log_view: Option<(PathBuf, String)>,
    pub confirm: Option<Del>,
    pub confirm_import: bool,
    pub settings_tab: u8,
    /// What is typed in the Settings search box.
    pub settings_query: String,
    /// "Clear history…" was pressed once; the next press clears.
    pub clear_armed: bool,
    /// The terminal font the `term` font family was last built with.
    pub term_font: String,
    pub ui_font: String,
    pub host_picker: Option<String>,
    /// The host picker is choosing the second terminal of this tab (and whether it goes beside or below).
    pub split_pick: Option<(u64, bool)>,
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
    pub wel: crate::welcome::WelcomeUi,
    pub kc: crate::keychain::KeychainUi,
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
            // sample hosts, but the settings on disk (so a screenshot can show a chosen setting)
            let settings = vault.settings.clone();
            vault = demo_vault();
            vault.settings = settings;
        }
        crate::fonts::apply(&cc.egui_ctx, &vault.settings.font_family, &vault.settings.ui_font);
        let term_font = vault.settings.font_family.clone();
        let ui_font = vault.settings.ui_font.clone();
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
            focus_search: false,
            forwards_live: HashMap::new(),
            toast: None,
            cur_vault: None,
            updates: Default::default(),
            tz: local_tz_offset(),
            shot: shot.map(|path| ShotState { path, frames: 0, requested: false, image: Arc::new(Mutex::new(None)) }),
            log_view: None,
            confirm: None,
            confirm_import: false,
            settings_tab: 1,
            settings_query: String::new(),
            clear_armed: false,
            term_font,
            ui_font,
            host_picker: None,
            split_pick: None,
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
            wel: Default::default(),
            kc: Default::default(),
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

    /// The people to draw as avatars on a tab: the host's own share, or, in a guest tab, whom the host told us about.
    pub fn people_of(&self, tab_id: u64) -> Vec<(String, Color32)> {
        let mine = self.collab.people(tab_id, self.my_name("you"));
        if !mine.is_empty() {
            return mine;
        }
        let Some(t) = self.tabs.iter().find(|t| t.id == tab_id) else { return vec![] };
        let list = t.sess.shared.people.lock().map(|p| p.clone()).unwrap_or_default();
        list.into_iter().take(4).map(|(n, c)| (n, Color32::from_rgb(c[0], c[1], c[2]))).collect()
    }

    pub fn open_session(&mut self, mut spec: SessionSpec) {
        if let Some(why) = spec.refused.take() {
            self.notify(why, RED());
            return;
        }
        crate::model::tighten_known_hosts();
        spec.log_key = self.log_key();
        match PtySession::spawn(spec.clone(), self.ctx.clone(), 30, 100) {
            Ok(sess) => {
                let id = self.next_tab;
                self.next_tab += 1;
                self.tabs.push(TermTab { id, title: spec.title.clone(), sess, view: TermViewState::default(), reconnect_at: None, focus_next: true, card_pinned: false, ready: false, pw_text: String::new(), pw_remember: false, bell_seen: 0, bell_flash: None, pane_of: None, split: None });
                self.active = Some(id);
            }
            Err(e) => self.notify(format!("Could not start: {}", e), RED()),
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
            Some(spec) if spec.refused.is_some() => self.notify(spec.refused.unwrap_or_default(), RED()),
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
                Err(e) => self.notify(format!("Could not start rule: {}", e), RED()),
            },
            None => self.notify("Choose a host for this rule first".into(), AMBER()),
        }
    }

    pub fn stop_forward(&mut self, id: u64) {
        self.forwards_live.remove(&id);
    }

    pub fn notify(&mut self, msg: String, color: Color32) {
        self.toast = Some((msg, Instant::now(), color));
    }

    /// Opens another terminal of the same session next to (or below) tab `id`.
    /// Opens a new tab with the same session (host or local shell) as tab `id`.
    pub fn duplicate_tab(&mut self, id: u64) {
        let Some(t) = self.tabs.iter().find(|t| t.id == id) else { return };
        let spec = t.sess.spec.clone();
        self.open_session(spec);
    }

    /// Asks which host the second terminal of tab `id` should show.
    pub fn ask_split(&mut self, id: u64, side_by_side: bool) {
        if self.tabs.iter().find(|t| t.id == id).map(|t| t.split.is_some()).unwrap_or(true) {
            self.notify("This tab is already split. Close the extra terminal first".into(), AMBER());
            return;
        }
        self.split_pick = Some((id, side_by_side));
        self.host_picker = Some(String::new());
    }

    /// Opens the terminal described by `spec` next to (or below) tab `id`; with no spec, the tab's own session again.
    pub fn split_tab(&mut self, id: u64, side_by_side: bool, spec: Option<SessionSpec>) {
        let Some(t) = self.tabs.iter().find(|t| t.id == id) else { return };
        if t.split.is_some() {
            return;
        }
        let mut spec = spec.unwrap_or_else(|| t.sess.spec.clone());
        if let Some(why) = spec.refused.take() {
            self.notify(why, RED());
            return;
        }
        crate::model::tighten_known_hosts();
        spec.log_key = self.log_key();
        match PtySession::spawn(spec.clone(), self.ctx.clone(), 30, 100) {
            Ok(sess) => {
                let second = self.next_tab;
                self.next_tab += 1;
                self.tabs.push(TermTab { id: second, title: spec.title.clone(), sess, view: TermViewState::default(), reconnect_at: None, focus_next: true, card_pinned: false, ready: false, pw_text: String::new(), pw_remember: false, bell_seen: 0, bell_flash: None, pane_of: Some(id), split: None });
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                    t.split = Some(Split { second, side_by_side, ratio: 0.5, second_focused: true });
                }
            }
            Err(e) => self.notify(format!("Could not start: {}", e), RED()),
        }
    }

    /// Closes the extra terminal of tab `id`.
    pub fn close_split(&mut self, id: u64) {
        let second = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.split.map(|s| s.second));
        if let Some(sec) = second {
            self.tabs.retain(|t| t.id != sec);
        }
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.split = None;
            t.focus_next = true;
        }
    }

    pub fn close_tab(&mut self, id: u64) {
        if let Some(owner) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.pane_of) {
            self.close_split(owner);
            return;
        }
        let second = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.split.map(|s| s.second));
        self.tabs.retain(|t| t.id != id && Some(t.id) != second);
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
        let guarded = match &d {
            Del::Host(id) => Some(("host", *id)),
            Del::Group(id) => Some(("group", *id)),
            Del::Key(id) => Some(("key", *id)),
            Del::Identity(id) => Some(("identity", *id)),
            Del::Forward(id) => Some(("forward", *id)),
            Del::Snippet(id) => Some(("snippet", *id)),
            _ => None,
        };
        if guarded.map(|(k, id)| self.item_read_only(k, id)).unwrap_or(false) {
            self.notify("You are a viewer in this team and cannot delete its items.".into(), AMBER());
            return;
        }
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
        let mut split_req: Option<(u64, Option<bool>)> = None;
        let mut search_req: Option<u64> = None;
        let dup = std::cell::Cell::new(None::<u64>);
        let mut collab_act: Option<(u64, crate::collab::CollabAct)> = None;
        // macOS draws the window buttons on top of this strip: keep the tabs clear of them
        let (left_pad, strip_h, top_pad) = if cfg!(target_os = "macos") { (84, 48.0, 8) } else { (10, 42.0, 10) };
        egui::Panel::top("tabs")
            .exact_size(strip_h)
            .frame(Frame::new().fill(crate::ui::with_dark(|| BG_WINDOW())).inner_margin(Margin { left: left_pad, right: 10, top: top_pad, bottom: 0 }))
            .show(ui, |ui| crate::ui::with_dark(|| {
                ui.set_style({
                    let mut st = (**ui.style()).clone();
                    st.visuals = crate::ui::visuals();
                    st
                });
                if cfg!(target_os = "macos") {
                    // the empty part of the strip moves the window (double click zooms it); the tabs are drawn on top of this
                    let r = egui::Rect::from_min_max(Pos2::new(ui.max_rect().left() - left_pad as f32, ui.max_rect().top() - top_pad as f32), Pos2::new(ui.max_rect().right() + 10.0, ui.max_rect().bottom()));
                    let drag = ui.interact(r, egui::Id::new("titlebar_drag"), egui::Sense::click_and_drag());
                    if drag.drag_started() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if drag.double_clicked() {
                        let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                }
ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // the buttons on the right stay put; the tabs take what is left and scroll
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(id) = self.active {
                            let split = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.split);
                            if split.is_some() {
                                if split_button(ui, SplitIcon::Close, "Close the extra terminal").clicked() {
                                    split_req = Some((id, None));
                                }
                            } else {
                                if split_button(ui, SplitIcon::Below, "Split down").clicked() {
                                    split_req = Some((id, Some(false)));
                                }
                                if split_button(ui, SplitIcon::Right, "Split right").clicked() {
                                    split_req = Some((id, Some(true)));
                                }
                            }
                            if split_button(ui, SplitIcon::Search, if cfg!(target_os = "macos") { "Find (Cmd+F)" } else { "Find (Ctrl+Shift+F)" }).clicked() {
                                let target = split.filter(|s| s.second_focused).map(|s| s.second).unwrap_or(id);
                                search_req = Some(target);
                            }
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let (c, _, _) = tab_button(ui, self.screen.title(), self.active.is_none(), false, None, Some(self.screen.glyph()));
                    if c {
                        self.active = None;
                    }
                    egui::ScrollArea::horizontal().id_salt("tab_scroll").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let ids: Vec<(u64, String, Color32)> = self
                        .tabs
                        .iter()
                        .filter(|t| t.pane_of.is_none())
                        .map(|t| (t.id, t.title.clone(), if t.sess.shared.alive.load(std::sync::atomic::Ordering::Relaxed) { GREEN() } else { RED() }))
                        .collect();
                    for (id, title, dot) in ids {
                        let people = self.people_of(id);
                        let (c, x, resp) = tab_button_with(ui, &title, self.active == Some(id), true, Some(dot), None, &people);
                        // a tab that has just become the active one scrolls into view
                        if self.active == Some(id) && ui.memory(|m| m.data.get_temp::<Option<u64>>(egui::Id::new("tabs_seen")).flatten()) != Some(id) {
                            resp.scroll_to_me(None);
                        }
                        let act = std::cell::Cell::new(crate::collab::CollabAct::None);
                        let sh = self.collab.shares.iter().find(|s| s.tab_id == id).cloned();
                        let can_share = self.tabs.iter().find(|t| t.id == id).map(|t| !t.sess.spec.is_guest()).unwrap_or(true);
                        resp.context_menu(|ui| {
                            if crate::ui::icon_item(ui, crate::icons::Icon::Copy, "Duplicate", TEXT()).clicked() {
                                dup.set(Some(id));
                                ui.close();
                            }
                            ui.separator();
                            act.set(crate::collab::collab_items(ui, sh.as_ref(), can_share));
                        });
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
                    let plus = ui.add(egui::Button::new(RichText::new("+").size(16.0).color(MUTED())).fill(Color32::TRANSPARENT).stroke(egui::Stroke::NONE).min_size(Vec2::new(28.0, 28.0)));
                    if plus.on_hover_text("New session").clicked() {
                        to_open_local = true;
                    }
                    });
                        });
                    });
                    ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("tabs_seen"), self.active));
                });
            }));
        if let Some((id, a)) = collab_act {
            self.collab_apply(ui, id, a);
        }
        if let Some(id) = to_close {
            self.close_tab(id);
        }
        if let Some(id) = dup.get() {
            self.duplicate_tab(id);
        }
        if let Some(t) = self.tabs.iter_mut().find(|t| Some(t.id) == search_req) {
            t.view.search.get_or_insert_with(Default::default).focus_input = true;
        }
        match split_req {
            Some((id, Some(side))) => self.ask_split(id, side),
            Some((id, None)) => self.close_split(id),
            None => {}
        }
        if to_open_local {
            self.host_picker = Some(String::new());
        }
    }

    /// The tile at the top of the sidebar. With teams it opens a list: all vaults, Personal and each team.
    fn vault_switcher(&mut self, ui: &mut egui::Ui) {
        let teams: Vec<(i64, String, String)> = self.vault.sync.as_ref().map(|c| c.teams.iter().map(|t| (t.id, t.name.clone(), crate::sync::ui::role_name(if t.role.is_empty() && t.owner { "owner" } else { &t.role }).to_string())).collect()).unwrap_or_default();
        let (name, sub) = match self.cur_vault.as_deref() {
            None if teams.is_empty() => ("Personal".to_string(), "Vault".to_string()),
            None => ("All vaults".to_string(), format!("{} teams", teams.len())),
            Some(v) => (self.vault_name(v), if v == "u" { "Only me".to_string() } else { "Team".to_string() }),
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), egui::Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter();
        let fill = if resp.hovered() { CARD_HOVER() } else { Color32::TRANSPARENT };
        p.rect(rect, 12.0, fill, egui::Stroke::new(1.0, BORDER()), egui::StrokeKind::Inside);
        let b = egui::Rect::from_min_size(rect.left_top() + Vec2::new(8.0, 8.0), Vec2::splat(30.0));
        badge(p, b, &name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "P".into()), icon_color(&name.to_lowercase()));
        p.text(egui::pos2(b.right() + 10.0, rect.center().y - 7.0), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(13.5), TEXT());
        p.text(egui::pos2(b.right() + 10.0, rect.center().y + 8.0), egui::Align2::LEFT_CENTER, &sub, egui::FontId::proportional(11.0), MUTED());
        let c = egui::pos2(rect.right() - 16.0, rect.center().y);
        let st = egui::Stroke::new(1.5, MUTED());
        p.line_segment([c + Vec2::new(-4.0, -2.0), c + Vec2::new(0.0, 2.0)], st);
        p.line_segment([c + Vec2::new(0.0, 2.0), c + Vec2::new(4.0, -2.0)], st);
        let mut pick: Option<Option<String>> = None;
        let cur = self.cur_vault.clone();
        egui::Popup::menu(&resp).gap(6.0).width(rect.width()).show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // one row: badge, name, muted detail on the right, check when picked
            let row = |ui: &mut egui::Ui, icon: &str, name: &str, detail: &str, on: bool| -> bool {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), egui::Sense::click());
                let p = ui.painter();
                if on {
                    p.rect_filled(r, 8.0, HOVER());
                } else if resp.hovered() {
                    p.rect_filled(r, 8.0, CARD_HOVER());
                }
                let b = egui::Rect::from_min_size(r.left_top() + Vec2::new(6.0, 6.0), Vec2::splat(22.0));
                badge(p, b, icon, icon_color(&name.to_lowercase()));
                p.text(egui::pos2(b.right() + 9.0, r.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(13.0), TEXT());
                let mut x = r.right() - 10.0;
                if on {
                    let c = egui::pos2(x - 4.0, r.center().y);
                    let st = egui::Stroke::new(1.6, TEXT());
                    p.line_segment([c + Vec2::new(-4.0, 0.0), c + Vec2::new(-1.0, 3.0)], st);
                    p.line_segment([c + Vec2::new(-1.0, 3.0), c + Vec2::new(4.5, -3.5)], st);
                    x -= 18.0;
                }
                if !detail.is_empty() {
                    p.text(egui::pos2(x, r.center().y), egui::Align2::RIGHT_CENTER, detail, egui::FontId::proportional(11.0), MUTED());
                }
                resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
            };
            if !teams.is_empty() && row(ui, "A", "All vaults", "", cur.is_none()) {
                pick = Some(None);
            }
            if row(ui, "P", "Personal", "Only me", cur.as_deref() == Some("u")) {
                pick = Some(Some("u".into()));
            }
            if !teams.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("TEAMS").size(10.0).color(MUTED()));
            }
            for (id, n, role) in &teams {
                let sc = crate::sync::teams::scope_of(*id);
                let ini = n.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "T".into());
                if row(ui, &ini, n, role, cur.as_deref() == Some(sc.as_str())) {
                    pick = Some(Some(sc));
                }
            }
        });
        if let Some(p) = pick {
            self.set_vault(p);
            egui::Popup::close_all(ui.ctx());
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("sidebar")
            .exact_size(212.0)
            .resizable(false)
            .frame(Frame::new().fill(BG_WINDOW()).inner_margin(Margin { left: 12, right: 12, top: 14, bottom: 12 }))
            .show(ui, |ui| {
                self.vault_switcher(ui);
                ui.add_space(14.0);
                for s in Screen::NAV {
                    if s == Screen::KnownHosts {
                        ui.add_space(10.0);
                        ui.label(RichText::new("MORE").size(10.5).color(if crate::ui::is_light() { MUTED() } else { MUTED().linear_multiply(0.8) }));
                    }
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
            ui.painter().rect_filled(rect, 8.0, CARD_HOVER());
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 8.0, CARD());
        }
        // the light sidebar is pale: darker text, drawn a touch heavier, keeps it readable
        let light = crate::ui::is_light();
        let col = if selected { TEXT() } else if light { Color32::from_rgb(0x3A, 0x3F, 0x55) } else { MUTED() };
        if let Some(ic) = crate::icons::Icon::parse(s.glyph()) {
            crate::icons::paint(ui.painter(), Pos2::new(rect.left() + 20.0, rect.center().y), 16.0, ic, col);
        }
        let at = Pos2::new(rect.left() + 40.0, rect.center().y);
        ui.painter().text(at, Align2::LEFT_CENTER, s.title(), FontId::proportional(13.5), col);
        if light {
            // egui has no font weights: a second pass half a pixel over reads as medium weight
            ui.painter().text(at + Vec2::new(0.45, 0.0), Align2::LEFT_CENTER, s.title(), FontId::proportional(13.5), col);
        }
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

    fn terminal_view(&mut self, ui: &mut egui::Ui, tab_id: u64, has_keys: bool) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == tab_id) else {
            self.active = None;
            return;
        };
        // Cmd/Ctrl + and - change the terminal's font size (instead of zooming the whole window), 0 sets it back.
        let zoom = if !has_keys { None } else { ui.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            if i.consume_shortcut(&K::new(M::COMMAND, Key::Num0)) {
                Some(0.0)
            } else if i.consume_shortcut(&K::new(M::COMMAND, Key::Plus)) || i.consume_shortcut(&K::new(M::COMMAND, Key::Equals)) {
                Some(1.0)
            } else if i.consume_shortcut(&K::new(M::COMMAND, Key::Minus)) {
                Some(-1.0)
            } else {
                None
            }
        }) };
        if let Some(step) = zoom {
            let spec_size = &mut self.tabs[idx].sess.spec.font_size;
            // a host with its own size changes for this tab only; otherwise the setting changes (and is saved)
            let size = match spec_size {
                Some(f) => f,
                None => {
                    self.dirty = true;
                    &mut self.vault.settings.font_size
                }
            };
            *size = if step == 0.0 { DEFAULT_FONT_SIZE } else { (size.round() + step).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE) };
        }
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
        let share = self.collab.shares.iter().find(|s| s.tab_id == tab_id).cloned();
        let people = self.people_of(tab_id);
        let can_share = !self.tabs[idx].sess.spec.is_guest();
        let mut card_collab: Option<crate::collab::CollabAct> = None;

        Frame::new().fill(rgb_c(scheme.background)).show(ui, |ui| {
            let opts = TermOptions { scheme, font_size: fsize, line_height: s.line_height, cursor_shape: &s.cursor_shape, cursor_blink: s.cursor_blink, brighten_bold: s.brighten_bold_text, option_is_meta: s.option_is_meta, right_click_paste: s.right_click_to_paste, ac: if s.autocomplete { Some(crate::autocomplete::AcSource { history: &ac_hist, snippets: &ac_snips }) } else { None } };
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
                // the bell: a short system sound, or a quick flash of the terminal
                if bell > tab.bell_seen {
                    tab.bell_seen = bell;
                    match s.bell_mode() {
                        "sound" => ring_bell(),
                        "flash" => tab.bell_flash = Some(Instant::now()),
                        _ => {}
                    }
                }
                if let Some(t) = tab.bell_flash {
                    let k = t.elapsed().as_secs_f32() / 0.2;
                    if k < 1.0 {
                        ui.painter().rect_filled(area, 0.0, Color32::from_white_alpha(((1.0 - k) * 70.0) as u8));
                        ui.ctx().request_repaint();
                    } else {
                        tab.bell_flash = None;
                    }
                }
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
            // the floating card: connection, snippets, sharing (not while a cover is up)
            // (a hidden card still comes back when the connection is lost: it holds the Reconnect button)
            let corner = crate::card::Corner::from_setting(&s.card_position);
            if !covered && (corner.is_some() || !alive) {
                let c = crate::card::CardIn { corner: corner.unwrap_or(crate::card::Corner::TopRight), tab_id, alive, bell: s.bell_mode() != "off" && bell > 0, snippets: &snippets, share: share.as_ref(), people: &people, can_share };
                let a = crate::card::session_card(ui.ctx(), area, &mut tab.card_pinned, &c);
                if a.disconnect {
                    tab.sess.kill();
                }
                reconnect |= a.reconnect;
                if a.run_snippet.is_some() {
                    run_snippet = a.run_snippet;
                }
                card_collab = a.collab;
            }
        });

        if let Some(a) = card_collab {
            self.collab_apply(ui, tab_id, a);
        }
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
                Err(e) => self.notify(format!("Reconnect failed: {}", e), RED()),
            }
        }
        let _ = &mut close;
    }

    /// The tab's terminal, or both terminals when the tab is split.
    fn terminal_area(&mut self, ui: &mut egui::Ui, id: u64) {
        // keys: Cmd+D splits side by side, Cmd+Shift+D one above the other (Ctrl+Shift+D / Ctrl+Shift+E elsewhere,
        // because Ctrl+D is end-of-file in a shell)
        let req = ui.input_mut(|i| {
            use egui::{Key, KeyboardShortcut as K, Modifiers as M};
            if cfg!(target_os = "macos") {
                if i.consume_shortcut(&K::new(M::COMMAND | M::SHIFT, Key::D)) {
                    Some(false)
                } else if i.consume_shortcut(&K::new(M::COMMAND, Key::D)) {
                    Some(true)
                } else {
                    None
                }
            } else if i.consume_shortcut(&K::new(M::COMMAND | M::SHIFT, Key::D)) {
                Some(true)
            } else if i.consume_shortcut(&K::new(M::COMMAND | M::SHIFT, Key::E)) {
                Some(false)
            } else {
                None
            }
        });
        if let Some(side) = req {
            self.ask_split(id, side);
        }
        let Some(split) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.split) else {
            self.terminal_view(ui, id, true);
            return;
        };
        if !self.tabs.iter().any(|t| t.id == split.second) {
            self.close_split(id);
            self.terminal_view(ui, id, true);
            return;
        }
        let full = ui.available_rect_before_wrap();
        let gap = 7.0;
        let (a, b, bar) = if split.side_by_side {
            let x = (full.left() + full.width() * split.ratio).clamp(full.left() + 160.0, full.right() - 160.0);
            (
                egui::Rect::from_min_max(full.min, Pos2::new(x - gap / 2.0, full.bottom())),
                egui::Rect::from_min_max(Pos2::new(x + gap / 2.0, full.top()), full.max),
                egui::Rect::from_min_max(Pos2::new(x - gap / 2.0, full.top()), Pos2::new(x + gap / 2.0, full.bottom())),
            )
        } else {
            let y = (full.top() + full.height() * split.ratio).clamp(full.top() + 80.0, full.bottom() - 80.0);
            (
                egui::Rect::from_min_max(full.min, Pos2::new(full.right(), y - gap / 2.0)),
                egui::Rect::from_min_max(Pos2::new(full.left(), y + gap / 2.0), full.max),
                egui::Rect::from_min_max(Pos2::new(full.left(), y - gap / 2.0), Pos2::new(full.right(), y + gap / 2.0)),
            )
        };
        // a click in a terminal gives it the keys
        let mut second_focused = split.second_focused;
        if ui.input(|i| i.pointer.primary_pressed()) {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                if a.contains(pos) {
                    second_focused = false;
                } else if b.contains(pos) {
                    second_focused = true;
                }
            }
        }
        // the divider: drag to resize
        let dv = ui.interact(bar, egui::Id::new(("split_bar", id)), egui::Sense::drag());
        let mut ratio = split.ratio;
        if dv.dragged() {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                ratio = if split.side_by_side { (pos.x - full.left()) / full.width() } else { (pos.y - full.top()) / full.height() };
                ratio = ratio.clamp(0.1, 0.9);
            }
        }
        if dv.hovered() || dv.dragged() {
            ui.ctx().set_cursor_icon(if split.side_by_side { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        let line = if dv.hovered() || dv.dragged() { BLUE() } else { BORDER() };
        let lr = if split.side_by_side { egui::Rect::from_center_size(bar.center(), Vec2::new(1.0, bar.height())) } else { egui::Rect::from_center_size(bar.center(), Vec2::new(bar.width(), 1.0)) };
        ui.painter().rect_filled(bar, 0.0, BG_WINDOW());
        ui.painter().rect_filled(lr, 0.0, line);
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
            t.split = Some(Split { ratio, second_focused, ..split });
        }
        for (rect, tid, has_keys) in [(a, id, !second_focused), (b, split.second, second_focused)] {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.set_clip_rect(rect);
                self.terminal_view(ui, tid, has_keys);
            });
            // a thin mark on the terminal that has the keys
            if has_keys {
                ui.painter().rect_stroke(rect.shrink(0.5), 0.0, egui::Stroke::new(1.0, BLUE().linear_multiply(0.55)), egui::StrokeKind::Inside);
            }
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        if let Some((msg, t, col)) = self.toast.clone() {
            if t.elapsed() > Duration::from_secs(4) {
                self.toast = None;
                return;
            }
            let mut close = false;
            egui::Area::new(egui::Id::new("toast")).anchor(Align2::RIGHT_BOTTOM, Vec2::new(-24.0, -64.0)).order(egui::Order::Foreground).show(ctx, |ui| {
                Frame::new().fill(CARD_HOVER()).stroke(egui::Stroke::new(1.0, BORDER())).corner_radius(10).inner_margin(Margin { left: 12, right: 8, top: 10, bottom: 10 }).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(10.0, 0.0);
                    ui.horizontal(|ui| {
                        // status mark: a check, a cross or an exclamation mark in a tinted circle
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
                        let p = ui.painter();
                        p.circle_filled(r.center(), 11.0, col.linear_multiply(0.22));
                        let st = egui::Stroke::new(1.8, col);
                        let c = r.center();
                        if col == GREEN() {
                            p.line_segment([c + Vec2::new(-4.5, 0.5), c + Vec2::new(-1.5, 3.5)], st);
                            p.line_segment([c + Vec2::new(-1.5, 3.5), c + Vec2::new(4.5, -3.5)], st);
                        } else if col == RED() {
                            p.line_segment([c + Vec2::new(-3.5, -3.5), c + Vec2::new(3.5, 3.5)], st);
                            p.line_segment([c + Vec2::new(-3.5, 3.5), c + Vec2::new(3.5, -3.5)], st);
                        } else {
                            p.line_segment([c + Vec2::new(0.0, -4.5), c + Vec2::new(0.0, 1.0)], st);
                            p.circle_filled(c + Vec2::new(0.0, 4.2), 1.1, col);
                        }
                        // the message on one line when it fits, wrapped at 380 px when it does not
                        let g = ui.painter().layout(msg.clone(), egui::FontId::proportional(13.0), TEXT(), 380.0);
                        let (tr, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
                        ui.painter().galley(tr.min, g, TEXT());
                        let (xr, xresp) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::click());
                        if xresp.hovered() {
                            ui.painter().rect_filled(xr, 6.0, HOVER());
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        crate::icons::paint(ui.painter(), xr.center(), 12.0, crate::icons::Icon::Close, if xresp.hovered() { TEXT() } else { MUTED() });
                        if xresp.on_hover_text("Dismiss").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if close {
                self.toast = None;
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        self.sync_move_pump(ctx);
        self.group_move_pump(ctx);
        if let Some(d) = self.confirm.clone() {
            let mut decided: Option<bool> = None;
            let m = egui::Modal::new(egui::Id::new("delete_modal")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::symmetric(28, 24))).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Delete").size(15.0).strong().color(TEXT()));
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
                let title = match self.split_pick {
                    Some((_, true)) => "Split right: choose a host",
                    Some((_, false)) => "Split down: choose a host",
                    None => "New session",
                };
                ui.label(RichText::new(title).size(15.0).strong().color(TEXT()));
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search hosts").desired_width(f32::INFINITY).margin(Vec2::new(10.0, 8.0)));
                if !r.has_focus() && query.is_empty() {
                    r.request_focus();
                }
                ui.add_space(8.0);
                let q = query.to_lowercase();
                egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    if q.is_empty() || "local terminal".contains(&q) {
                        if ui.add_sized([ui.available_width(), 34.0], egui::Button::new(RichText::new("Local terminal").size(13.0).color(TEXT())).fill(CARD_HOVER()).stroke(egui::Stroke::NONE).corner_radius(6)).clicked() {
                            pick = Some(None);
                        }
                        ui.add_space(4.0);
                    }
                    let mut any = false;
                    for (id, label, sub) in hosts.iter().filter(|(_, l, a)| q.is_empty() || l.to_lowercase().contains(&q) || a.to_lowercase().contains(&q)) {
                        any = true;
                        let text = egui::RichText::new(format!("{label}   {sub}")).size(13.0).color(TEXT());
                        if ui.add_sized([ui.available_width(), 34.0], egui::Button::new(text).fill(CARD_HOVER()).stroke(egui::Stroke::NONE).corner_radius(6)).clicked() {
                            pick = Some(Some(*id));
                        }
                        ui.add_space(4.0);
                    }
                    if !any && hosts.is_empty() {
                        ui.label(RichText::new("No saved hosts yet. Add one under Hosts.").size(12.0).color(MUTED()));
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
                self.split_pick = None;
            } else {
                self.host_picker = Some(query);
                if let Some(p) = pick {
                    self.host_picker = None;
                    if let Some((tab, side)) = self.split_pick.take() {
                        let spec = match p {
                            Some(id) => self.vault.host(Some(id)).cloned().map(|h| self.spec_for_host(&h)),
                            None => Some(self.local_spec()),
                        };
                        if let Some(spec) = spec {
                            self.split_tab(tab, side, Some(spec));
                        }
                    } else {
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
        }
        if self.confirm_import {
            let mut decided: Option<bool> = None;
            let m = egui::Modal::new(egui::Id::new("import_ssh_modal")).frame(egui::Frame::popup(&ctx.global_style()).inner_margin(Margin::symmetric(28, 24))).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(RichText::new("Import SSH config").size(15.0).strong().color(TEXT()));
                ui.add_space(6.0);
                ui.label("Import hosts from ~/.ssh/config?");
                ui.add_space(4.0);
                ui.label(RichText::new("Hosts that are not saved yet are added (wildcard entries and duplicates are skipped). Key files named by IdentityFile are added to Keys.").size(12.0).color(MUTED()));
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

/// Plays the system's alert sound, at most a few times a second (a program can ring many bells at once).
fn ring_bell() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if let Ok(mut last) = LAST.lock() {
        if last.map(|t| t.elapsed() < Duration::from_millis(250)).unwrap_or(false) {
            return;
        }
        *last = Some(Instant::now());
    }
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("afplay");
        c.arg("/System/Library/Sounds/Tink.aiff");
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-Command", "[System.Media.SystemSounds]::Beep.Play()"]);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // no console window
        }
        c
    } else {
        let mut c = Command::new("canberra-gtk-play");
        c.args(["-i", "bell"]);
        c
    };
    if let Ok(mut child) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

pub fn rgb_c(c: u32) -> Color32 {
    crate::term::rgb(c)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        crate::macos_bar::align(_frame);
        let ctx = ui.ctx().clone();
        if self.lock_screen(ui) {
            return;
        }
        sync_theme(&ctx, &self.vault.settings.app_theme);
        self.sync_poll(&ctx);
        if self.term_font != self.vault.settings.font_family || self.ui_font != self.vault.settings.ui_font {
            self.term_font = self.vault.settings.font_family.clone();
            self.ui_font = self.vault.settings.ui_font.clone();
            if !crate::fonts::apply(&ctx, &self.term_font, &self.ui_font) {
                self.notify(format!("Could not read the font {}; using {}", self.term_font, crate::fonts::BUILT_IN), AMBER());
            }
        }
        // background rule sessions: drop dead ones' processes after reporting
        let dead: Vec<u64> = self.forwards_live.iter().filter(|(_, s)| !s.is_alive()).map(|(k, _)| *k).collect();
        for id in dead {
            let code = self.forwards_live.get(&id).and_then(|s| s.exit_code());
            let text = self.forwards_live.get(&id).map(|s| s.parser.lock().map(|p| p.screen().contents()).unwrap_or_default()).unwrap_or_default();
            let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
            self.forwards_live.remove(&id);
            self.notify(format!("Port forwarding stopped (exit {}) {}", code.map(|c| c.to_string()).unwrap_or("?".into()), last), AMBER());
        }
        if !self.forwards_live.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        self.tab_bar(ui);
        if self.needs_welcome() {
            self.welcome_screen(ui);
            self.remote_history_poll();
            self.toasts(&ctx);
            if self.dirty && !self.demo && self.lock.is_none() && !save_vault(&self.vault, self.vault_key.as_ref()) {
                self.notify("Could not save your changes".into(), RED());
            }
            self.dirty = false;
            self.handle_screenshot(&ctx);
            return;
        }
        match self.active {
            Some(id) => {
                egui::CentralPanel::default().frame(Frame::new().fill(BG_CONTENT())).show(ui, |ui| self.terminal_area(ui, id));
            }
            None => {
                self.sidebar(ui);
                self.editor_panel(ui);
                // The content panel has a rounded top-left corner; fill the notch behind it with the window colour.
                {
                    let r = ui.available_rect_before_wrap();
                    ui.painter().rect_filled(egui::Rect::from_min_size(r.min, Vec2::splat(12.0)), 0.0, BG_WINDOW());
                }
                let browser = self.screen == Screen::Sftp && self.sftp.host.is_some();
                let margin = if browser { Margin::ZERO } else { Margin::symmetric(24, 18) };
                egui::CentralPanel::default().frame(Frame::new().fill(BG_CONTENT()).corner_radius(CornerRadius { nw: 12, ..Default::default() }).inner_margin(margin)).show(ui, |ui| {
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
            self.notify("Could not save your changes".into(), RED());
        }
        self.dirty = false;
        self.handle_screenshot(&ctx);
    }
}


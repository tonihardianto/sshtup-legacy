//! A real terminal: a pseudo-terminal running a program (a shell or `ssh`), a screen
//! emulator that understands escape codes, and an egui widget that draws it.

use crate::model::{data_dir, now_secs};
use crate::schemes::Scheme;
use eframe::egui::{self, Color32, Event, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct SessionSpec {
    pub title: String,
    pub program: String,
    pub args: Vec<String>,
    pub password: Option<String>,
    pub passphrase: Option<String>,
    pub startup: Option<String>,
    pub host_id: Option<u64>,
    pub scheme: Option<String>,
    pub font_size: Option<f32>,
    pub is_ssh: bool,
    pub term_type: String,
    pub scrollback: usize,
    /// Set when the vault is encrypted: the session log is then written encrypted.
    pub log_key: Option<crate::vaultcrypt::LogKey>,
    /// Why this connection must not be started (for example an address that looks like an option).
    pub refused: Option<String>,
}

impl SessionSpec {
    /// A guest's tab of someone else's shared terminal (it runs our own `--relay-join` helper).
    pub fn is_guest(&self) -> bool {
        self.args.first().map(|a| a == "--relay-join").unwrap_or(false)
    }
}

/// The host asked for a password or key passphrase that is not saved (or was wrong).
#[derive(Clone, Copy, Debug)]
pub struct AuthPrompt {
    pub passphrase: bool,
    /// An earlier answer was refused.
    pub wrong: bool,
}

pub struct Shared {
    /// Set while the program waits for a password the person has to type.
    pub prompt: Mutex<Option<AuthPrompt>>,
    /// When the last password was sent.
    pub answered_at: Mutex<Option<Instant>>,
    /// When the host first showed real output after a typed or saved password (a sign the login worked).
    pub login_seen: Mutex<Option<Instant>>,
    pub alive: AtomicBool,
    pub bytes: AtomicUsize,
    pub bell: AtomicU64,
    pub last_output: Mutex<Instant>,
    pub exit_code: Mutex<Option<u32>>,
    /// When set, every chunk the program prints is also sent here (used to share a terminal).
    pub tap: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    /// Who of the guests of a shared terminal typed last, shown under the cursor for a moment.
    pub typing: Mutex<Option<Typing>>,
    /// In a guest tab: who is in the session (the host first), as the host last said.
    pub people: Mutex<Vec<(String, [u8; 3])>>,
}

/// A guest's keystroke seen by the host: shown as a small badge under the cursor.
#[derive(Clone)]
pub struct Typing {
    pub name: String,
    pub color: [u8; 3],
    pub at: Instant,
}

/// How long the badge stays after the last keystroke.
pub const TYPING_SHOWN: Duration = Duration::from_millis(2500);

pub struct PtySession {
    pub spec: SessionSpec,
    pub parser: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    pub shared: Arc<Shared>,
    pub started: Instant,
    pub log_path: Option<std::path::PathBuf>,
    pub size: (u16, u16),
    startup_sent: bool,
}

impl PtySession {
    pub fn spawn(spec: SessionSpec, ctx: egui::Context, rows: u16, cols: u16) -> Result<Self, String> {
        let rows = rows.max(2);
        let cols = cols.max(10);
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(&spec.program);
        for a in &spec.args {
            cmd.arg(a);
        }
        cmd.env("TERM", if spec.term_type.is_empty() { "xterm-256color" } else { &spec.term_type });
        cmd.env("COLORTERM", "truecolor");
        // a guest tab of a shared terminal learns who is typing from its own helper program (see typing_osc)
        let typing_secret = spec.is_guest().then(crate::typing_osc::new_secret);
        if let Some(s) = &typing_secret {
            cmd.env("SSHTUP_TYPING_SECRET", s);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.cwd(home);
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));

        let sb = if spec.scrollback == 0 { 10_000 } else { spec.scrollback };
        let parser = Arc::new(Mutex::new(vt100::Parser::new(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(cols).unwrap(),
            sb,
        )));
        let shared = Arc::new(Shared {
            alive: AtomicBool::new(true),
            bytes: AtomicUsize::new(0),
            bell: AtomicU64::new(0),
            last_output: Mutex::new(Instant::now()),
            exit_code: Mutex::new(None),
            tap: Mutex::new(None),
            typing: Mutex::new(None),
            people: Mutex::new(Vec::new()),
            prompt: Mutex::new(None),
            answered_at: Mutex::new(None),
            login_seen: Mutex::new(None),
        });

        // Session log: everything the program printed, saved for the Logs screen.
        let log_path = {
            let dir = data_dir().join("logs");
            let _ = std::fs::create_dir_all(&dir);
            let safe: String = spec.title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
            let mut p = dir.join(format!("{}__{}.log", now_secs(), safe));
            let mut n = 2;
            // two sessions with the same title in the same second must not share a log file
            while p.exists() {
                p = dir.join(format!("{}__{}-{}.log", now_secs(), safe, n));
                n += 1;
            }
            Some(p)
        };

        {
            let parser = parser.clone();
            let shared = shared.clone();
            let writer = writer.clone();
            let password = spec.password.clone();
            let passphrase = spec.passphrase.clone();
            let log_path = log_path.clone();
            let log_key = spec.log_key.clone();
            let is_ssh = spec.is_ssh;
            let mut typing_scan = typing_secret.map(crate::typing_osc::Scan::new);
            std::thread::spawn(move || {
                let mut buf = [0u8; 16384];
                let mut tail = String::new();
                let mut auto_pw = false;
                let mut auto_pp = false;
                let mut seen = [0u32; 2];
                let mut denied = false;
                let mut authed = false;
                // output that looks like the start of a session (after a quiet spell following an answer); it only
                // counts once more output or a prompt shows up a moment later, because a refusal can arrive in pieces
                let mut login_guess: Option<Instant> = None;
                let begun = Instant::now();
                let mut log = log_path.as_ref().and_then(|p| crate::vaultcrypt::LogSink::create(p, log_key.as_ref()));
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let scanned;
                            let chunk: &[u8] = match typing_scan.as_mut() {
                                Some(scan) => {
                                    let (clean, seen) = scan.feed(&buf[..n]);
                                    for report in seen {
                                        match report {
                                            crate::typing_osc::Report::Typing(p) => {
                                                if let Ok(mut t) = shared.typing.lock() {
                                                    *t = Some(Typing { name: p.name, color: p.color, at: Instant::now() });
                                                }
                                            }
                                            crate::typing_osc::Report::People(v) => {
                                                if let Ok(mut r) = shared.people.lock() {
                                                    *r = v.into_iter().map(|p| (p.name, p.color)).collect();
                                                }
                                            }
                                        }
                                    }
                                    scanned = clean;
                                    &scanned
                                }
                                None => &buf[..n],
                            };
                            if let Ok(mut p) = parser.lock() {
                                p.process(chunk);
                            }
                            if let Some(f) = log.as_mut() {
                                f.write(chunk);
                            }
                            if let Ok(t) = shared.tap.lock() {
                                if let Some(tx) = t.as_ref() {
                                    let _ = tx.send(chunk.to_vec());
                                }
                            }
                            shared.bytes.fetch_add(n, Ordering::Relaxed);
                            let bells = chunk.iter().filter(|b| **b == 7).count() as u64;
                            if bells > 0 {
                                shared.bell.fetch_add(bells, Ordering::Relaxed);
                            }
                            *shared.last_output.lock().unwrap() = Instant::now();
                            // Auto-login: answer the password / passphrase prompt automatically.
                            tail.push_str(&String::from_utf8_lossy(chunk).to_lowercase());
                            if tail.len() > 300 {
                                let cut = tail.len() - 200;
                                let cut = (cut..tail.len()).find(|i| tail.is_char_boundary(*i)).unwrap_or(0);
                                tail = tail[cut..].to_string();
                            }
                            if tail.contains("permission denied") || tail.contains("authentication failed") {
                                denied = true;
                            }
                            let t = tail.trim_end();
                            let kind = if t.ends_with(':') && t.contains("passphrase") {
                                Some(true)
                            } else if t.ends_with(':') && t.contains("password") {
                                Some(false)
                            } else {
                                None
                            };
                            match kind {
                                Some(is_pp) => {
                                    let saved = if is_pp { passphrase.clone() } else { password.clone() };
                                    let done = if is_pp { &mut auto_pp } else { &mut auto_pw };
                                    let pending = shared.prompt.lock().map(|p| p.is_some()).unwrap_or(false);
                                    if !denied && login_guess.map(|g| g.elapsed() > Duration::from_secs(1)).unwrap_or(false) {
                                        authed = true;
                                    }
                                    if let (Some(r), false) = (saved, *done) {
                                        // saved secret: answer once
                                        *done = true;
                                        if let Ok(mut w) = writer.lock() {
                                            let _ = w.write_all(format!("{}\r", r).as_bytes());
                                            let _ = w.flush();
                                        }
                                        *shared.answered_at.lock().unwrap() = Some(Instant::now());
                                        *shared.login_seen.lock().unwrap() = None;
                                        seen[is_pp as usize] += 1;
                                        tail.clear();
                                    } else if is_ssh && !authed && !pending {
                                        // nothing saved, or the answer was refused: the window asks the person
                                        let wrong = seen[is_pp as usize] > 0 || denied;
                                        seen[is_pp as usize] += 1;
                                        denied = false;
                                        *shared.prompt.lock().unwrap() = Some(AuthPrompt { passphrase: is_pp, wrong });
                                        *shared.login_seen.lock().unwrap() = None;
                                        tail.clear();
                                    }
                                }
                                None => {
                                    let pending = shared.prompt.lock().map(|p| p.is_some()).unwrap_or(false);
                                    let answered = *shared.answered_at.lock().unwrap();
                                    if denied {
                                        login_guess = None;
                                        *shared.login_seen.lock().unwrap() = None;
                                    } else if !pending && answered.is_some() && chunk.iter().any(|b| !b.is_ascii_whitespace()) {
                                        let mut ls = shared.login_seen.lock().unwrap();
                                        if ls.is_none() {
                                            *ls = Some(Instant::now());
                                        }
                                    }
                                    // a refusal that arrives late (hosts delay it) must never count as a login
                                    if !pending && !authed && !denied {
                                        let quiet_after_answer = answered.map(|a| a.elapsed() > Duration::from_secs(3)).unwrap_or(false);
                                        if quiet_after_answer && chunk.iter().any(|b| !b.is_ascii_whitespace()) {
                                            match login_guess {
                                                None => login_guess = Some(Instant::now()),
                                                Some(g) if g.elapsed() > Duration::from_secs(1) => authed = true,
                                                Some(_) => {}
                                            }
                                        } else if answered.is_none() && seen == [0, 0] && begun.elapsed() > Duration::from_secs(45) {
                                            authed = true;
                                        }
                                    }
                                }
                            }
                            ctx.request_repaint();
                        }
                    }
                }
                shared.alive.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }

        Ok(Self {
            spec,
            parser,
            writer,
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            shared,
            started: Instant::now(),
            log_path,
            size: (rows, cols),
            startup_sent: false,
        })
    }

    pub fn writer_handle(&self) -> Arc<Mutex<Box<dyn Write + Send>>> {
        self.writer.clone()
    }

    pub fn write(&self, data: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(data);
            let _ = w.flush();
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(2);
        let cols = cols.max(10);
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        if let Ok(m) = self.master.lock() {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        if let Ok(mut p) = self.parser.lock() {
            if let (Some(r), Some(c)) = (NonZeroU16::new(rows), NonZeroU16::new(cols)) {
                p.screen_mut().set_size(r, c);
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        if !self.shared.alive.load(Ordering::SeqCst) {
            self.capture_exit();
            return false;
        }
        true
    }

    fn capture_exit(&self) {
        let mut code = self.shared.exit_code.lock().unwrap();
        if code.is_none() {
            if let Ok(mut c) = self.child.lock() {
                if let Ok(Some(st)) = c.try_wait() {
                    *code = Some(st.exit_code());
                } else if let Ok(st) = c.wait() {
                    *code = Some(st.exit_code());
                }
            }
        }
    }

    pub fn exit_code(&self) -> Option<u32> {
        *self.shared.exit_code.lock().unwrap()
    }

    pub fn kill(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }

    /// Runs the host's startup snippet once the remote side has printed its first prompt.
    pub fn tick(&mut self) {
        if self.startup_sent {
            return;
        }
        if let Some(s) = self.spec.startup.clone() {
            let quiet = self.shared.last_output.lock().unwrap().elapsed() > Duration::from_millis(700);
            if self.shared.bytes.load(Ordering::Relaxed) > 20 && quiet && self.started.elapsed() > Duration::from_secs(1) {
                self.write(format!("{}\r", s).as_bytes());
                self.startup_sent = true;
            }
        } else {
            self.startup_sent = true;
        }
    }

    /// Sends what the person typed into the password window.
    pub fn submit_auth(&self, text: &str) {
        // state first, so output answering the password can't be wiped by the bookkeeping
        *self.shared.answered_at.lock().unwrap() = Some(Instant::now());
        *self.shared.login_seen.lock().unwrap() = None;
        *self.shared.prompt.lock().unwrap() = None;
        self.write(format!("{}\r", text).as_bytes());
    }

    pub fn auth_prompt(&self) -> Option<AuthPrompt> {
        *self.shared.prompt.lock().unwrap()
    }

    pub fn send_line(&self, text: &str) {
        for line in text.lines() {
            self.write(line.as_bytes());
            self.write(b"\r");
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

// ---------------------------------------------------------------- drawing ---

pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

pub fn indexed_color(scheme: &Scheme, n: u8) -> Color32 {
    match n {
        0..=15 => rgb(scheme.palette[n as usize]),
        16..=231 => {
            let i = n - 16;
            let lv = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(lv(i / 36), lv((i / 6) % 6), lv(i % 6))
        }
        _ => {
            let g = 8 + (n - 232) * 10;
            Color32::from_rgb(g, g, g)
        }
    }
}

pub struct TermOptions<'a> {
    pub scheme: &'a Scheme,
    pub font_size: f32,
    /// Row height as a multiple of the font's line height (1.0 = as tight as the font allows).
    pub line_height: f32,
    /// "block", "bar" or "underline".
    pub cursor_shape: &'a str,
    pub cursor_blink: bool,
    pub brighten_bold: bool,
    pub option_is_meta: bool,
    pub right_click_paste: bool,
    /// Autocomplete suggestions (None = off).
    pub ac: Option<crate::autocomplete::AcSource<'a>>,
}

#[derive(Default)]
pub struct TermViewState {
    pub selection: Option<((u16, u16), (u16, u16))>,
    pub dragging: bool,
    pub ac: crate::autocomplete::AcState,
    /// The search bar (None = closed).
    pub search: Option<SearchState>,
}

/// Find-in-terminal: the text looked for and where it was found, over the screen and the lines scrolled out of it.
#[derive(Default)]
pub struct SearchState {
    pub query: String,
    /// (line counted from the oldest line kept, first column, last column), in reading order.
    pub matches: Vec<(usize, u16, u16)>,
    pub cur: usize,
    /// The text changed: look again and go to the best match.
    pub new_query: bool,
    /// Go to the current match (scroll to it).
    pub jump: bool,
    pub focus_input: bool,
    last_bytes: usize,
    last_scan: Option<Instant>,
}

fn row_chars(screen: &vt100::Screen, r: u16, cols: u16, fold: bool) -> Vec<(u16, char)> {
    let mut v = Vec::with_capacity(cols as usize);
    for c in 0..cols {
        if let Some(cell) = screen.cell(r, c) {
            if cell.is_wide_continuation() {
                continue;
            }
            let ch = if cell.has_contents() { cell.contents().chars().next().unwrap_or(' ') } else { ' ' };
            v.push((c, if fold { ch.to_lowercase().next().unwrap_or(ch) } else { ch }));
        }
    }
    v
}

/// The most lines the view can be scrolled back (leaves the view where it was).
fn max_back(sess: &PtySession) -> usize {
    let Ok(mut p) = sess.parser.lock() else { return 0 };
    let cur = p.screen().scrollback();
    p.screen_mut().set_scrollback(usize::MAX);
    let max = p.screen().scrollback();
    p.screen_mut().set_scrollback(cur);
    max
}

/// Looks for `query` in every line (smart case: capitals in the query make it case sensitive).
fn scan(sess: &PtySession, query: &str) -> Vec<(usize, u16, u16)> {
    let fold = query == query.to_lowercase();
    let q: Vec<char> = if fold { query.to_lowercase().chars().collect() } else { query.chars().collect() };
    if q.is_empty() {
        return vec![];
    }
    let Ok(mut p) = sess.parser.lock() else { return vec![] };
    let keep = p.screen().scrollback();
    p.screen_mut().set_scrollback(usize::MAX);
    let max = p.screen().scrollback();
    let (rows, cols) = {
        let (r, c) = p.screen().size();
        (r.get() as usize, c.get())
    };
    let mut lines: Vec<Option<Vec<(u16, char)>>> = vec![None; max + rows];
    let mut off = 0usize;
    loop {
        p.screen_mut().set_scrollback(off);
        let top = max - off.min(max);
        for r in 0..rows {
            let abs = top + r;
            if abs < lines.len() && lines[abs].is_none() {
                lines[abs] = Some(row_chars(p.screen(), r as u16, cols, fold));
            }
        }
        if off >= max {
            break;
        }
        off = (off + rows).min(max);
    }
    p.screen_mut().set_scrollback(keep);
    let mut out = vec![];
    for (abs, line) in lines.iter().enumerate() {
        let Some(line) = line else { continue };
        let mut i = 0;
        while i + q.len() <= line.len() {
            if line[i..i + q.len()].iter().map(|x| x.1).eq(q.iter().copied()) {
                out.push((abs, line[i].0, line[i + q.len() - 1].0));
                i += q.len();
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Scrolls the view so line `abs` sits in the middle.
fn show_line(sess: &PtySession, abs: usize) {
    let Ok(mut p) = sess.parser.lock() else { return };
    p.screen_mut().set_scrollback(usize::MAX);
    let max = p.screen().scrollback();
    let rows = p.screen().size().0.get() as usize;
    let top = abs.saturating_sub(rows / 2);
    p.screen_mut().set_scrollback(max.saturating_sub(top));
}

/// The search bar. Returns true when it should close.
fn search_bar(ctx: &egui::Context, id: egui::Id, rect: Rect, s: &mut SearchState) -> bool {
    use crate::ui::{BORDER, CARD_HOVER, MUTED, RED, TEXT};
    let mut close = false;
    let w = (rect.width() - 24.0).clamp(200.0, 400.0);
    egui::Area::new(id.with("bar")).order(egui::Order::Foreground).fixed_pos(Pos2::new(rect.center().x - w / 2.0, rect.top() + 8.0)).show(ctx, |ui| {
        egui::Frame::new().fill(CARD_HOVER()).stroke(Stroke::new(1.0, BORDER())).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_width(w - 16.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let status = if s.query.is_empty() {
                    String::new()
                } else if s.matches.is_empty() {
                    "No results".to_string()
                } else {
                    format!("{} of {}", s.cur + 1, s.matches.len())
                };
                let text_w = (w - 16.0 - 3.0 * 28.0 - 70.0 - 16.0).max(60.0);
                let r = ui.add(egui::TextEdit::singleline(&mut s.query).id(id.with("input")).hint_text("Find").desired_width(text_w).margin(Vec2::new(6.0, 4.0)));
                if s.focus_input {
                    r.request_focus();
                    s.focus_input = false;
                }
                if r.changed() {
                    s.new_query = true;
                }
                let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
                let mut dir = 0i32; // -1 = older match, 1 = newer
                if r.lost_focus() && enter {
                    dir = if shift { 1 } else { -1 };
                    r.request_focus();
                }
                if r.has_focus() && esc {
                    close = true;
                }
                let (txt, col) = if status == "No results" { (status.as_str(), RED()) } else { (status.as_str(), MUTED()) };
                ui.add_sized([70.0, 20.0], egui::Label::new(egui::RichText::new(txt).size(11.5).color(col)));
                for (ic, tip, d) in [(crate::icons::Icon::ChevUp, "Previous (Shift+Enter)", 1), (crate::icons::Icon::ChevDown, "Next (Enter)", -1)] {
                    let (b, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(b, 5.0, crate::ui::CARD());
                    }
                    crate::icons::paint(ui.painter(), b.center(), 14.0, ic, if resp.hovered() { TEXT() } else { MUTED() });
                    if resp.on_hover_text(tip).clicked() {
                        // ▲ goes to the older match (up the screen), ▼ to the newer
                        dir = if d == 1 { -1 } else { 1 };
                    }
                }
                let (b, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(b, 5.0, crate::ui::CARD());
                }
                crate::icons::paint(ui.painter(), b.center(), 12.0, crate::icons::Icon::Close, if resp.hovered() { TEXT() } else { MUTED() });
                if resp.on_hover_text("Close (Esc)").clicked() {
                    close = true;
                }
                if dir != 0 && !s.matches.is_empty() {
                    let n = s.matches.len() as i32;
                    s.cur = ((s.cur as i32 + dir).rem_euclid(n)) as usize;
                    s.jump = true;
                }
            });
        });
    });
    close
}

/// The size of one terminal cell, and how far down in it the glyphs start (rows taller than the font centre their text).
pub fn cell_size(ui: &egui::Ui, font: &FontId, line_height: f32) -> (Vec2, f32) {
    ui.fonts_mut(|f| {
        let g = f.layout_no_wrap("M".into(), font.clone(), Color32::WHITE);
        let tight = f.row_height(font).ceil() + 1.0;
        let h = (f.row_height(font) * line_height.clamp(1.0, 2.0)).ceil() + 1.0;
        (Vec2::new(g.size().x, h), ((h - tight) / 2.0).floor())
    })
}

/// The cursor drawn in `r` (one cell). A block shows the character under it in `bg`.
pub fn paint_cursor(painter: &egui::Painter, r: Rect, shape: &str, color: Color32) {
    let r = match shape {
        "bar" => Rect::from_min_size(r.min, Vec2::new(2.0, r.height())),
        "underline" => Rect::from_min_max(Pos2::new(r.left(), r.bottom() - 2.0), r.max),
        _ => r,
    };
    painter.rect_filled(r, 0.0, color);
}

/// Draws the terminal and forwards keyboard / mouse input. Returns true when the pointer clicked in it.
pub fn terminal_ui(ui: &mut egui::Ui, id: egui::Id, sess: &mut PtySession, st: &mut TermViewState, o: &TermOptions, want_focus: bool) -> bool {
    let font = crate::fonts::term(o.font_size);
    let (cs, ty) = cell_size(ui, &font, o.line_height);
    let avail = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(avail, Sense::click_and_drag());
    let pad = 10.0;
    let inner = rect.shrink2(Vec2::new(pad, 6.0));
    let cols = (inner.width() / cs.x).floor().max(10.0) as u16;
    let rows = (inner.height() / cs.y).floor().max(2.0) as u16;
    sess.resize(rows, cols);
    sess.tick();

    let painter = ui.painter_at(rect);
    let bg = rgb(o.scheme.background);
    painter.rect_filled(rect, 0.0, bg);

    let focus_id = id;
    if want_focus {
        ui.memory_mut(|m| m.request_focus(focus_id));
    }
    // make the terminal a focusable widget so Tab / arrows reach the program
    let resp = ui.interact(rect, focus_id, Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() {
        ui.memory_mut(|m| m.request_focus(focus_id));
    }
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            focus_id,
            egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
        )
    });
    let focused = ui.memory(|m| m.has_focus(focus_id));

    let fg_default = rgb(o.scheme.foreground);
    let origin = inner.min;
    let origin_of = |r: u16, c: u16| Pos2::new(origin.x + c as f32 * cs.x, origin.y + r as f32 * cs.y);

    // ----- autocomplete: what the terminal shows on the cursor row
    let ac_on = o.ac.is_some();
    if ac_on && std::env::var("SSHTUP_DEMO_AC").is_ok() && !st.ac.demo_done && sess.started.elapsed() > Duration::from_millis(900) {
        st.ac.demo_done = true;
        sess.write(b"git s");
        st.ac.typed_text("git s");
    }
    let (ac_alt, ac_scroll, ac_row) = match sess.parser.lock() {
        Ok(p) => {
            let sc = p.screen();
            let (cr, cc) = sc.cursor_position();
            let mut row = String::new();
            for c in 0..cc {
                match sc.cell(cr, c) {
                    Some(cell) if cell.is_wide_continuation() => {}
                    Some(cell) if cell.has_contents() => row.push_str(&cell.contents()),
                    _ => row.push(' '),
                }
            }
            (sc.alternate_screen(), sc.scrollback(), row)
        }
        Err(_) => (false, 0, String::new()),
    };
    let ac_blocked = ac_alt || ac_scroll > 0 || sess.auth_prompt().is_some() || crate::autocomplete::password_like(&ac_row);
    let ac_echo = crate::autocomplete::echoes(&ac_row, &st.ac.line);

    // ----- mouse: wheel scroll, selection, paste
    let hovered = resp.hovered();
    if hovered {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            let lines = (scroll / cs.y).round() as i32;
            if let Ok(mut p) = sess.parser.lock() {
                if p.screen().alternate_screen() {
                    let app = p.screen().application_cursor();
                    drop(p);
                    let key = match (lines > 0, app) {
                        (true, true) => "\x1bOA",
                        (true, false) => "\x1b[A",
                        (false, true) => "\x1bOB",
                        (false, false) => "\x1b[B",
                    };
                    for _ in 0..lines.unsigned_abs().min(10) {
                        sess.write(key.as_bytes());
                    }
                } else {
                    let cur = p.screen().scrollback() as i32;
                    let next = (cur + lines).max(0) as usize;
                    p.screen_mut().set_scrollback(next);
                }
            }
        }
    }
    let to_cell = |pos: Pos2| -> (u16, u16) {
        let c = ((pos.x - origin.x) / cs.x).floor().clamp(0.0, (cols - 1) as f32) as u16;
        let r = ((pos.y - origin.y) / cs.y).floor().clamp(0.0, (rows - 1) as f32) as u16;
        (r, c)
    };
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let c = to_cell(p);
            st.selection = Some((c, c));
            st.dragging = true;
        }
    } else if resp.dragged_by(egui::PointerButton::Primary) {
        if let (Some(p), Some((a, _))) = (resp.interact_pointer_pos(), st.selection) {
            st.selection = Some((a, to_cell(p)));
        }
    }
    if resp.clicked_by(egui::PointerButton::Primary) {
        let hit = resp.interact_pointer_pos().and_then(|p| st.ac.rects.iter().position(|r| r.contains(p)));
        if let Some(i) = hit {
            if let Some(rest) = st.ac.accept(i) {
                sess.write(rest.as_bytes());
            }
        } else {
            st.selection = None;
        }
    }
    if resp.clicked_by(egui::PointerButton::Secondary) && o.right_click_paste {
        if let Some(t) = ui.ctx().input(|i| i.events.iter().find_map(|e| if let Event::Paste(t) = e { Some(t.clone()) } else { None })) {
            paste(sess, &t);
        }
    }

    // ----- find: Cmd+F (Ctrl+Shift+F elsewhere, since Ctrl+F belongs to the shell)
    if focused {
        let m = if cfg!(target_os = "macos") { egui::Modifiers::COMMAND } else { egui::Modifiers::COMMAND | egui::Modifiers::SHIFT };
        if ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(m, Key::F))) {
            st.search.get_or_insert_with(Default::default).focus_input = true;
        }
    }

    // ----- keyboard
    if focused {
        let events = ui.input(|i| i.events.clone());
        let mods_now = ui.input(|i| i.modifiers);
        let app_cursor = sess.parser.lock().map(|p| p.screen().application_cursor()).unwrap_or(false);
        let bracketed = sess.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
        let mut typed = false;
        for ev in events {
            // the suggestion list takes Tab / Right (accept), Up / Down (choose) and Esc (close)
            if ac_on && !st.ac.shown.is_empty() && !st.ac.lost && !st.ac.dismissed {
                if let Event::Key { key, pressed: true, modifiers, .. } = &ev {
                    if modifiers.is_none() {
                        let n = st.ac.shown.len();
                        let mut took = true;
                        match key {
                            Key::Tab | Key::ArrowRight => {
                                let i = st.ac.sel;
                                if let Some(rest) = st.ac.accept(i) {
                                    sess.write(rest.as_bytes());
                                    typed = true;
                                }
                            }
                            Key::ArrowDown => st.ac.sel = (st.ac.sel + 1) % n,
                            Key::ArrowUp => st.ac.sel = (st.ac.sel + n - 1) % n,
                            Key::Escape => st.ac.dismissed = true,
                            _ => took = false,
                        }
                        if took {
                            continue;
                        }
                    }
                }
            }
            match ev {
                Event::Text(t) => {
                    if o.option_is_meta && mods_now.alt {
                        continue;
                    }
                    if !t.chars().all(|c| c.is_control()) {
                        sess.write(t.as_bytes());
                        st.ac.typed_text(&t);
                        typed = true;
                    }
                }
                Event::Paste(t) => {
                    st.ac.pasted(&t);
                    if bracketed {
                        sess.write(b"\x1b[200~");
                        // normalise line endings and strip any embedded bracket markers so the
                        // pasted text cannot close the bracket early
                        let clean = t.replace("\x1b[201~", "").replace("\x1b[200~", "").replace("\r\n", "\r").replace('\n', "\r");
                        sess.write(clean.as_bytes());
                        sess.write(b"\x1b[201~");
                    } else {
                        paste(sess, &t);
                    }
                    typed = true;
                }
                Event::Copy | Event::Cut => {
                    if let Some(text) = selection_text(sess, st) {
                        ui.ctx().copy_text(text);
                    }
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if let Some(bytes) = key_bytes(key, modifiers, app_cursor, o.option_is_meta) {
                        sess.write(&bytes);
                        st.ac.key(key, modifiers, ac_echo && !ac_blocked);
                        typed = true;
                    }
                }
                _ => {}
            }
        }
        if typed {
            st.selection = None;
            if let Ok(mut p) = sess.parser.lock() {
                p.screen_mut().set_scrollback(0);
            }
        }
    }

    st.ac.refresh(o.ac.as_ref(), ac_blocked, ac_echo);
    if st.ac.typed_at.map(|t| t.elapsed() < Duration::from_millis(900)).unwrap_or(false) {
        ui.ctx().request_repaint_after(Duration::from_millis(120));
    }

    // ----- find: the bar, then the matches
    if st.search.is_some() {
        let close = search_bar(ui.ctx(), focus_id, rect, st.search.as_mut().unwrap());
        if close {
            st.search = None;
            ui.memory_mut(|m| m.request_focus(focus_id));
        }
    }
    let mut max_now = 0usize;
    if let Some(sr) = st.search.as_mut() {
        let bytes = sess.shared.bytes.load(Ordering::Relaxed);
        let stale = sr.last_bytes != bytes && sr.last_scan.map(|t| t.elapsed() > Duration::from_millis(300)).unwrap_or(true);
        if sr.new_query || stale {
            let keep = sr.matches.get(sr.cur).copied();
            sr.matches = scan(sess, &sr.query);
            sr.last_bytes = bytes;
            sr.last_scan = Some(Instant::now());
            if sr.new_query {
                // start with the match nearest the end of the output
                sr.cur = sr.matches.len().saturating_sub(1);
                sr.jump = !sr.matches.is_empty();
                sr.new_query = false;
            } else if let Some(k) = keep {
                sr.cur = sr.matches.iter().position(|m| *m == k).unwrap_or_else(|| sr.cur.min(sr.matches.len().saturating_sub(1)));
            }
        }
        if sr.last_bytes != bytes {
            ui.ctx().request_repaint_after(Duration::from_millis(320));
        }
        if sr.jump {
            sr.jump = false;
            if let Some(m) = sr.matches.get(sr.cur) {
                show_line(sess, m.0);
            }
        }
        if !sr.matches.is_empty() {
            max_now = max_back(sess);
        }
    }
    let hit_color = Color32::from_rgba_unmultiplied(255, 200, 40, 90);
    let cur_hit_color = Color32::from_rgba_unmultiplied(255, 140, 20, 190);

    // ----- paint cells
    let parser = sess.parser.lock().unwrap();
    let screen = parser.screen();
    let (srows, scols) = screen.size();
    let (srows, scols) = (srows.get(), scols.get());
    let sel = st.selection.map(|(a, b)| if (a.0, a.1) <= (b.0, b.1) { (a, b) } else { (b, a) });
    let sel_color = {
        let s = rgb(o.scheme.selection);
        Color32::from_rgba_unmultiplied(s.r(), s.g(), s.b(), 110)
    };
    for r in 0..rows.min(srows) {
        // collect runs of identical style
        struct Run {
            col: u16,
            text: String,
            fg: Color32,
            bold: bool,
            italic: bool,
            underline: bool,
        }
        let mut runs: Vec<Run> = vec![];
        let mut bg_runs: Vec<(u16, u16, Color32)> = vec![];
        let mut c = 0u16;
        while c < cols.min(scols) {
            let cell = match screen.cell(r, c) {
                Some(x) => x,
                None => break,
            };
            if cell.is_wide_continuation() {
                c += 1;
                continue;
            }
            let mut fg = match cell.fgcolor() {
                vt100::Color::Default => fg_default,
                vt100::Color::Idx(n) => {
                    let n = if cell.bold() && o.brighten_bold && n < 8 { n + 8 } else { n };
                    indexed_color(o.scheme, n)
                }
                vt100::Color::Rgb(rr, gg, bb) => Color32::from_rgb(rr, gg, bb),
            };
            let mut cbg = match cell.bgcolor() {
                vt100::Color::Default => None,
                vt100::Color::Idx(n) => Some(indexed_color(o.scheme, n)),
                vt100::Color::Rgb(rr, gg, bb) => Some(Color32::from_rgb(rr, gg, bb)),
            };
            if cell.inverse() {
                let b = cbg.unwrap_or(bg);
                cbg = Some(fg);
                fg = b;
            }
            if cell.dim() {
                fg = blend(fg, bg, 0.5);
            }
            let width = if cell.is_wide() { 2 } else { 1 };
            if let Some(b) = cbg {
                match bg_runs.last_mut() {
                    Some((_, end, col)) if *end == c && *col == b => *end = c + width,
                    _ => bg_runs.push((c, c + width, b)),
                }
            }
            let ch = if cell.has_contents() { cell.contents() } else { " " };
            let can_extend = runs.last().map(|ru| {
                ru.fg == fg && ru.bold == cell.bold() && ru.italic == cell.italic() && ru.underline == cell.underline() && ru.col as usize + ru.text.chars().map(|x| if x.len_utf8() > 2 { 2 } else { 1 }).sum::<usize>() == c as usize
            });
            if can_extend == Some(true) && width == 1 && ch.chars().all(|x| x.len_utf8() <= 2) {
                runs.last_mut().unwrap().text.push_str(ch);
            } else {
                runs.push(Run { col: c, text: ch.to_string(), fg, bold: cell.bold(), italic: cell.italic(), underline: cell.underline() });
            }
            c += width;
        }
        for (a, b, col) in bg_runs {
            let p = origin_of(r, a);
            painter.rect_filled(Rect::from_min_size(p, Vec2::new((b - a) as f32 * cs.x, cs.y)), 0.0, col);
        }
        for ru in runs {
            if ru.text.trim().is_empty() && !ru.underline {
                continue;
            }
            let p = origin_of(r, ru.col) + Vec2::new(0.0, ty);
            let mut text = egui::RichText::new(&ru.text).font(font.clone()).color(ru.fg);
            if ru.bold {
                text = text.strong();
            }
            if ru.italic {
                text = text.italics();
            }
            // draw one glyph run at its exact column; bold is faked by a 1px double strike
            let galley = painter.layout_no_wrap(ru.text.clone(), font.clone(), ru.fg);
            painter.galley(p, galley.clone(), ru.fg);
            if ru.bold {
                painter.galley(p + Vec2::new(0.6, 0.0), galley, ru.fg);
            }
            if ru.underline {
                let y = p.y + cs.y - 2.0 - 2.0 * ty;
                painter.line_segment([Pos2::new(p.x, y), Pos2::new(p.x + ru.text.chars().count() as f32 * cs.x, y)], Stroke::new(1.0, ru.fg));
            }
            let _ = text;
        }
        // search matches
        if let Some(sr) = &st.search {
            let abs = max_now.saturating_sub(screen.scrollback()) + r as usize;
            let first = sr.matches.partition_point(|m| m.0 < abs);
            for (i, m) in sr.matches.iter().enumerate().skip(first).take_while(|(_, m)| m.0 == abs) {
                let p = origin_of(r, m.1.min(cols - 1));
                painter.rect_filled(Rect::from_min_size(p, Vec2::new((m.2.saturating_sub(m.1) + 1) as f32 * cs.x, cs.y)), 2.0, if i == sr.cur { cur_hit_color } else { hit_color });
            }
        }
        // selection highlight
        if let Some((a, b)) = sel {
            if r >= a.0 && r <= b.0 {
                let c0 = if r == a.0 { a.1 } else { 0 };
                let c1 = if r == b.0 { b.1 } else { cols - 1 };
                if c1 >= c0 {
                    let p = origin_of(r, c0);
                    painter.rect_filled(Rect::from_min_size(p, Vec2::new((c1 - c0 + 1) as f32 * cs.x, cs.y)), 0.0, sel_color);
                }
            }
        }
    }
    // cursor
    if !screen.hide_cursor() && screen.scrollback() == 0 {
        let (cr, cc) = screen.cursor_position();
        if cr < rows && cc < cols {
            let p = origin_of(cr, cc);
            let r = Rect::from_min_size(p, Vec2::new(cs.x, cs.y));
            let cur = rgb(o.scheme.cursor);
            if focused {
                let on = !o.cursor_blink || (ui.input(|i| i.time) * 1.8) as i64 % 2 == 0;
                if on {
                    paint_cursor(&painter, r, o.cursor_shape, cur);
                    if o.cursor_shape == "block" {
                        if let Some(cell) = screen.cell(cr, cc) {
                            if cell.has_contents() {
                                painter.text(p + Vec2::new(0.0, ty), egui::Align2::LEFT_TOP, cell.contents(), font.clone(), bg);
                            }
                        }
                    }
                }
                if o.cursor_blink {
                    ui.ctx().request_repaint_after(Duration::from_millis(280));
                }
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, cur), StrokeKind::Inside);
            }
        }
    }
    // a guest typing: their badge under the cursor, with a small arrow pointing at it
    if let Some(t) = sess.shared.typing.lock().ok().and_then(|g| g.clone()) {
        let age = t.at.elapsed();
        if age < TYPING_SHOWN && !screen.hide_cursor() && screen.scrollback() == 0 {
            let (cr, cc) = screen.cursor_position();
            if cr < rows && cc < cols {
                let p = origin_of(cr, cc);
                let c = Color32::from_rgb(t.color[0], t.color[1], t.color[2]);
                let tip = Pos2::new(p.x + cs.x * 0.5, p.y + cs.y + 2.0);
                let size = 22.0;
                let mut b = Rect::from_min_size(Pos2::new(tip.x - size / 2.0, tip.y + 6.0), Vec2::splat(size));
                if b.right() > rect.right() {
                    b = b.translate(Vec2::new(rect.right() - b.right(), 0.0));
                }
                painter.add(egui::Shape::convex_polygon(vec![tip, Pos2::new(tip.x - 5.0, tip.y + 7.0), Pos2::new(tip.x + 5.0, tip.y + 7.0)], c, Stroke::NONE));
                crate::ui::avatar(&painter, b.center(), size / 2.0, &t.name, c);
                ui.ctx().request_repaint_after(TYPING_SHOWN.saturating_sub(age) + Duration::from_millis(20));
            }
        }
    }
    // scrollback indicator
    if screen.scrollback() > 0 {
        let txt = format!("↑ {} lines back", screen.scrollback());
        let pos = Pos2::new(rect.right() - 12.0, rect.top() + if st.search.is_some() { 52.0 } else { 8.0 });
        let g = painter.layout_no_wrap(txt, FontId::proportional(12.0), Color32::WHITE);
        let rr = Rect::from_min_size(Pos2::new(pos.x - g.size().x - 10.0, pos.y), g.size() + Vec2::new(16.0, 8.0));
        painter.rect_filled(rr, 6.0, Color32::from_black_alpha(170));
        painter.galley(rr.min + Vec2::new(8.0, 4.0), g, Color32::WHITE);
    }
    let ac_anchor = screen.cursor_position();
    drop(parser);
    if ac_on {
        crate::autocomplete::draw(&painter, &mut st.ac, &font, cs, origin, ac_anchor, rect, fg_default);
    }

    // context menu: copy / paste / clear
    let ctx_state = st.selection.is_some();
    let mut do_copy = false;
    let mut do_paste = false;
    let mut do_clear = false;
    resp.context_menu(|ui| {
        if ui.add_enabled(ctx_state, egui::Button::new("Copy")).clicked() {
            do_copy = true;
            ui.close();
        }
        if ui.button("Paste").clicked() {
            do_paste = true;
            ui.close();
        }
        if ui.button("Clear terminal").clicked() {
            do_clear = true;
            ui.close();
        }
    });
    if do_copy {
        if let Some(t) = selection_text(sess, st) {
            ui.ctx().copy_text(t);
        }
    }
    if do_paste {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }
    if do_clear {
        sess.write(&[0x0c]);
    }
    resp.clicked()
}

fn paste(sess: &PtySession, t: &str) {
    sess.write(t.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
}

fn selection_text(sess: &PtySession, st: &TermViewState) -> Option<String> {
    let (a, b) = st.selection?;
    let (a, b) = if (a.0, a.1) <= (b.0, b.1) { (a, b) } else { (b, a) };
    let p = sess.parser.lock().ok()?;
    let screen = p.screen();
    let (_, scols) = screen.size();
    let scols = scols.get();
    let mut out = String::new();
    for r in a.0..=b.0 {
        let c0 = if r == a.0 { a.1 } else { 0 };
        let c1 = if r == b.0 { b.1 } else { scols - 1 };
        let mut line = String::new();
        for c in c0..=c1.min(scols - 1) {
            if let Some(cell) = screen.cell(r, c) {
                if cell.is_wide_continuation() {
                    continue;
                }
                line.push_str(if cell.has_contents() { cell.contents() } else { " " });
            }
        }
        out.push_str(line.trim_end());
        if r != b.0 {
            out.push('\n');
        }
    }
    Some(out)
}

fn key_bytes(key: Key, m: egui::Modifiers, app_cursor: bool, option_meta: bool) -> Option<Vec<u8>> {
    let modn = 1 + (m.shift as u8) + 2 * (m.alt as u8) + 4 * (m.ctrl as u8);
    let arrow = |ch: char| -> Vec<u8> {
        if modn > 1 {
            format!("\x1b[1;{}{}", modn, ch).into_bytes()
        } else if app_cursor {
            format!("\x1bO{}", ch).into_bytes()
        } else {
            format!("\x1b[{}", ch).into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modn > 1 { format!("\x1b[{};{}~", n, modn).into_bytes() } else { format!("\x1b[{}~", n).into_bytes() }
    };
    let b = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Backspace => if m.alt { b"\x1b\x7f".to_vec() } else { b"\x7f".to_vec() },
        Key::Tab => if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
        Key::Escape => b"\x1b".to_vec(),
        Key::ArrowUp => arrow('A'),
        Key::ArrowDown => arrow('B'),
        Key::ArrowRight => arrow('C'),
        Key::ArrowLeft => arrow('D'),
        Key::Home => if modn > 1 { format!("\x1b[1;{}H", modn).into_bytes() } else { b"\x1b[H".to_vec() },
        Key::End => if modn > 1 { format!("\x1b[1;{}F", modn).into_bytes() } else { b"\x1b[F".to_vec() },
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Delete => tilde(3),
        Key::Insert => tilde(2),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ => {
            let name = key.name();
            let mut chars = name.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                if key == Key::Space && m.ctrl {
                    return Some(vec![0]);
                }
                return None;
            }
            if m.ctrl && !m.command_only_mac() && ch.is_ascii_alphabetic() {
                return Some(vec![(ch.to_ascii_uppercase() as u8) - b'A' + 1]);
            }
            if m.alt && option_meta && !m.ctrl {
                let c = if m.shift { ch.to_ascii_uppercase() } else { ch.to_ascii_lowercase() };
                let mut v = vec![0x1b];
                v.extend(c.to_string().into_bytes());
                return Some(v);
            }
            return None;
        }
    };
    Some(b)
}

trait ModExt {
    fn command_only_mac(&self) -> bool;
}
impl ModExt for egui::Modifiers {
    /// true when the "command" key (not control) is what is held on a Mac
    fn command_only_mac(&self) -> bool {
        cfg!(target_os = "macos") && self.mac_cmd && !self.ctrl
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    fn spec(script: &str) -> SessionSpec {
        SessionSpec {
            title: "t".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            is_ssh: true,
            term_type: "xterm-256color".into(),
            ..Default::default()
        }
    }

    fn wait_prompt(s: &PtySession) -> Option<AuthPrompt> {
        for _ in 0..60 {
            if let Some(p) = s.auth_prompt() {
                return Some(p);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    #[test]
    fn asks_for_a_missing_password_and_flags_a_wrong_one() {
        let script = "printf \"me@host's password: \"; read -r a; printf 'Permission denied, please try again.\\r\\nme@host password: '; read -r b; echo got:$b";
        let s = PtySession::spawn(spec(script), egui::Context::default(), 24, 80).unwrap();
        let p = wait_prompt(&s).expect("first prompt");
        assert!(!p.wrong && !p.passphrase);
        s.submit_auth("bad");
        let p = wait_prompt(&s).expect("second prompt");
        assert!(p.wrong);
        s.submit_auth("good");
        std::thread::sleep(Duration::from_millis(500));
        let text = s.parser.lock().unwrap().screen().contents();
        assert!(text.contains("got:good"), "{text}");
    }

    #[test]
    fn a_slow_refusal_asks_again_every_time() {
        // the host waits 3.5 s before refusing, twice, and prints the refusal before the next prompt.
        // Each wait allows 30 s: a busy CI machine can be several times slower than this one.
        let script = "printf \"me@host's password: \"; read -r a; sleep 3.5; printf '\\r\\nPermission denied, please try again.\\r\\n'; sleep 0.3; printf 'me@host password: '; read -r b; sleep 3.5; printf '\\r\\nPermission denied, please try again.\\r\\n'; sleep 0.3; printf 'me@host password: '; read -r c; echo got:$c";
        let s = PtySession::spawn(spec(script), egui::Context::default(), 24, 80).unwrap();
        let p = wait_prompt(&s).expect("first prompt");
        assert!(!p.wrong);
        s.submit_auth("bad1");
        let p = wait_long(&s, 300).expect("second prompt");
        assert!(p.wrong);
        s.submit_auth("bad2");
        let p = wait_long(&s, 300).expect("third prompt");
        assert!(p.wrong);
        // a refusal is never taken for a login
        assert!(s.shared.login_seen.lock().unwrap().is_none());
        s.submit_auth("good");
        std::thread::sleep(Duration::from_millis(500));
        assert!(s.shared.login_seen.lock().unwrap().is_some());
        let text = s.parser.lock().unwrap().screen().contents();
        assert!(text.contains("got:good"), "{text}");
    }

    #[test]
    fn a_refusal_that_arrives_in_pieces_is_still_a_refusal() {
        // the host is quiet for 3.5 s, then the refusal comes in two reads: a bare line break first
        // (a terminal flushes line by line), the rest a moment later. The first piece must not be
        // taken for the start of a session.
        let script = "printf \"me@host's password: \"; read -r a; sleep 3.5; printf '\\r\\n'; sleep 0.4; printf 'Permission denied, please try again.\\r\\n'; sleep 0.3; printf 'me@host password: '; read -r b; echo got:$b";
        let s = PtySession::spawn(spec(script), egui::Context::default(), 24, 80).unwrap();
        assert!(wait_prompt(&s).is_some(), "first prompt");
        s.submit_auth("bad");
        let p = wait_long(&s, 100).expect("second prompt after a refusal that came in pieces");
        assert!(p.wrong);
        assert!(s.shared.login_seen.lock().unwrap().is_none(), "a refusal is never taken for a login");
    }

    #[test]
    fn a_later_sudo_prompt_after_a_login_does_not_open_the_window() {
        // the login works after a quiet spell, and a password prompt of a program inside the session comes later
        let script = "printf \"me@host's password: \"; read -r a; sleep 3.4; echo Welcome; sleep 1.4; printf '[sudo] password for me: '; read -r c; echo got:$c";
        let s = PtySession::spawn(spec(script), egui::Context::default(), 24, 80).unwrap();
        assert!(wait_prompt(&s).is_some(), "first prompt");
        s.submit_auth("good");
        std::thread::sleep(Duration::from_millis(6500));
        assert!(s.parser.lock().unwrap().screen().contents().contains("[sudo] password"), "the sudo prompt did arrive");
        assert!(s.auth_prompt().is_none(), "a prompt from inside the session is not the login");
    }

    fn wait_long(s: &PtySession, tries: u32) -> Option<AuthPrompt> {
        for _ in 0..tries {
            if let Some(p) = s.auth_prompt() {
                return Some(p);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        None
    }

    #[test]
    fn saved_password_is_answered_without_a_window() {
        let mut sp = spec("printf \"me@host's password: \"; read -r a; echo got:$a");
        sp.password = Some("saved".into());
        let s = PtySession::spawn(sp, egui::Context::default(), 24, 80).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert!(s.auth_prompt().is_none());
        let text = s.parser.lock().unwrap().screen().contents();
        assert!(text.contains("got:saved"), "{text}");
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;
    use crate::vaultcrypt::{is_encrypted_log, read_log, LogKey};

    fn run(key: Option<LogKey>) -> (Vec<u8>, std::path::PathBuf) {
        let spec = SessionSpec {
            title: "logtest".into(),
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo visible-secret-output".into()],
            term_type: "xterm-256color".into(),
            log_key: key,
            ..Default::default()
        };
        let s = PtySession::spawn(spec, egui::Context::default(), 24, 80).unwrap();
        let path = s.log_path.clone().unwrap();
        for _ in 0..100 {
            if !s.shared.alive.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(200));
        (std::fs::read(&path).unwrap_or_default(), path)
    }

    #[test]
    fn session_log_is_encrypted_with_a_key_and_plain_without() {
        let key = LogKey::random().unwrap();
        let (bytes, path) = run(Some(key.clone()));
        assert!(is_encrypted_log(&bytes), "log should be encrypted");
        assert!(!String::from_utf8_lossy(&bytes).contains("visible-secret-output"));
        assert!(String::from_utf8_lossy(&read_log(&bytes, &key).unwrap()).contains("visible-secret-output"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_file(path);

        let (bytes, path) = run(None);
        assert!(String::from_utf8_lossy(&bytes).contains("visible-secret-output"));
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod guest_tests {
    use super::*;

    #[test]
    fn only_a_relay_join_tab_is_a_guest_tab() {
        let mut s = SessionSpec::default();
        assert!(!s.is_guest());
        s.args = vec!["-l".into()];
        assert!(!s.is_guest());
        s.args = vec!["--relay-join".into(), "addr".into()];
        assert!(s.is_guest());
    }
}

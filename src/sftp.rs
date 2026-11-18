//! The SFTP page: a two-pane file browser (this computer on the left, the chosen host on the right).
//!
//! Remote work runs the system `ssh` / `sftp` programs in background threads. One shared connection
//! (ControlMaster) is opened with the stored password or passphrase handed over through SSH_ASKPASS,
//! so listings, folder changes and transfers do not ask again.

use crate::app::App;
use crate::icons::{paint, Icon};
use crate::model::*;
use crate::screens::{empty_state, item_card, page_header};
use crate::ui::*;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use std::collections::HashSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[allow(non_snake_case)]
fn HDR() -> Color32 {
    pick(0x272A3C, 0xE9ECF2)
}
#[allow(non_snake_case)]
fn COLHDR() -> Color32 {
    pick(0x222537, 0xEFF1F6)
}
#[allow(non_snake_case)]
fn BODY() -> Color32 {
    pick(0x1B1E30, 0xFAFBFD)
}
#[allow(non_snake_case)]
fn HOVER_ROW() -> Color32 {
    pick(0x23273B, 0xEEF0F6)
}
const ROW_H: f32 = 40.0;
const BAR_H: f32 = 44.0;
/// Secondary text in the rows (dates, sizes, permissions): a little brighter than MUTED so it stays readable.
#[allow(non_snake_case)]
fn META() -> Color32 {
    pick(0xA4A8BC, 0x777C92)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Push,
    Back,
    Fwd,
    Stay,
}

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u64>,
    pub mtime_text: String,
    pub perms: String,
    pub kind: String,
}

struct ListResult {
    path: String,
    entries: Vec<Entry>,
    error: Option<String>,
    mode: Mode,
}

/// What the path box knows while someone types in it: folder names already read for each folder,
/// the one remote listing in flight, the highlighted row and where the list was drawn last frame.
#[derive(Default)]
struct PathAc {
    cache: std::collections::HashMap<String, Vec<String>>,
    pending: Option<(String, Receiver<ListResult>)>,
    sel: Option<usize>,
    dismissed: bool,
    touched: bool,
    rows: Vec<(Rect, String)>,
}

/// Splits what was typed into the folder part (up to and including the last slash) and the start of the next name.
fn split_typed(text: &str) -> (String, String) {
    if text == "~" {
        return ("~/".into(), String::new());
    }
    match text.rfind('/') {
        Some(i) => (text[..=i].to_string(), text[i + 1..].to_string()),
        None => (String::new(), text.to_string()),
    }
}

/// Folder names that start with what was typed (any case), A to Z. Hidden ones only when the typed start begins
/// with a dot or "Hidden files" is ticked.
fn pick_matches(names: &[String], prefix: &str, show_hidden: bool) -> Vec<String> {
    let low = prefix.to_lowercase();
    let mut v: Vec<String> = names
        .iter()
        .filter(|n| (show_hidden || prefix.starts_with('.') || !n.starts_with('.')) && n.to_lowercase().starts_with(&low))
        .cloned()
        .collect();
    v.sort_by_key(|n| n.to_lowercase());
    v
}

fn same_dir(a: &str, b: &str) -> bool {
    let t = |x: &str| if x.len() > 1 { x.trim_end_matches('/').to_string() } else { x.to_string() };
    t(a) == t(b)
}

fn dir_names(entries: &[Entry]) -> Vec<String> {
    entries.iter().filter(|e| e.is_dir).map(|e| e.name.clone()).collect()
}

/// The folder names inside `dir`: from the folder already open, from what was read before, straight from disk
/// (this computer) or by one extra listing in the background (a host). `None` while that listing is on its way.
fn path_names(pane: &mut Pane, eff: Side, cfg: &Option<Arc<RemoteCfg>>, dir: &str, ctx: &egui::Context) -> Option<Vec<String>> {
    let done = pane.ac.pending.as_ref().and_then(|(d, rx)| rx.try_recv().ok().map(|r| (d.clone(), r)));
    if let Some((d, r)) = done {
        let names = if r.error.is_some() { vec![] } else { dir_names(&r.entries) };
        pane.ac.cache.insert(d, names);
        pane.ac.pending = None;
    }
    if same_dir(dir, &pane.path) {
        return Some(dir_names(&pane.entries));
    }
    if let Some(v) = pane.ac.cache.get(dir) {
        return Some(v.clone());
    }
    if eff == Side::Local {
        let r = list_local(dir, Mode::Stay);
        let names = if r.error.is_some() { vec![] } else { dir_names(&r.entries) };
        pane.ac.cache.insert(dir.to_string(), names.clone());
        return Some(names);
    }
    if pane.ac.pending.is_none() {
        match cfg {
            Some(c) => pane.ac.pending = Some((dir.to_string(), spawn_list(c.clone(), dir.to_string(), Mode::Stay, ctx.clone()))),
            None => return Some(vec![]),
        }
    }
    None
}

#[derive(Default)]
pub struct Pane {
    pub path: String,
    pub entries: Vec<Entry>,
    back: Vec<String>,
    fwd: Vec<String>,
    filter: String,
    show_filter: bool,
    show_hidden: bool,
    path_edit: Option<String>,
    path_edit_new: bool,
    ac: PathAc,
    sort_col: u8,
    sort_desc: bool,
    selected: HashSet<String>,
    loading: bool,
    error: Option<String>,
    rx: Option<Receiver<ListResult>>,
}

pub struct OpResult {
    ok: bool,
    msg: String,
    refresh_local: bool,
    refresh_remote: bool,
}

#[derive(Clone)]
pub struct RemoteCfg {
    opts: Vec<String>,
    port: u16,
    target: String,
    secret: String,
}

enum Op {
    Mkdir(Side, String),
    Rename(Side, String, String),
    Delete(Side, Vec<String>),
    Upload(Vec<String>),
    Download(Vec<String>),
    OpenFiles(Side, Vec<String>, String),
}

#[derive(Clone)]
enum SAct {
    Open(String),
    Up,
    Go(String),
    Back,
    Fwd,
    Refresh,
    Sort(u8),
    Transfer,
    OpenFile(Option<String>),
    NewFolder,
    Rename,
    Delete,
    ChangeHost,
    ToggleHidden,
    /// A drag began on this row (it joins the selection unless it was already part of it).
    DragStart(String),
    LeftLocal,
    /// Close the connection(s) and go back to the host list.
    Disconnect,
}

struct SAuth {
    left: bool,
    wrong: bool,
    text: String,
    remember: bool,
}

fn is_auth_error(e: &str) -> bool {
    let l = e.to_lowercase();
    l.contains("permission denied") || l.contains("authentication failed") || l.contains("too many authentication")
}

struct Prompt {
    side: Side,
    rename_from: Option<String>,
    text: String,
}

/// One upload or download shown in the Transfers panel.
struct Transfer {
    id: u64,
    title: String,
    route: String,
    /// Total bytes when every item is a file (folders have no cheap size).
    total: Option<u64>,
    /// Where finished bytes can be counted: the local folder the files are written to (downloads only).
    watch: Option<(String, Vec<String>)>,
    /// Uploads: bytes seen on the host, filled in by a background thread that asks the host for the file sizes.
    remote: Option<(Arc<std::sync::atomic::AtomicU64>, Arc<std::sync::atomic::AtomicBool>)>,
    started: std::time::Instant,
    done_bytes: u64,
    speed: f64,
    last_tick: (std::time::Instant, u64),
    /// None while running; Some(Ok) / Some(Err(message)) when it ended.
    end: Option<(std::time::Instant, Result<(), String>)>,
}

impl Transfer {
    fn running(&self) -> bool {
        self.end.is_none()
    }

    /// True when the bytes that have moved can be counted (so there is a real percentage).
    fn measured(&self) -> bool {
        self.watch.is_some() || self.remote.is_some()
    }

    /// Measures how much has arrived so far and smooths the speed.
    fn tick(&mut self) {
        let got: u64 = if let Some((n, _)) = &self.remote {
            n.load(std::sync::atomic::Ordering::Relaxed)
        } else if let Some((dir, names)) = &self.watch {
            names.iter().map(|n| std::fs::metadata(join(dir, n)).map(|m| m.len()).unwrap_or(0)).sum()
        } else {
            return;
        };
        let now = std::time::Instant::now();
        self.done_bytes = self.total.map_or(got, |t| got.min(t));
        let dt = now.duration_since(self.last_tick.0).as_secs_f64();
        if dt >= 0.5 {
            let inst = self.done_bytes.saturating_sub(self.last_tick.1) as f64 / dt;
            self.speed = if self.speed == 0.0 { inst } else { self.speed * 0.6 + inst * 0.4 };
            self.last_tick = (now, self.done_bytes);
        }
    }
}

#[derive(Default)]
pub struct SftpState {
    /// The pane a drag started in, while the pointer is held down on a file or folder.
    drag: Option<Side>,
    transfers: Vec<Transfer>,
    transfers_collapsed: bool,
    next_transfer: u64,
    /// The transfer the running operation belongs to, if it is an upload or a download.
    cur_transfer: Option<u64>,
    pub filter: String,
    pub host: Option<u64>,
    local: Pane,
    remote: Pane,
    cfg: Option<Arc<RemoteCfg>>,
    /// When set, the left pane shows this host instead of this computer.
    left_host: Option<u64>,
    lcfg: Option<Arc<RemoteCfg>>,
    pick_left: bool,
    lconnect_started: Option<std::time::Instant>,
    /// The password window, while a host refuses the saved (or missing) password.
    auth: Option<SAuth>,
    op_rx: Option<Receiver<OpResult>>,
    watch_tx: Option<std::sync::mpsc::Sender<OpResult>>,
    watch_rx: Option<Receiver<OpResult>>,
    busy: Option<String>,
    prompt: Option<Prompt>,
    confirm_del: Option<(Side, Vec<String>)>,
    /// A transfer that would replace files already in the destination: (from, names, those that already exist, destination name).
    confirm_over: Option<(Side, Vec<String>, Vec<String>, String)>,
    seeded: bool,
    connect_started: Option<std::time::Instant>,
}

// ------------------------------------------------------------ helpers --

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn join(base: &str, name: &str) -> String {
    if base.ends_with('/') { format!("{}{}", base, name) } else { format!("{}/{}", base, name) }
}

fn parent(p: &str) -> String {
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => t[..i].to_string(),
    }
}

fn kind_of(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "folder".into();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_lowercase(),
        _ => "file".into(),
    }
}

fn fmt_us(secs: u64, tz: i64) -> String {
    let s = fmt_time(secs, tz); // YYYY-MM-DD HH:MM
    let (d, t) = s.split_once(' ').unwrap_or((&s, "00:00"));
    let mut dp = d.split('-');
    let y = dp.next().unwrap_or("");
    let m: u32 = dp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let day: u32 = dp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let mut tp = t.split(':');
    let h: u32 = tp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let mi = tp.next().unwrap_or("00");
    let (h12, ap) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{}/{}/{}, {}:{} {}", m, day, y, h12, mi, ap)
}

/// "1 item", "3 items".
fn count(n: usize, word: &str) -> String {
    format!("{} {}{}", n, word, if n == 1 { "" } else { "s" })
}

fn fmt_sz(n: u64) -> String {
    let f = n as f64;
    if n < 1000 {
        format!("{} B", n)
    } else if n < 1_000_000 {
        format!("{:.2} kB", f / 1e3)
    } else if n < 1_000_000_000 {
        format!("{:.2} MB", f / 1e6)
    } else {
        format!("{:.2} GB", f / 1e9)
    }
}

fn mode_str(mode: u32, is_dir: bool) -> String {
    let mut s = String::from(if is_dir { "d" } else { "-" });
    for (i, c) in "rwxrwxrwx".chars().enumerate() {
        s.push(if mode & (1 << (8 - i)) != 0 { c } else { '-' });
    }
    s
}

fn sort_entries(p: &Pane, view: &mut Vec<usize>) {
    let e = &p.entries;
    view.sort_by(|&a, &b| {
        let (x, y) = (&e[a], &e[b]);
        if x.is_dir != y.is_dir {
            return y.is_dir.cmp(&x.is_dir);
        }
        let ord = match p.sort_col {
            1 => x.mtime.cmp(&y.mtime),
            2 => x.size.cmp(&y.size),
            3 => x.kind.cmp(&y.kind).then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase())),
            _ => x.name.to_lowercase().cmp(&y.name.to_lowercase()),
        };
        if p.sort_desc { ord.reverse() } else { ord }
    });
}

// ---------------------------------------------------------- local side --

#[cfg(unix)]
fn local_perms(md: &std::fs::Metadata, is_dir: bool) -> String {
    use std::os::unix::fs::PermissionsExt;
    mode_str(md.permissions().mode(), is_dir)
}

// Windows has no Unix permission bits
#[cfg(not(unix))]
fn local_perms(_md: &std::fs::Metadata, _is_dir: bool) -> String {
    String::new()
}

fn list_local(path: &str, mode: Mode) -> ListResult {
    let mut entries = Vec::new();
    let rd = match std::fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => return ListResult { path: path.to_string(), entries, error: Some(format!("Cannot open {}: {}", path, e)), mode },
    };
    for ent in rd.flatten() {
        let name = ent.file_name().to_string_lossy().to_string();
        let md = std::fs::metadata(ent.path()).or_else(|_| ent.metadata());
        let Ok(md) = md else { continue };
        let is_dir = md.is_dir();
        entries.push(Entry {
            kind: kind_of(&name, is_dir),
            perms: local_perms(&md, is_dir),
            mtime: md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()),
            mtime_text: String::new(),
            size: if is_dir { 0 } else { md.len() },
            is_dir,
            name,
        });
    }
    ListResult { path: path.to_string(), entries, error: None, mode }
}

// --------------------------------------------------------- remote side --

fn askpass_script() -> String {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("askpass.sh");
    let body = "#!/bin/sh\nprintf '%s\\n' \"$SSHTUP_PW\"\n";
    if std::fs::read_to_string(&p).ok().as_deref() != Some(body) {
        let _ = std::fs::write(&p, body);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700));
    }
    p.to_string_lossy().to_string()
}

fn apply_env(c: &mut Command, cfg: &RemoteCfg) {
    c.env("SSH_ASKPASS", askpass_script()).env("SSH_ASKPASS_REQUIRE", "force").env("DISPLAY", ":0").env("SSHTUP_PW", &cfg.secret);
}

fn clean_err(stderr: &[u8], fallback: &str) -> String {
    let t = String::from_utf8_lossy(stderr).trim().to_string();
    if t.is_empty() { fallback.to_string() } else { t.lines().last().unwrap_or(fallback).to_string() }
}

/// Runs one command on the host (through a shell) and returns its output.
fn run_ssh(cfg: &RemoteCfg, script: &str) -> Result<String, String> {
    let mut c = Command::new("ssh");
    c.args(&cfg.opts);
    if cfg.port != DEFAULT_SSH_PORT {
        c.arg("-p").arg(cfg.port.to_string());
    }
    c.arg("--").arg(&cfg.target).arg(format!("sh -c {}", q(script)));
    apply_env(&mut c, cfg);
    c.stdin(Stdio::null());
    let out = c.output().map_err(|e| format!("Cannot run ssh: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(clean_err(&out.stderr, "The connection failed"))
    }
}

/// Every moment or so, asks the host how many bytes the given files hold and keeps the sum in the returned counter.
/// The thread stops when the flag is set or when nobody holds the counter any more.
fn spawn_size_poll(cfg: Arc<RemoteCfg>, paths: Vec<String>) -> (Arc<std::sync::atomic::AtomicU64>, Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let bytes = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (b, st) = (bytes.clone(), stop.clone());
    std::thread::spawn(move || {
        // `wc -c` reads the size without reading the file; a file that is not there yet counts as 0
        let script: String = paths.iter().map(|p| format!("wc -c < {} 2>/dev/null || echo 0; ", q(p))).collect();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        while !st.load(Ordering::Relaxed) && Arc::strong_count(&b) > 1 {
            if let Ok(out) = run_ssh(&cfg, &script) {
                let sum: u64 = out.lines().filter_map(|l| l.trim().parse::<u64>().ok()).sum();
                b.store(sum, Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_millis(700));
        }
    });
    (bytes, stop)
}

fn run_sftp(cfg: &RemoteCfg, batch: &str) -> Result<(), String> {
    let mut c = Command::new("sftp");
    c.arg("-b").arg("-").args(&cfg.opts);
    if cfg.port != DEFAULT_SSH_PORT {
        c.arg("-P").arg(cfg.port.to_string());
    }
    c.arg("--").arg(&cfg.target);
    apply_env(&mut c, cfg);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("Cannot run sftp: {}", e))?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(batch.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let o = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let msg = if !e.is_empty() { e } else { o };
        Err(msg.lines().last().unwrap_or("Transfer failed").to_string())
    }
}

fn sftp_q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Turns what the person typed into a path to open (`~` is the home folder; a name without a leading slash is inside the current folder).
fn resolve_path(side: Side, cur: &str, typed: &str) -> String {
    let t = typed.trim();
    match side {
        Side::Local => {
            let e = crate::app::expand_home(if t == "~" { "~/" } else { t });
            let e = if e.len() > 1 { e.trim_end_matches('/').to_string() } else { e };
            if e.starts_with('/') || cur.is_empty() { e } else { join(cur, &e) }
        }
        Side::Remote => {
            if t == "~" || t.starts_with("~/") || t.starts_with('/') || cur.is_empty() {
                if t.len() > 1 { t.trim_end_matches('/').to_string() } else { t.to_string() }
            } else {
                join(cur, t.trim_end_matches('/'))
            }
        }
    }
}

fn list_script(path: &str) -> String {
    let cd = if path == "~" || path.is_empty() {
        "cd".to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("cd -- \"$HOME\"/{}", q(rest))
    } else {
        format!("cd -- {}", q(path))
    };
    format!(
        "{} && pwd && {{ if ls --time-style=+%s -d . >/dev/null 2>&1; then LC_ALL=C ls -lAL --time-style=+%s 2>/dev/null; else LC_ALL=C ls -lAL 2>/dev/null; fi; true; }}",
        cd
    )
}

fn next_field(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    match s.find(char::is_whitespace) {
        Some(i) => Some((&s[..i], &s[i..])),
        None => Some((s, "")),
    }
}

/// Reads the output of `list_script`: the first line is the real path, the rest is `ls -l` lines.
fn parse_listing(out: &str) -> Option<(String, Vec<Entry>)> {
    let mut lines = out.lines();
    let path = lines.next()?.trim().to_string();
    let mut entries = Vec::new();
    for line in lines {
        let first = line.chars().next().unwrap_or(' ');
        if !"-dlcbps".contains(first) {
            continue;
        }
        let mut rest = line;
        let mut f: Vec<&str> = Vec::new();
        for _ in 0..5 {
            let Some((a, r)) = next_field(rest) else { break };
            f.push(a);
            rest = r;
        }
        if f.len() < 5 {
            continue;
        }
        let Some((sixth, r)) = next_field(rest) else { continue };
        let (mtime, mtime_text, name) = if let Ok(t) = sixth.parse::<u64>() {
            (Some(t), String::new(), r.trim_start().to_string())
        } else {
            let Some((d, r2)) = next_field(r) else { continue };
            let Some((t, r3)) = next_field(r2) else { continue };
            (None, format!("{} {} {}", sixth, d, t), r3.trim_start().to_string())
        };
        let name = match name.find(" -> ") {
            Some(i) if first == 'l' => name[..i].to_string(),
            _ => name,
        };
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        let is_dir = first == 'd';
        entries.push(Entry {
            kind: kind_of(&name, is_dir),
            perms: f[0].trim_end_matches(['@', '+', '.']).to_string(),
            size: if is_dir { 0 } else { f[4].parse().unwrap_or(0) },
            mtime,
            mtime_text,
            is_dir,
            name,
        });
    }
    Some((path, entries))
}

fn list_remote(cfg: &RemoteCfg, path: &str, mode: Mode) -> ListResult {
    match run_ssh(cfg, &list_script(path)) {
        Ok(out) => match parse_listing(&out) {
            Some((p, entries)) => ListResult { path: p, entries, error: None, mode },
            None => ListResult { path: path.to_string(), entries: vec![], error: Some("Folder not found".into()), mode },
        },
        Err(e) => ListResult { path: path.to_string(), entries: vec![], error: Some(e), mode },
    }
}

fn spawn_list(cfg: Arc<RemoteCfg>, path: String, mode: Mode, ctx: egui::Context) -> Receiver<ListResult> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = list_remote(&cfg, &path, mode);
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

fn launch_editor(key: &str, path: &str) -> Result<(), String> {
    let (prog, mut args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        match key {
            "vscode" => ("open", vec!["-a", "Visual Studio Code"]),
            "sublime" => ("open", vec!["-a", "Sublime Text"]),
            "textedit" => ("open", vec!["-a", "TextEdit"]),
            _ => ("open", vec![]),
        }
    } else {
        match key {
            "vscode" => ("code", vec![]),
            "sublime" => ("subl", vec![]),
            "textedit" => ("gedit", vec![]),
            _ => ("xdg-open", vec![]),
        }
    };
    args.push(path);
    let out = Command::new(prog).args(&args).output().map_err(|e| format!("Cannot start {}: {}", editor_label(key), e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Could not open with {} (is it installed?)", editor_label(key)))
    }
}

/// Removes temporary copies of remote files that are more than a day old.
fn clean_old_copies() {
    let dir = data_dir().join("open");
    let now = std::time::SystemTime::now();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).map(|d| d.as_secs() > 86_400).unwrap_or(false);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Watches a temporary copy and sends it back to the host each time it is saved (for up to four hours).
fn watch_and_upload(cfg: Arc<RemoteCfg>, local: String, remote: String, name: String, tx: std::sync::mpsc::Sender<OpResult>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let stamp = |p: &str| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let mut last = stamp(&local);
        for _ in 0..14_400 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = stamp(&local);
            if now.is_none() {
                return;
            }
            if now != last {
                std::thread::sleep(std::time::Duration::from_millis(600));
                last = stamp(&local);
                let res = run_ssh(&cfg, "true").and_then(|_| run_sftp(&cfg, &format!("put {} {}\n", sftp_q(&local), sftp_q(&remote))));
                let r = match res {
                    Ok(_) => OpResult { ok: true, msg: format!("Saved {} to the host", name), refresh_local: false, refresh_remote: true },
                    Err(e) => OpResult { ok: false, msg: format!("Could not save {} to the host: {}", name, e), refresh_local: false, refresh_remote: false },
                };
                let _ = tx.send(r);
                ctx.request_repaint();
            }
        }
    });
}

/// Copies items from one host to another through a temporary folder on this computer.
fn host_to_host(src: &RemoteCfg, spath: &str, dst: &RemoteCfg, dpath: &str, names: &[String]) -> Result<(), String> {
    run_ssh(src, "true")?;
    run_ssh(dst, "true")?;
    let tmp = data_dir().join("transfer").join(format!("{}", now_secs()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("Cannot create a temporary folder: {}", e))?;
    let t = tmp.to_string_lossy().to_string();
    let get: String = names.iter().map(|n| format!("get -R {} {}\n", sftp_q(&join(spath, n)), sftp_q(&join(&t, n)))).collect();
    let r = run_sftp(src, &get).and_then(|_| {
        let put: String = names.iter().map(|n| format!("put -R {} {}\n", sftp_q(&join(&t, n)), sftp_q(&join(dpath, n)))).collect();
        run_sftp(dst, &put)
    });
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

fn exec_op(op: Op, lpath: String, rpath: String, cfg: Option<Arc<RemoteCfg>>, lcfg: Option<Arc<RemoteCfg>>, wtx: std::sync::mpsc::Sender<OpResult>, ctx: egui::Context) -> OpResult {
    // the left pane shows a host: its folder work runs on that host, and transfers go host to host
    if let Some(lc) = lcfg {
        let flip = |mut r: OpResult| {
            std::mem::swap(&mut r.refresh_local, &mut r.refresh_remote);
            r
        };
        let need = || OpResult { ok: false, msg: "Not connected to the host".into(), refresh_local: true, refresh_remote: true };
        match op {
            Op::Mkdir(Side::Local, n) => return flip(exec_op(Op::Mkdir(Side::Remote, n), String::new(), lpath, Some(lc), None, wtx, ctx)),
            Op::Rename(Side::Local, a, b) => return flip(exec_op(Op::Rename(Side::Remote, a, b), String::new(), lpath, Some(lc), None, wtx, ctx)),
            Op::Delete(Side::Local, n) => return flip(exec_op(Op::Delete(Side::Remote, n), String::new(), lpath, Some(lc), None, wtx, ctx)),
            Op::OpenFiles(Side::Local, n, e) => return flip(exec_op(Op::OpenFiles(Side::Remote, n, e), String::new(), lpath, Some(lc), None, wtx, ctx)),
            Op::Upload(names) => {
                let Some(c) = cfg else { return need() };
                return match host_to_host(&lc, &lpath, &c, &rpath, &names) {
                    Ok(_) => OpResult { ok: true, msg: format!("Sent {} from one host to the other", count(names.len(), "item")), refresh_local: false, refresh_remote: true },
                    Err(e) => OpResult { ok: false, msg: e, refresh_local: true, refresh_remote: true },
                };
            }
            Op::Download(names) => {
                let Some(c) = cfg else { return need() };
                return match host_to_host(&c, &rpath, &lc, &lpath, &names) {
                    Ok(_) => OpResult { ok: true, msg: format!("Sent {} from one host to the other", count(names.len(), "item")), refresh_local: true, refresh_remote: false },
                    Err(e) => OpResult { ok: false, msg: e, refresh_local: true, refresh_remote: true },
                };
            }
            other => return exec_op(other, lpath, rpath, cfg, None, wtx, ctx),
        }
    }
    let fail = |m: String| OpResult { ok: false, msg: m, refresh_local: true, refresh_remote: true };
    let done = |m: String, l: bool, r: bool| OpResult { ok: true, msg: m, refresh_local: l, refresh_remote: r };
    let need = || "Not connected to the host".to_string();
    match op {
        Op::Mkdir(Side::Local, n) => match std::fs::create_dir(join(&lpath, &n)) {
            Ok(_) => done(format!("Created folder {}", n), true, false),
            Err(e) => fail(format!("Cannot create folder: {}", e)),
        },
        Op::Mkdir(Side::Remote, n) => {
            let Some(c) = cfg else { return fail(need()) };
            match run_ssh(&c, &format!("mkdir -- {}", q(&join(&rpath, &n)))) {
                Ok(_) => done(format!("Created folder {}", n), false, true),
                Err(e) => fail(e),
            }
        }
        Op::Rename(Side::Local, a, b) => match std::fs::rename(join(&lpath, &a), join(&lpath, &b)) {
            Ok(_) => done(format!("Renamed to {}", b), true, false),
            Err(e) => fail(format!("Cannot rename: {}", e)),
        },
        Op::Rename(Side::Remote, a, b) => {
            let Some(c) = cfg else { return fail(need()) };
            match run_ssh(&c, &format!("mv -- {} {}", q(&join(&rpath, &a)), q(&join(&rpath, &b)))) {
                Ok(_) => done(format!("Renamed to {}", b), false, true),
                Err(e) => fail(e),
            }
        }
        Op::Delete(Side::Local, names) => {
            for n in &names {
                let p = join(&lpath, n);
                let r = match std::fs::symlink_metadata(&p) {
                    Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
                    _ => std::fs::remove_file(&p),
                };
                if let Err(e) = r {
                    return fail(format!("Cannot delete {}: {}", n, e));
                }
            }
            done(format!("Deleted {}", count(names.len(), "item")), true, false)
        }
        Op::Delete(Side::Remote, names) => {
            let Some(c) = cfg else { return fail(need()) };
            let args: Vec<String> = names.iter().map(|n| q(&join(&rpath, n))).collect();
            match run_ssh(&c, &format!("rm -rf -- {}", args.join(" "))) {
                Ok(_) => done(format!("Deleted {}", count(names.len(), "item")), false, true),
                Err(e) => fail(e),
            }
        }
        Op::Upload(names) => {
            let Some(c) = cfg else { return fail(need()) };
            if let Err(e) = run_ssh(&c, "true") {
                return fail(e);
            }
            let batch: String = names.iter().map(|n| format!("put -R {} {}\n", sftp_q(&join(&lpath, n)), sftp_q(&join(&rpath, n)))).collect();
            match run_sftp(&c, &batch) {
                Ok(_) => done(format!("Uploaded {}", count(names.len(), "item")), false, true),
                Err(e) => fail(e),
            }
        }
        Op::Download(names) => {
            let Some(c) = cfg else { return fail(need()) };
            if let Err(e) = run_ssh(&c, "true") {
                return fail(e);
            }
            let batch: String = names.iter().map(|n| format!("get -R {} {}\n", sftp_q(&join(&rpath, n)), sftp_q(&join(&lpath, n)))).collect();
            match run_sftp(&c, &batch) {
                Ok(_) => done(format!("Downloaded {}", count(names.len(), "item")), true, false),
                Err(e) => fail(e),
            }
        }
        Op::OpenFiles(Side::Local, names, editor) => {
            for n in &names {
                if let Err(e) = launch_editor(&editor, &join(&lpath, n)) {
                    return fail(e);
                }
            }
            OpResult { ok: true, msg: format!("Opened {} with {}", count(names.len(), "file"), editor_label(&editor)), refresh_local: false, refresh_remote: false }
        }
        Op::OpenFiles(Side::Remote, names, editor) => {
            let Some(c) = cfg else { return fail(need()) };
            if let Err(e) = run_ssh(&c, "true") {
                return fail(e);
            }
            clean_old_copies();
            let dir = data_dir().join("open").join(format!("{}", now_secs()));
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return fail(format!("Cannot create a temporary folder: {}", e));
            }
            for n in &names {
                let local = dir.join(n).to_string_lossy().to_string();
                let remote = join(&rpath, n);
                if let Err(e) = run_sftp(&c, &format!("get {} {}\n", sftp_q(&remote), sftp_q(&local))) {
                    return fail(e);
                }
                if let Err(e) = launch_editor(&editor, &local) {
                    return fail(e);
                }
                watch_and_upload(c.clone(), local, remote, n.clone(), wtx.clone(), ctx.clone());
            }
            OpResult { ok: true, msg: format!("Opened {} with {}. Saving them there sends them back to the host.", count(names.len(), "file"), editor_label(&editor)), refresh_local: false, refresh_remote: false }
        }
    }
}

// ------------------------------------------------------------- state --

fn apply(p: &mut Pane, r: ListResult) {
    p.loading = false;
    if let Some(e) = r.error {
        p.error = Some(e);
        return;
    }
    match r.mode {
        Mode::Push => {
            if !p.path.is_empty() && p.path != r.path {
                p.back.push(p.path.clone());
                p.fwd.clear();
            }
        }
        Mode::Back => {
            p.back.pop();
            p.fwd.push(p.path.clone());
        }
        Mode::Fwd => {
            p.fwd.pop();
            p.back.push(p.path.clone());
        }
        Mode::Stay => {}
    }
    let same = p.path == r.path;
    p.path = r.path;
    p.entries = r.entries;
    p.error = None;
    if same {
        let names: HashSet<&String> = p.entries.iter().map(|e| &e.name).collect();
        p.selected.retain(|n| names.contains(n));
    } else {
        p.selected.clear();
        p.filter.clear();
    }
}

impl SftpState {
    fn pane(&mut self, s: Side) -> &mut Pane {
        match s {
            Side::Local => &mut self.local,
            Side::Remote => &mut self.remote,
        }
    }

    fn navigate(&mut self, side: Side, path: String, mode: Mode, ctx: &egui::Context) {
        match side {
            Side::Local => {
                if let Some(c) = self.lcfg.clone() {
                    let p = &mut self.local;
                    p.loading = true;
                    p.error = None;
                    p.rx = Some(spawn_list(c, path, mode, ctx.clone()));
                } else {
                    let r = list_local(&path, mode);
                    apply(&mut self.local, r);
                }
            }
            Side::Remote => {
                let cfg = self.cfg.clone();
                let p = &mut self.remote;
                match cfg {
                    Some(c) => {
                        p.loading = true;
                        p.error = None;
                        p.rx = Some(spawn_list(c, path, mode, ctx.clone()));
                    }
                    None => p.error = Some("Not connected to the host".into()),
                }
            }
        }
    }

    fn watch_sender(&mut self) -> std::sync::mpsc::Sender<OpResult> {
        if self.watch_tx.is_none() {
            let (tx, rx) = channel();
            self.watch_tx = Some(tx);
            self.watch_rx = Some(rx);
        }
        self.watch_tx.clone().unwrap()
    }

    fn start_op(&mut self, op: Op, label: &str, ctx: &egui::Context) {
        if self.busy.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let (l, r, cfg, lcfg) = (self.local.path.clone(), self.remote.path.clone(), self.cfg.clone(), self.lcfg.clone());
        let wtx = self.watch_sender();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = exec_op(op, l, r, cfg, lcfg, wtx, ctx.clone());
            let _ = tx.send(res);
            ctx.request_repaint();
        });
        self.op_rx = Some(rx);
        self.busy = Some(label.to_string());
    }

    /// Adds a row to the Transfers panel for the upload or download that has just started from `from`.
    fn begin_transfer(&mut self, from: Side, names: &[String], route: String) {
        let src = if from == Side::Local { &self.local } else { &self.remote };
        let mut total = Some(0u64);
        for n in names {
            match src.entries.iter().find(|e| &e.name == n) {
                Some(e) if !e.is_dir => total = total.map(|t| t + e.size),
                _ => total = None,
            }
        }
        let title = if names.len() == 1 { names[0].clone() } else { format!("{} items", names.len()) };
        // downloads to this computer can be measured by the size of the files that are arriving
        let watch = if from == Side::Remote && self.lcfg.is_none() && total.is_some() { Some((self.local.path.clone(), names.to_vec())) } else { None };
        // uploads to a host: ask the host how big the files have become
        let remote = match (&self.cfg, from == Side::Local && self.lcfg.is_none() && total.is_some()) {
            (Some(cfg), true) => Some(spawn_size_poll(cfg.clone(), names.iter().map(|n| join(&self.remote.path, n)).collect())),
            _ => None,
        };
        let now = std::time::Instant::now();
        self.next_transfer += 1;
        let id = self.next_transfer;
        self.transfers.push(Transfer { id, title, route, total, watch, remote, started: now, done_bytes: 0, speed: 0.0, last_tick: (now, 0), end: None });
        self.cur_transfer = Some(id);
        self.transfers_collapsed = false;
        // keep the list short: drop the oldest finished rows
        while self.transfers.len() > 20 {
            if let Some(i) = self.transfers.iter().position(|t| !t.running()) {
                self.transfers.remove(i);
            } else {
                break;
            }
        }
    }

    fn end_transfer(&mut self, ok: bool, msg: &str) {
        let Some(id) = self.cur_transfer.take() else { return };
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            if let Some((_, stop)) = &t.remote {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            t.tick();
            if ok {
                if let Some(total) = t.total {
                    t.done_bytes = total;
                }
            }
            t.end = Some((std::time::Instant::now(), if ok { Ok(()) } else { Err(msg.to_string()) }));
        }
    }
}

fn demo_entries(remote: bool) -> Vec<Entry> {
    let mk = |n: &str, d: bool, s: u64, t: u64, p: &str| Entry { name: n.into(), is_dir: d, size: s, mtime: Some(t), mtime_text: String::new(), perms: p.into(), kind: kind_of(n, d) };
    if remote {
        vec![
            mk("backups", true, 0, 1_794_000_000, "drwxr-xr-x"),
            mk("logs", true, 0, 1_794_200_000, "drwxr-x---"),
            mk("site", true, 0, 1_793_800_000, "drwxr-xr-x"),
            mk(".bashrc", false, 3771, 1_790_000_000, "-rw-r--r--"),
            mk("deploy.sh", false, 1840, 1_793_100_000, "-rwxr-xr-x"),
            mk("notes.txt", false, 12_480, 1_794_100_000, "-rw-r--r--"),
            mk("dump.sql", false, 48_210_000, 1_794_250_000, "-rw-------"),
        ]
    } else {
        vec![
            mk("Designs", true, 0, 1_794_000_000, "drwxr-xr-x"),
            mk("Invoices", true, 0, 1_793_000_000, "drwxr-xr-x"),
            mk("Music", true, 0, 1_792_000_000, "drwx------"),
            mk(".env", false, 312, 1_793_900_000, "-rw-------"),
            mk("config.json", false, 2480, 1_794_100_000, "-rw-r--r--"),
            mk("readme.md", false, 6120, 1_793_500_000, "-rw-r--r--"),
            mk("photo.png", false, 2_410_000, 1_794_200_000, "-rw-r--r--"),
        ]
    }
}

// ---------------------------------------------------------------- UI --

fn icon_btn(ui: &mut egui::Ui, icon: Icon, enabled: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, CARD_HOVER());
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    paint(ui.painter(), rect.center(), 15.0, icon, if enabled { TEXT() } else { MUTED().linear_multiply(0.45) });
    resp.on_hover_text(tip)
}

fn menu_items(ui: &mut egui::Ui, side: Side, lhost: bool, show_hidden: bool, acts: &mut Vec<(Side, SAct)>) {
    // room around the rows so a hovered row never touches the edge of the menu
    ui.spacing_mut().item_spacing = Vec2::new(0.0, 3.0);
    ui.spacing_mut().button_padding = Vec2::new(12.0, 7.0);
    ui.set_min_width(210.0);
    let t = match (side, lhost) {
        (Side::Local, false) => "Upload selected",
        (Side::Local, true) => "Send to right host",
        (Side::Remote, false) => "Download selected",
        (Side::Remote, true) => "Send to left host",
    };
    let mut items: Vec<(&str, SAct)> = vec![(t, SAct::Transfer), ("New folder", SAct::NewFolder), ("Rename", SAct::Rename), ("Delete selected", SAct::Delete), ("Refresh", SAct::Refresh)];
    if side == Side::Remote {
        items.push(("Change host", SAct::ChangeHost));
        items.push(("Disconnect", SAct::Disconnect));
    } else {
        items.push((if lhost { "Change host…" } else { "Connect to a host…" }, SAct::ChangeHost));
        if lhost {
            items.push(("Show this computer", SAct::LeftLocal));
        }
    }
    items.insert(0, ("Open", SAct::OpenFile(None)));
    for (label, a) in items {
        if label == "New folder" {
            ui.menu_button("Open with…", |ui| {
                for (k, name) in EDITORS {
                    if ui.button(name).clicked() {
                        acts.push((side, SAct::OpenFile(Some(k.to_string()))));
                        ui.close();
                    }
                }
            });
        }
        if ui.button(label).clicked() {
            acts.push((side, a));
            ui.close();
        }
        if label == "Refresh" {
            ui.separator();
            let mark = if show_hidden { "✓  " } else { "     " };
            if ui.button(format!("{}Hidden files", mark)).clicked() {
                acts.push((side, SAct::ToggleHidden));
                ui.close();
            }
            ui.separator();
        }
    }
}

fn section(ui: &mut egui::Ui, w: f32, h: f32, fill: Color32) -> Rect {
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter().rect_filled(r, 0.0, fill);
    r
}

#[allow(clippy::too_many_arguments)]
fn draw_pane(ui: &mut egui::Ui, rect: Rect, side: Side, lhost: bool, cfg: &Option<Arc<RemoteCfg>>, pane: &mut Pane, title: &str, badge_text: &str, badge_color: Color32, busy: Option<&str>, tz: i64, acts: &mut Vec<(Side, SAct)>) {
    let remote = side == Side::Remote;
    let builder = egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min));
    ui.scope_builder(builder, |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.painter().rect_filled(rect, 0.0, BODY());
        let w = rect.width();

        // ---- header
        let hr = section(ui, w, 46.0, HDR());
        let hb = egui::UiBuilder::new().max_rect(hr.shrink2(Vec2::new(14.0, 0.0))).layout(Layout::left_to_right(Align::Center));
        ui.scope_builder(hb, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
            badge(ui.painter(), r, badge_text, badge_color);
            ui.add(egui::Label::new(RichText::new(title).size(14.5).strong().color(TEXT())).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (ar, aresp) = ui.allocate_exact_size(Vec2::new(90.0, 28.0), Sense::click());
                let menu_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&aresp));
                if aresp.hovered() || menu_open {
                    ui.painter().rect_filled(ar, 6.0, CARD_HOVER());
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                paint(ui.painter(), Pos2::new(ar.left() + 14.0, ar.center().y), 14.0, Icon::List, TEXT());
                ui.painter().text(Pos2::new(ar.left() + 28.0, ar.center().y), Align2::LEFT_CENTER, "Actions", FontId::proportional(13.0), TEXT());
                let show_hidden_now = pane.show_hidden;
                egui::Popup::menu(&aresp).show(|ui| menu_items(ui, side, lhost, show_hidden_now, acts));
                let (fr, fresp) = ui.allocate_exact_size(Vec2::new(66.0, 28.0), Sense::click());
                if fresp.hovered() {
                    ui.painter().rect_filled(fr, 6.0, CARD_HOVER());
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let col = if pane.show_filter { BLUE() } else { TEXT() };
                paint(ui.painter(), Pos2::new(fr.left() + 13.0, fr.center().y), 14.0, Icon::Search, col);
                ui.painter().text(Pos2::new(fr.left() + 26.0, fr.center().y), Align2::LEFT_CENTER, "Filter", FontId::proportional(13.0), col);
                if fresp.clicked() {
                    pane.show_filter = !pane.show_filter;
                    if !pane.show_filter {
                        pane.filter.clear();
                    }
                }
                if let Some(b) = busy {
                    if remote || side == Side::Local {
                        ui.label(RichText::new(b).size(12.0).color(AMBER()));
                    }
                }
            });
        });

        // ---- breadcrumb
        let br = section(ui, w, 38.0, BODY());
        let bb = egui::UiBuilder::new().max_rect(br.shrink2(Vec2::new(10.0, 0.0))).layout(Layout::left_to_right(Align::Center));
        ui.scope_builder(bb, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
            if icon_btn(ui, Icon::ChevLeft, !pane.back.is_empty(), "Back").clicked() && !pane.back.is_empty() {
                acts.push((side, SAct::Back));
            }
            if icon_btn(ui, Icon::ChevRight, !pane.fwd.is_empty(), "Forward").clicked() && !pane.fwd.is_empty() {
                acts.push((side, SAct::Fwd));
            }
            ui.add_space(6.0);
            if pane.path_edit.is_some() {
                let first = pane.path_edit_new;
                pane.path_edit_new = false;
                let width = (ui.available_width() - 4.0).max(40.0);
                let ctx = ui.ctx().clone();
                let eid = egui::Id::new(("sftp_path_edit", remote));
                let eff = if lhost { Side::Remote } else { side };
                let keep_open = crate::ui::demo_pathac();

                // folders that match what is typed so far
                let text_now = pane.path_edit.clone().unwrap_or_default();
                let (dir_part, prefix) = split_typed(&text_now);
                let open = pane.ac.touched && !pane.ac.dismissed;
                let names = if open {
                    let dir = if dir_part.is_empty() { pane.path.clone() } else { resolve_path(eff, &pane.path, &dir_part) };
                    path_names(pane, eff, cfg, &dir, &ctx)
                } else {
                    Some(vec![])
                };
                let loading = open && names.is_none();
                let matches: Vec<String> = if open { names.as_ref().map(|n| pick_matches(n, &prefix, pane.show_hidden)).unwrap_or_default() } else { vec![] };
                let n = matches.len();
                if pane.ac.sel.map_or(false, |s| s >= n) {
                    pane.ac.sel = None;
                }

                // keys that belong to the list (taken before the text box sees them)
                let mut accept: Option<String> = None;
                if !first && ui.memory(|m| m.has_focus(eid)) {
                    use egui::{Key, Modifiers};
                    let pick = |i: usize| format!("{}{}/", dir_part, matches[i]);
                    if n > 0 {
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
                            pane.ac.sel = Some(pane.ac.sel.map_or(0, |s| (s + 1) % n));
                        }
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
                            pane.ac.sel = Some(pane.ac.sel.map_or(n - 1, |s| (s + n - 1) % n));
                        }
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
                            accept = Some(pick(pane.ac.sel.unwrap_or(0)));
                        } else if pane.ac.sel.is_some() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                            accept = Some(pick(pane.ac.sel.unwrap_or(0)));
                        }
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                            pane.ac.dismissed = true;
                            pane.ac.sel = None;
                        }
                    } else {
                        ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
                    }
                }
                // a click on a row (the box loses focus on that press, so it is read from the pointer)
                let pointer = ui.input(|i| i.pointer.interact_pos());
                if ui.input(|i| i.pointer.primary_pressed()) {
                    if let Some(p) = pointer {
                        if let Some((_, t)) = pane.ac.rows.iter().find(|(r, _)| r.contains(p)) {
                            accept = Some(t.clone());
                        }
                    }
                }
                let over_list = pointer.map_or(false, |p| pane.ac.rows.iter().any(|(r, _)| r.contains(p)));
                if let Some(t) = &accept {
                    let len = t.chars().count();
                    pane.path_edit = Some(t.clone());
                    pane.ac.sel = None;
                    pane.ac.dismissed = false;
                    pane.ac.touched = true;
                    if let Some(mut st) = egui::TextEdit::load_state(&ctx, eid) {
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(len))));
                        egui::TextEdit::store_state(&ctx, eid, st);
                    }
                    ctx.request_repaint();
                }

                let resp = ui.add(
                    egui::TextEdit::singleline(pane.path_edit.as_mut().unwrap())
                        .id(eid)
                        .event_filter(egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true })
                        .hint_text("Type a path and press Enter")
                        .desired_width(width)
                        .margin(Vec2::new(8.0, 5.0)),
                );
                if first || accept.is_some() {
                    resp.request_focus();
                }
                if resp.changed() {
                    pane.ac.touched = true;
                    pane.ac.sel = None;
                    pane.ac.dismissed = false;
                }
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if esc {
                    pane.path_edit = None;
                } else if resp.lost_focus() && accept.is_none() && !over_list && !keep_open {
                    let text = pane.path_edit.take().unwrap_or_default();
                    if enter {
                        let t = text.trim();
                        if !t.is_empty() {
                            acts.push((side, SAct::Go(resolve_path(eff, &pane.path, t))));
                        }
                    }
                }

                // the list under the box
                pane.ac.rows.clear();
                if pane.path_edit.is_some() && accept.is_none() && (n > 0 || loading) {
                    let shown: Vec<String> = matches.iter().take(8).cloned().collect();
                    let sel = pane.ac.sel;
                    let pos = resp.rect.left_bottom() + Vec2::new(0.0, 3.0);
                    let lw = resp.rect.width().clamp(160.0, 420.0);
                    let mut rows: Vec<(Rect, String)> = Vec::new();
                    egui::Area::new(egui::Id::new(("sftp_path_ac", remote))).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
                        egui::Frame::new()
                            .fill(pick(0x25283C, 0xE9ECF3))
                            .stroke(Stroke::new(1.0, BORDER()))
                            .corner_radius(8)
                            .inner_margin(4)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                ui.set_width(lw - 10.0);
                                if shown.is_empty() {
                                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
                                    ui.painter().text(Pos2::new(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, "Reading folders…", FontId::proportional(12.5), MUTED());
                                }
                                for (i, name) in shown.iter().enumerate() {
                                    let (r, rr) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
                                    if sel == Some(i) {
                                        ui.painter().rect_filled(r, 5.0, pick(0x2E3A5C, 0xD5E3F7));
                                    } else if rr.hovered() {
                                        ui.painter().rect_filled(r, 5.0, CARD_HOVER());
                                    }
                                    if rr.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    paint(ui.painter(), Pos2::new(r.left() + 16.0, r.center().y), 14.0, Icon::Folder, MUTED());
                                    ui.painter().text(Pos2::new(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, name, FontId::proportional(12.5), TEXT());
                                    rows.push((r, format!("{}{}/", dir_part, name)));
                                }
                            });
                    });
                    pane.ac.rows = rows;
                }
            } else {
            let mut start_edit = false;
            let segs: Vec<&str> = pane.path.split('/').filter(|s| !s.is_empty()).collect();
            let mut crumbs: Vec<(String, String)> = vec![("/".into(), "/".into())];
            let mut acc = String::new();
            for s in &segs {
                acc = format!("{}/{}", acc, s);
                crumbs.push((s.to_string(), acc.clone()));
            }
            if pane.path.is_empty() {
                crumbs.clear();
            }
            let skip = crumbs.len().saturating_sub(4);
            if skip > 0 {
                ui.label(RichText::new("…").color(MUTED()));
            }
            for (i, (label, full)) in crumbs.iter().enumerate().skip(skip) {
                if i > skip {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                    paint(ui.painter(), r.center(), 9.0, Icon::ChevRight, MUTED());
                }
                let (r, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                paint(ui.painter(), r.center(), 14.0, Icon::Folder, MUTED());
                let last = i + 1 == crumbs.len();
                let resp = ui.add(egui::Label::new(RichText::new(label).size(12.5).color(if last { TEXT() } else { MUTED() })).sense(Sense::click()));
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() && !last {
                    acts.push((side, SAct::Go(full.clone())));
                }
            }
            let rest = ui.available_width();
            if rest > 8.0 {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(rest, 28.0), Sense::click());
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                    ui.painter().rect_stroke(r, 6.0, Stroke::new(1.0, BORDER()), egui::StrokeKind::Inside);
                }
                if resp.clicked() {
                    start_edit = true;
                }
                resp.on_hover_text("Click to type a path");
            }
            if start_edit {
                pane.path_edit = Some(pane.path.clone());
                pane.path_edit_new = true;
                pane.ac = PathAc::default();
            }
            }
        });

        // ---- filter row
        if pane.show_filter {
            let fr = section(ui, w, 38.0, BODY());
            let fb = egui::UiBuilder::new().max_rect(fr.shrink2(Vec2::new(12.0, 4.0))).layout(Layout::left_to_right(Align::Center));
            ui.scope_builder(fb, |ui| {
                ui.add(egui::TextEdit::singleline(&mut pane.filter).hint_text("Filter by name").desired_width(w - 24.0).margin(Vec2::new(10.0, 6.0)));
            });
        }

        // ---- column header
        let cr = section(ui, w, 30.0, COLHDR());
        let kind_x = cr.right() - 62.0;
        let size_x = kind_x - 74.0;
        let date_x = size_x - 142.0;
        let cols: [(&str, f32, f32); 4] = [("Name", cr.left() + 16.0, date_x - 6.0), ("Date Modified", date_x, size_x - 6.0), ("Size", size_x, kind_x - 6.0), ("Kind", kind_x, cr.right())];
        for (i, (label, x0, x1)) in cols.iter().enumerate() {
            let r = Rect::from_min_max(Pos2::new(*x0 - if i == 0 { 16.0 } else { 0.0 }, cr.top()), Pos2::new(*x1, cr.bottom()));
            let resp = ui.interact(r, ui.id().with(("col", remote, i)), Sense::click());
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let active = pane.sort_col as usize == i;
            let col = if active { TEXT() } else { MUTED() };
            let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(12.0), col);
            let gw = galley.size().x;
            ui.painter().galley(Pos2::new(*x0, cr.center().y - galley.size().y / 2.0), galley, col);
            if active {
                paint(ui.painter(), Pos2::new(*x0 + gw + 10.0, cr.center().y), 9.0, if pane.sort_desc { Icon::ChevDown } else { Icon::ChevUp }, TEXT());
            }
            if resp.clicked() {
                acts.push((side, SAct::Sort(i as u8)));
            }
        }

        // ---- rows
        let q = pane.filter.trim().to_lowercase();
        let mut view: Vec<usize> = (0..pane.entries.len()).filter(|&i| (pane.show_hidden || !pane.entries[i].name.starts_with('.')) && (q.is_empty() || pane.entries[i].name.to_lowercase().contains(&q))).collect();
        sort_entries(pane, &mut view);
        let has_up = pane.path != "/" && !pane.path.is_empty();
        let n = view.len() + has_up as usize;
        let mods = ui.input(|i| i.modifiers);
        let show_hidden = pane.show_hidden;
        let mut clicked: Option<(String, bool)> = None;
        let mut dbl: Option<Option<String>> = None;
        let mut right: Option<String> = None;
        let body_h = (rect.bottom() - ui.cursor().top()).max(10.0);

        if let Some(e) = pane.error.clone() {
            let er = section(ui, w, 34.0, pick(0x3A1F2A, 0xFBE4E6));
            ui.painter().with_clip_rect(er).text(Pos2::new(er.left() + 14.0, er.center().y), Align2::LEFT_CENTER, e, FontId::proportional(12.0), pick(0xFFB4B4, 0xB02A2E));
        }
        if pane.loading && pane.entries.is_empty() {
            let lr = section(ui, w, 60.0, BODY());
            ui.painter().text(lr.center(), Align2::CENTER_CENTER, "Loading…", FontId::proportional(13.0), MUTED());
        } else if n == 0 && pane.error.is_none() {
            let lr = section(ui, w, 60.0, BODY());
            ui.painter().text(lr.center(), Align2::CENTER_CENTER, "This folder is empty", FontId::proportional(13.0), MUTED());
        }

        let sid = if remote { "rows_remote" } else { "rows_local" };
        egui::ScrollArea::vertical().id_salt(sid).max_height(body_h).auto_shrink([false, false]).show_rows(ui, ROW_H, n, |ui, range| {
            for row in range {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(w), ROW_H), Sense::click_and_drag());
                let p = ui.painter();
                let up_row = has_up && row == 0;
                let ent = if up_row { None } else { Some(&pane.entries[view[row - has_up as usize]]) };
                let selected = ent.map(|e| pane.selected.contains(&e.name)).unwrap_or(false);
                if selected {
                    p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(0x20, 0x91, 0xF6, 56));
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, HOVER_ROW());
                }
                let cy = rect.center().y;
                let ic = Pos2::new(rect.left() + 26.0, cy);
                match ent {
                    Some(e) if !e.is_dir => paint(p, ic, 18.0, Icon::File, MUTED()),
                    _ => paint(p, ic, 20.0, Icon::FolderFill, BLUE()),
                }
                let name_clip = Rect::from_min_max(Pos2::new(rect.left() + 42.0, rect.top()), Pos2::new(date_x - 8.0, rect.bottom()));
                let pc = p.with_clip_rect(name_clip);
                match ent {
                    None => {
                        pc.text(Pos2::new(rect.left() + 44.0, cy), Align2::LEFT_CENTER, "..", FontId::proportional(13.5), TEXT());
                    }
                    Some(e) => {
                        pc.text(Pos2::new(rect.left() + 44.0, cy - 7.0), Align2::LEFT_CENTER, &e.name, FontId::proportional(13.5), TEXT());
                        pc.text(Pos2::new(rect.left() + 44.0, cy + 9.0), Align2::LEFT_CENTER, &e.perms, FontId::monospace(10.5), META());
                        let date = match e.mtime {
                            Some(t) => fmt_us(t, tz),
                            None => e.mtime_text.clone(),
                        };
                        p.with_clip_rect(Rect::from_min_max(Pos2::new(date_x, rect.top()), Pos2::new(size_x - 4.0, rect.bottom()))).text(Pos2::new(date_x, cy), Align2::LEFT_CENTER, date, FontId::proportional(12.0), META());
                        let sz = if e.is_dir { "—".to_string() } else { fmt_sz(e.size) };
                        p.text(Pos2::new(size_x, cy), Align2::LEFT_CENTER, sz, FontId::proportional(12.0), META());
                        p.with_clip_rect(Rect::from_min_max(Pos2::new(kind_x, rect.top()), rect.right_bottom())).text(Pos2::new(kind_x, cy), Align2::LEFT_CENTER, &e.kind, FontId::proportional(12.0), META());
                    }
                }
                if resp.clicked() {
                    if let Some(e) = ent {
                        clicked = Some((e.name.clone(), mods.command || mods.ctrl));
                    }
                }
                if resp.double_clicked() {
                    dbl = Some(ent.map(|e| e.name.clone()));
                }
                if resp.drag_started() {
                    if let Some(e) = ent {
                        acts.push((side, SAct::DragStart(e.name.clone())));
                    }
                }
                if resp.secondary_clicked() {
                    if let Some(e) = ent {
                        right = Some(e.name.clone());
                    }
                }
                resp.context_menu(|ui| menu_items(ui, side, lhost, show_hidden, acts));
            }
        });

        if let Some((name, multi)) = clicked {
            if multi {
                if !pane.selected.remove(&name) {
                    pane.selected.insert(name);
                }
            } else {
                pane.selected.clear();
                pane.selected.insert(name);
            }
        }
        if let Some(name) = right {
            if !pane.selected.contains(&name) {
                pane.selected.clear();
                pane.selected.insert(name);
            }
        }
        match dbl {
            Some(Some(name)) => acts.push((side, SAct::Open(name))),
            Some(None) => acts.push((side, SAct::Up)),
            None => {}
        }
    });
}

fn fmt_dur(secs: f64) -> String {
    let t = secs.max(0.0).round() as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

/// One line of text that ends in "…" when it does not fit in `max_w`.
fn fit_text(p: &egui::Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    p.layout_job(job)
}

const PANEL_HEAD: f32 = 40.0;
const PANEL_ROW: f32 = 50.0;
const PANEL_MAX_ROWS: usize = 3;

/// Height of the list of transfers above the status bar (0 when there is nothing to show or it is hidden).
fn panel_height(s: &SftpState) -> f32 {
    if s.transfers.is_empty() || s.transfers_collapsed {
        0.0
    } else {
        PANEL_ROW * s.transfers.len().min(PANEL_MAX_ROWS) as f32 + 4.0
    }
}

/// What a pane shows in the status bar: items in the folder and what is selected.
fn pane_status(p: &Pane) -> (String, bool) {
    let q = p.filter.trim().to_lowercase();
    let count = p.entries.iter().filter(|e| (p.show_hidden || !e.name.starts_with('.')) && (q.is_empty() || e.name.to_lowercase().contains(&q))).count();
    let (mut n, mut bytes, mut dirs) = (0usize, 0u64, 0usize);
    for e in p.entries.iter().filter(|e| p.selected.contains(&e.name)) {
        n += 1;
        if e.is_dir {
            dirs += 1;
        } else {
            bytes += e.size;
        }
    }
    let mut t = format!("{} item{}", count, if count == 1 { "" } else { "s" });
    if n == 0 {
        t.push_str("  ·  No selection");
    } else {
        t.push_str(&format!("  ·  {} selected", n));
        if bytes > 0 || dirs == 0 {
            t.push_str(&format!(" · {}", fmt_sz(bytes)));
        }
        if dirs > 0 {
            t.push_str(if bytes > 0 { " + folders" } else { " (folders)" });
        }
    }
    (t, n > 0)
}

/// One bar under both panes: each pane's item count and selection, and the Transfers switch on the right.
fn draw_status_bar(ui: &mut egui::Ui, rect: Rect, split_x: f32, left: (String, bool), right: (String, bool), s: &mut SftpState) {
    ui.painter().rect_filled(rect, 0.0, COLHDR());
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, BORDER()));
    ui.painter().line_segment([Pos2::new(split_x, rect.top()), Pos2::new(split_x, rect.bottom())], Stroke::new(1.0, BORDER()));
    let cy = rect.center().y;
    let active = s.transfers.iter().filter(|t| t.running()).count();
    let finished = s.transfers.len() - active;

    // right edge: Transfers switch, and "Clear completed" when something has finished
    let mut x = rect.right() - 10.0;
    let mut btn = |ui: &mut egui::Ui, label: &str, badge: Option<String>, accent: bool, tip: &str, x: &mut f32| -> bool {
        let g = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(12.5), if accent { TEXT() } else { META() });
        let bg = badge.map(|b| ui.painter().layout_no_wrap(b, FontId::proportional(11.5), pick(0x9CC9FF, 0x1B6FC9)));
        let bw = bg.as_ref().map_or(0.0, |b| b.size().x + 16.0 + 8.0);
        let w = g.size().x + bw + 20.0;
        let r = Rect::from_min_size(Pos2::new(*x - w, cy - 14.0), Vec2::new(w, 28.0));
        let resp = ui.interact(r, ui.id().with(("bar_btn", label)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, CARD_HOVER());
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        ui.painter().galley(Pos2::new(r.left() + 10.0, cy - g.size().y / 2.0), g.clone(), TEXT());
        if let Some(b) = bg {
            let br = Rect::from_min_size(Pos2::new(r.left() + 10.0 + g.size().x + 8.0, cy - 10.0), Vec2::new(b.size().x + 16.0, 20.0));
            ui.painter().rect_filled(br, 10.0, Color32::from_rgba_unmultiplied(0x20, 0x91, 0xF6, 50));
            ui.painter().galley(Pos2::new(br.left() + 8.0, cy - b.size().y / 2.0), b, TEXT());
        }
        *x = r.left() - 4.0;
        resp.on_hover_text(tip).clicked()
    };
    let has = !s.transfers.is_empty();
    let label = if has && !s.transfers_collapsed { "Transfers  ▾" } else if has { "Transfers  ▴" } else { "Transfers" };
    let badge = (active > 0).then(|| format!("{} active", active));
    if btn(ui, label, badge, has, if has { "Show or hide the list of transfers" } else { "Uploads and downloads show up here" }, &mut x) && has {
        s.transfers_collapsed = !s.transfers_collapsed;
    }
    if finished > 0 && btn(ui, "Clear completed", None, false, "Remove finished transfers from the list", &mut x) {
        s.transfers.retain(|t| t.running());
    }
    let controls_left = x;

    for (info, x0, x1) in [(left, rect.left() + 16.0, split_x - 12.0), (right, split_x + 16.0, controls_left - 8.0)] {
        let col = if info.1 { TEXT() } else { META() };
        let g = fit_text(ui.painter(), &info.0, FontId::proportional(12.5), col, (x1 - x0).max(20.0));
        ui.painter().galley(Pos2::new(x0, cy - g.size().y / 2.0), g, col);
    }
}

/// The list of uploads and downloads above the status bar: progress, speed and time left while running.
fn draw_transfers(ui: &mut egui::Ui, rect: Rect, s: &mut SftpState) {
    let builder = egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min));
    ui.scope_builder(builder, |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.painter().rect_filled(rect, 0.0, BODY());
        ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, BORDER()));

        // rows, newest first
        let mut remove: Option<u64> = None;
        let body_h = (rect.bottom() - ui.cursor().top()).max(10.0);
        egui::ScrollArea::vertical().id_salt("transfer_rows").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for t in s.transfers.iter().rev() {
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), PANEL_ROW), Sense::hover());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 0.0, HOVER_ROW());
                }
                let cy = r.center().y;
                let state_w = 86.0;
                let bar_w = (r.width() * 0.16).clamp(80.0, 220.0);
                let bar_r = Rect::from_min_size(Pos2::new(r.right() - 16.0 - 28.0 - state_w - bar_w, cy - 3.0), Vec2::new(bar_w, 6.0));
                let info_right = bar_r.left() - 14.0;
                let p = ui.painter();

                // numbers: bytes, speed, time left
                let el = t.end.as_ref().map_or(t.started.elapsed(), |(at, _)| at.duration_since(t.started)).as_secs_f64();
                let info = match (&t.end, t.total) {
                    (None, Some(total)) if t.measured() => {
                        let mut v = format!("{} / {}", fmt_sz(t.done_bytes), fmt_sz(total));
                        if t.speed > 1.0 {
                            v.push_str(&format!("  ·  {}/s", fmt_sz(t.speed as u64)));
                            let left = total.saturating_sub(t.done_bytes) as f64 / t.speed;
                            v.push_str(&format!("  ·  {} left", fmt_dur(left)));
                        }
                        v
                    }
                    (None, Some(total)) => format!("{}  ·  {} elapsed", fmt_sz(total), fmt_dur(el)),
                    (None, None) => format!("{} elapsed", fmt_dur(el)),
                    (Some((_, Ok(()))), Some(total)) => format!("{}  ·  {}", fmt_sz(total), fmt_dur(el)),
                    (Some((_, Ok(()))), None) => fmt_dur(el),
                    (Some((_, Err(_))), _) => format!("stopped after {}", fmt_dur(el)),
                };
                let ig = p.layout_no_wrap(info, FontId::proportional(12.0), META());
                let iw = ig.size().x;
                let name_w = (info_right - iw - 20.0 - (r.left() + 16.0)).max(120.0);
                if info_right - iw > r.left() + 16.0 + 120.0 + 12.0 {
                    p.galley(Pos2::new(info_right - iw, cy - ig.size().y / 2.0), ig, META());
                }

                // name and route
                let tg = fit_text(p, &t.title, FontId::proportional(13.5), TEXT(), name_w);
                p.galley(Pos2::new(r.left() + 16.0, cy - 9.0 - tg.size().y / 2.0), tg, TEXT());
                let rg = fit_text(p, &t.route, FontId::proportional(11.5), META(), name_w);
                p.galley(Pos2::new(r.left() + 16.0, cy + 9.0 - rg.size().y / 2.0), rg, META());

                // bar
                p.rect_filled(bar_r, 3.0, pick(0x2A2E44, 0xDCE0EA));
                let (frac, col, state) = match &t.end {
                    None => match (t.total, t.measured()) {
                        (Some(total), true) if total > 0 => {
                            let f = (t.done_bytes as f32 / total as f32).clamp(0.0, 1.0);
                            (Some(f), BLUE(), format!("{}%", (f * 100.0).floor() as u32))
                        }
                        _ => (None, BLUE(), "Working…".to_string()),
                    },
                    Some((_, Ok(()))) => (Some(1.0), GREEN(), "Done".to_string()),
                    Some((_, Err(_))) => (Some(1.0), RED(), "Failed".to_string()),
                };
                match frac {
                    Some(f) => {
                        let w = (bar_r.width() * f).max(if f > 0.0 { 6.0 } else { 0.0 });
                        p.rect_filled(Rect::from_min_size(bar_r.min, Vec2::new(w, bar_r.height())), 3.0, col);
                    }
                    None => {
                        // no size to measure: a block slides along the bar
                        let phase = (ui.input(|i| i.time) * 0.9).fract() as f32;
                        let bw = bar_r.width() * 0.3;
                        let x0 = bar_r.left() + (bar_r.width() - bw) * (0.5 - 0.5 * (phase * std::f32::consts::TAU).cos());
                        p.rect_filled(Rect::from_min_size(Pos2::new(x0, bar_r.top()), Vec2::new(bw, bar_r.height())), 3.0, col);
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
                    }
                }
                let scol = match &t.end {
                    None => TEXT(),
                    Some((_, Ok(()))) => GREEN(),
                    Some((_, Err(_))) => RED(),
                };
                let state_x = bar_r.right() + 14.0;
                p.text(Pos2::new(state_x, cy), Align2::LEFT_CENTER, state, FontId::proportional(12.5), scol);

                // a finished row can be dismissed
                let xr = Rect::from_center_size(Pos2::new(r.right() - 16.0 - 11.0, cy), Vec2::splat(24.0));
                if t.running() {
                    ui.painter().text(xr.center(), Align2::CENTER_CENTER, "···", FontId::proportional(13.0), META());
                } else {
                    let xresp = ui.interact(xr, ui.id().with(("tr_x", t.id)), Sense::click());
                    if xresp.hovered() {
                        ui.painter().rect_filled(xr, 6.0, CARD_HOVER());
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    paint(ui.painter(), xr.center(), 13.0, Icon::Close, if xresp.hovered() { TEXT() } else { META() });
                    if xresp.on_hover_text("Remove from the list").clicked() {
                        remove = Some(t.id);
                    }
                }
                if let Some((_, Err(e))) = &t.end {
                    ui.interact(Rect::from_min_max(Pos2::new(state_x - 4.0, r.top()), Pos2::new(state_x + state_w, r.bottom())), ui.id().with(("tr_err", t.id)), Sense::hover()).on_hover_text(e.clone());
                }
                ui.painter().line_segment([Pos2::new(r.left() + 16.0, r.bottom()), Pos2::new(r.right() - 16.0, r.bottom())], Stroke::new(1.0, pick(0x25283B, 0xE4E7EE)));
            }
        });
        if let Some(id) = remove {
            s.transfers.retain(|t| t.id != id);
        }
    });
}

/// While files are being dragged: the pane they can be dropped on lights up, and a label follows the pointer.
fn draw_drag_overlay(ctx: &egui::Context, from: Side, target: Rect, target_name: &str, names: &[String], pointer: Pos2) -> bool {
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("sftp_dnd"));
    let p = ctx.layer_painter(layer);
    let over = target.contains(pointer);
    ctx.set_cursor_icon(if over { egui::CursorIcon::Copy } else { egui::CursorIcon::Grabbing });
    if over {
        p.rect_filled(target, 0.0, Color32::from_rgba_unmultiplied(0x20, 0x91, 0xF6, 26));
        p.rect_stroke(target.shrink(2.0), 6.0, Stroke::new(2.0, BLUE()), egui::StrokeKind::Inside);
        let verb = if from == Side::Local { "Upload to" } else { "Download to" };
        let g = p.layout_no_wrap(format!("{} {}", verb, target_name), FontId::proportional(14.0), Color32::WHITE);
        let pill = Rect::from_center_size(target.center(), g.size() + Vec2::new(36.0, 20.0));
        p.rect_filled(pill, pill.height() / 2.0, BLUE());
        p.galley(Pos2::new(pill.center().x - g.size().x / 2.0, pill.center().y - g.size().y / 2.0), g, Color32::WHITE);
    } else {
        p.rect_stroke(target.shrink(2.0), 6.0, Stroke::new(1.5, BLUE().linear_multiply(0.55)), egui::StrokeKind::Inside);
    }
    // what is being carried
    let label = if names.len() == 1 { names[0].clone() } else { count(names.len(), "item") };
    let g = p.layout_no_wrap(label, FontId::proportional(13.0), TEXT());
    let r = Rect::from_min_size(pointer + Vec2::new(14.0, 14.0), g.size() + Vec2::new(24.0, 14.0));
    p.rect_filled(r, 8.0, CARD_HOVER());
    p.rect_stroke(r, 8.0, Stroke::new(1.0, BLUE()), egui::StrokeKind::Inside);
    p.galley(Pos2::new(r.left() + 12.0, r.center().y - g.size().y / 2.0), g, TEXT());
    over
}

fn selected_sorted(p: &Pane) -> Vec<String> {
    let mut v: Vec<String> = p.selected.iter().cloned().collect();
    v.sort();
    v
}

impl App {
    /// Reads the host's own shell history in the background (once per host) for autocomplete.
    /// `typed_secret` is the password or passphrase typed in the window, when the vault has none.
    pub fn remote_history_start(&mut self, host_id: u64, typed_secret: Option<String>, ctx: &egui::Context) {
        let Some(h) = self.vault.hosts.iter().find(|x| x.id == host_id).cloned() else { return };
        let mut cfg = self.sftp_cfg(&h);
        if cfg.secret.is_empty() {
            cfg.secret = typed_secret.unwrap_or_default();
        }
        // a host that needs a password nobody has given would only count a failed login
        if cfg.secret.is_empty() && self.ssh_parts(&h).key.is_none() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = run_ssh(&cfg, crate::autocomplete::REMOTE_SCRIPT).map(|o| crate::autocomplete::parse_remote_history(&o));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.hist_jobs.push((host_id, rx));
    }

    /// Takes in finished history reads.
    pub fn remote_history_poll(&mut self) {
        let mut i = 0;
        while i < self.hist_jobs.len() {
            match self.hist_jobs[i].1.try_recv() {
                Ok(r) => {
                    let (host, _) = self.hist_jobs.remove(i);
                    if let Ok(cmds) = r {
                        self.history.merge_remote(host, cmds);
                    }
                    self.hist_tried.insert(host);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
                Err(_) => {
                    let (host, _) = self.hist_jobs.remove(i);
                    self.hist_tried.insert(host);
                }
            }
        }
    }

    pub fn sftp_cfg(&self, h: &Host) -> RemoteCfg {
        let p = self.ssh_parts(h);
        let mut opts = p.ssh_opts();
        let dir = data_dir();
        let _ = std::fs::create_dir_all(&dir);
        for o in [
            "ControlMaster=auto".to_string(),
            format!("ControlPath={}/cm-%C", dir.to_string_lossy()),
            "ControlPersist=300".to_string(),
            "ConnectTimeout=15".to_string(),
            "NumberOfPasswordPrompts=1".to_string(),
            "LogLevel=ERROR".to_string(),
        ] {
            opts.push("-o".into());
            opts.push(o);
        }
        let secret = if p.key.is_some() && p.passphrase.is_some() { p.passphrase.clone() } else { p.password.clone().or(p.passphrase.clone()) };
        RemoteCfg { opts, port: p.port, target: p.target, secret: secret.unwrap_or_default() }
    }

    pub(crate) fn sftp_open(&mut self, h: &Host, ctx: &egui::Context) {
        crate::model::tighten_known_hosts();
        if let Some(why) = self.connection_problem(h) {
            self.notify(why, RED());
            return;
        }
        let cfg = Arc::new(self.sftp_cfg(h));
        let s = &mut self.sftp;
        s.host = Some(h.id);
        s.cfg = Some(cfg);
        s.remote = Pane::default();
        s.navigate(Side::Remote, "~".into(), Mode::Stay, ctx);
    }

    fn sftp_left_open(&mut self, h: &Host, ctx: &egui::Context) {
        if let Some(why) = self.connection_problem(h) {
            self.notify(why, RED());
            return;
        }
        let cfg = Arc::new(self.sftp_cfg(h));
        let s = &mut self.sftp;
        s.left_host = Some(h.id);
        s.lcfg = Some(cfg);
        s.local = Pane::default();
        s.pick_left = false;
        s.navigate(Side::Local, "~".into(), Mode::Stay, ctx);
    }

    fn sftp_left_picker(&mut self, ctx: &egui::Context) {
        if !self.sftp.pick_left {
            return;
        }
        let hosts: Vec<Host> = self.vault.hosts.iter().filter(|h| h.protocol == Protocol::Ssh).cloned().collect();
        let mut go: Option<Host> = None;
        let mut local = false;
        let mut close = false;
        let (_, closed) = app_modal(ctx, "left_pane_modal", "Left pane", |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new("Show another host on the left to move files from host to host.").size(12.5).color(MUTED()));
            ui.add_space(8.0);
            if flat_button(ui, "This computer").clicked() {
                local = true;
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for h in &hosts {
                    let label = format!("{}  ·  {}", h.display_label(), h.address);
                    if ui.add_sized([ui.available_width(), 30.0], egui::Button::new(label)).clicked() {
                        go = Some(h.clone());
                    }
                }
                if hosts.is_empty() {
                    ui.label(RichText::new("No SSH hosts yet.").color(MUTED()));
                }
            });
            modal_buttons(ui, |ui| {
                if flat_button(ui, "Cancel").clicked() {
                    close = true;
                }
            });
        });
        close |= closed;
        if let Some(h) = go {
            self.sftp_left_open(&h, ctx);
        } else if local {
            self.sftp.pick_left = false;
            self.sftp_act(Side::Local, SAct::LeftLocal, ctx);
        } else if close {
            self.sftp.pick_left = false;
        }
    }

    fn sftp_act(&mut self, side: Side, act: SAct, ctx: &egui::Context) {
        let dflt = self.vault.settings.default_editor.clone();
        let s = &mut self.sftp;
        match act {
            SAct::Open(name) => {
                let p = s.pane(side);
                if p.entries.iter().any(|e| e.name == name && e.is_dir) {
                    let target = join(&p.path, &name);
                    s.navigate(side, target, Mode::Push, ctx);
                }
            }
            SAct::Up => {
                let t = parent(&s.pane(side).path);
                s.navigate(side, t, Mode::Push, ctx);
            }
            SAct::Go(p) => s.navigate(side, p, Mode::Push, ctx),
            SAct::Back => {
                if let Some(t) = s.pane(side).back.last().cloned() {
                    s.navigate(side, t, Mode::Back, ctx);
                }
            }
            SAct::Fwd => {
                if let Some(t) = s.pane(side).fwd.last().cloned() {
                    s.navigate(side, t, Mode::Fwd, ctx);
                }
            }
            SAct::Refresh => {
                let t = s.pane(side).path.clone();
                s.navigate(side, if t.is_empty() { "~".into() } else { t }, Mode::Stay, ctx);
            }
            SAct::Sort(c) => {
                let p = s.pane(side);
                if p.sort_col == c {
                    p.sort_desc = !p.sort_desc;
                } else {
                    p.sort_col = c;
                    p.sort_desc = false;
                }
            }
            SAct::Transfer => {
                let names = selected_sorted(s.pane(side));
                if names.is_empty() {
                    self.notify("Select a file or folder first".into(), AMBER());
                } else if s.busy.is_some() {
                    self.notify("Wait for the current transfer to finish".into(), AMBER());
                } else {
                    // files with the same name in the other pane would be replaced: ask first
                    let dst = if side == Side::Local { &s.remote } else { &s.local };
                    let clash: Vec<String> = names.iter().filter(|n| dst.entries.iter().any(|e| &e.name == *n)).cloned().collect();
                    if clash.is_empty() {
                        self.run_transfer(side, names, ctx);
                    } else {
                        let dname = if side == Side::Local {
                            s.host.and_then(|id| self.vault.host(Some(id))).map(|h| h.display_label()).unwrap_or_else(|| "the host".into())
                        } else {
                            s.left_host.and_then(|id| self.vault.host(Some(id))).map(|h| h.display_label()).unwrap_or_else(|| "this computer".into())
                        };
                        s.confirm_over = Some((side, names, clash, dname));
                    }
                }
            }
            SAct::OpenFile(choice) => {
                let editor = choice.unwrap_or(dflt);
                let p = s.pane(side);
                let mut files: Vec<String> = p.selected.iter().filter(|n| p.entries.iter().any(|e| &e.name == *n && !e.is_dir)).cloned().collect();
                files.sort();
                if files.is_empty() {
                    self.notify("Select a file to open".into(), AMBER());
                } else {
                    s.start_op(Op::OpenFiles(side, files, editor), "Opening…", ctx);
                }
            }
            SAct::NewFolder => s.prompt = Some(Prompt { side, rename_from: None, text: String::new() }),
            SAct::Rename => {
                let names = selected_sorted(s.pane(side));
                if names.len() == 1 {
                    s.prompt = Some(Prompt { side, rename_from: Some(names[0].clone()), text: names[0].clone() });
                } else {
                    self.notify("Select exactly one item to rename".into(), AMBER());
                }
            }
            SAct::Delete => {
                let names = selected_sorted(s.pane(side));
                if names.is_empty() {
                    self.notify("Select a file or folder first".into(), AMBER());
                } else {
                    s.confirm_del = Some((side, names));
                }
            }
            SAct::DragStart(name) => {
                let p = s.pane(side);
                if !p.selected.contains(&name) {
                    p.selected.clear();
                    p.selected.insert(name);
                }
                s.drag = Some(side);
            }
            SAct::ToggleHidden => {
                let p = s.pane(side);
                p.show_hidden = !p.show_hidden;
                if !p.show_hidden {
                    p.selected.retain(|n| !n.starts_with('.'));
                }
            }
            SAct::ChangeHost => {
                if side == Side::Local {
                    s.pick_left = true;
                } else {
                    s.host = None;
                    s.cfg = None;
                }
            }
            SAct::Disconnect => {
                // the whole browser: the right host and a left host, if there is one
                s.host = None;
                s.cfg = None;
                s.remote = Pane::default();
                s.connect_started = None;
                s.left_host = None;
                s.lcfg = None;
                s.local = Pane::default();
                s.lconnect_started = None;
                s.pick_left = false;
            }
            SAct::LeftLocal => {
                s.left_host = None;
                s.lcfg = None;
                s.local = Pane::default();
                s.lconnect_started = None;
            }
        }
    }

    /// Starts an upload (from the left pane) or a download (from the right pane) of the given names.
    fn run_transfer(&mut self, side: Side, names: Vec<String>, ctx: &egui::Context) {
        let s = &mut self.sftp;
        if names.is_empty() || s.busy.is_some() {
            return;
        }
        let n = names.len();
        let rl = s.host.and_then(|id| self.vault.host(Some(id))).map(|h| h.display_label()).unwrap_or_else(|| "host".into());
        let ll = s.left_host.and_then(|id| self.vault.host(Some(id))).map(|h| h.display_label()).unwrap_or_else(|| "This computer".into());
        let (lp, rp) = (s.local.path.clone(), s.remote.path.clone());
        let route = if side == Side::Local { format!("{}:{} → {}:{}", ll, lp, rl, rp) } else { format!("{}:{} → {}:{}", rl, rp, ll, lp) };
        let tn = names.clone();
        if side == Side::Local {
            s.start_op(Op::Upload(names), &format!("Uploading {}…", n), ctx);
        } else {
            s.start_op(Op::Download(names), &format!("Downloading {}…", n), ctx);
        }
        s.begin_transfer(side, &tn, route);
    }

    fn sftp_dialogs(&mut self, ctx: &egui::Context) {
        // name prompt (new folder / rename)
        let mut finish: Option<bool> = None;
        if let Some(p) = self.sftp.prompt.as_mut() {
            let title = if p.rename_from.is_some() { "Rename" } else { "New folder" };
            let (_, closed) = app_modal(ctx, "sftp_prompt_modal", title, |ui| {
                ui.set_width(300.0);
                let r = ui.add(egui::TextEdit::singleline(&mut p.text).hint_text("Name").desired_width(f32::INFINITY).margin(Vec2::new(10.0, 7.0)));
                r.request_focus();
                modal_buttons(ui, |ui| {
                    if primary_button(ui, "OK").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        finish = Some(true);
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        finish = Some(false);
                    }
                });
            });
            if closed {
                finish = Some(false);
            }
        }
        if let Some(ok) = finish {
            let p = self.sftp.prompt.take();
            if let (true, Some(p)) = (ok, p) {
                let name = p.text.trim().to_string();
                if name.is_empty() || name.contains('/') {
                    self.notify("Enter a valid name".into(), AMBER());
                } else if let Some(old) = p.rename_from {
                    self.sftp.start_op(Op::Rename(p.side, old, name), "Renaming…", ctx);
                } else {
                    self.sftp.start_op(Op::Mkdir(p.side, name), "Creating folder…", ctx);
                }
            }
        }

        // replace confirmation: Some(true) replaces, Some(false) skips the existing ones, None keeps waiting
        let mut over: Option<Option<bool>> = None;
        if let Some((_, names, clash, dest)) = &self.sftp.confirm_over {
            let msg = if clash.len() == 1 { format!("\"{}\" already exists in {}.", clash[0], dest) } else { format!("{} of these items already exist in {}.", clash.len(), dest) };
            let rest = names.len() > clash.len();
            let (_, closed) = app_modal(ctx, "sftp_overwrite_modal", "Replace existing?", |ui| {
                ui.set_width(340.0);
                ui.label(msg);
                ui.label(RichText::new("Replacing overwrites what is there now. This cannot be undone.").size(12.0).color(MUTED()));
                modal_buttons(ui, |ui| {
                    if danger_button(ui, "Replace").clicked() {
                        over = Some(Some(true));
                    }
                    if rest && flat_button(ui, "Skip existing").clicked() {
                        over = Some(Some(false));
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        over = Some(None);
                    }
                });
            });
            if closed {
                over = Some(None);
            }
        }
        if let Some(choice) = over {
            if let Some((side, names, clash, _)) = self.sftp.confirm_over.take() {
                match choice {
                    Some(true) => self.run_transfer(side, names, ctx),
                    Some(false) => self.run_transfer(side, names.into_iter().filter(|n| !clash.contains(n)).collect(), ctx),
                    None => {}
                }
            }
        }

        // delete confirmation
        let mut del: Option<bool> = None;
        if let Some((side, names)) = &self.sftp.confirm_del {
            let msg = if names.len() == 1 { format!("Delete \"{}\"?", names[0]) } else { format!("Delete {} items?", names.len()) };
            let where_ = if *side == Side::Local && self.sftp.lcfg.is_none() { "from this computer" } else { "from the host" };
            let (_, closed) = app_modal(ctx, "sftp_delete_modal", "Delete", |ui| {
                ui.set_width(300.0);
                ui.label(msg);
                ui.label(RichText::new(format!("This removes it {} for good.", where_)).size(12.0).color(MUTED()));
                modal_buttons(ui, |ui| {
                    if danger_button(ui, "Delete").clicked() {
                        del = Some(true);
                    }
                    if flat_button(ui, "Cancel").clicked() {
                        del = Some(false);
                    }
                });
            });
            if closed {
                del = Some(false);
            }
        }
        if let Some(yes) = del {
            if let Some((side, names)) = self.sftp.confirm_del.take() {
                if yes {
                    self.sftp.start_op(Op::Delete(side, names), "Deleting…", ctx);
                }
            }
        }
    }

    pub fn screen_sftp(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // demo (screenshot) data: no network, no real folders
        if self.demo && !self.sftp.seeded {
            self.sftp.seeded = true;
            if let Some(h) = self.vault.hosts.first() {
                self.sftp.host = Some(h.id);
            }
            let left_h = if std::env::var("SSHTUP_DEMO_LEFT_HOST").is_ok() { self.vault.hosts.get(1).cloned() } else { None };
            if let Some(h) = &left_h {
                self.sftp.left_host = Some(h.id);
                self.sftp.lcfg = Some(Arc::new(self.sftp_cfg(h)));
            }
            for (pane, remote, path) in [(0, left_h.is_some(), if left_h.is_some() { "/srv/backups" } else { "/Users/demo/Projects" }), (1, true, "/var/www/site")] {
                let mut p = Pane { path: path.into(), entries: demo_entries(remote), ..Default::default() };
                p.back.push(parent(path));
                if pane == 1 {
                    p.selected.insert("deploy.sh".into());
                    if crate::ui::demo_pathac() {
                        p.path_edit = Some("/var/www/s".into());
                        p.ac.touched = true;
                        p.ac.sel = Some(1);
                        p.ac.cache.insert("/var/www".into(), ["backups", "logs", "shared", "site", "staging", ".secrets", "static"].iter().map(|x| x.to_string()).collect());
                    }
                    self.sftp.remote = p;
                } else {
                    p.selected.insert(if remote { "dump.sql" } else { "config.json" }.into());
                    self.sftp.local = p;
                }
            }
            if std::env::var("SSHTUP_DEMO_OVERWRITE").is_ok() {
                self.sftp.confirm_over = Some((Side::Local, vec!["config.json".into(), "readme.md".into()], vec!["config.json".into()], "web-01".into()));
            }
            if std::env::var("SSHTUP_DEMO_DRAG").is_ok() {
                self.sftp.drag = Some(Side::Local);
            }
            if let Ok(t) = std::env::var("SSHTUP_DEMO_TOAST") {
                let col = match t.as_str() { "red" => RED(), "amber" => AMBER(), _ => GREEN() };
                self.notify("Deleted 1 item".into(), col);
            }
            if std::env::var("SSHTUP_DEMO_NO_TRANSFERS").is_err() {
                let now = std::time::Instant::now();
                let ago = |s: u64| now.checked_sub(std::time::Duration::from_secs(s)).unwrap_or(now);
                let host = self.vault.hosts.first().map(|h| h.display_label()).unwrap_or_else(|| "host".into());
                let mk = |id: u64, title: &str, total: u64, done: u64, speed: f64, started: u64, end: Option<(u64, Result<(), String>)>| Transfer {
                    id,
                    title: title.into(),
                    route: format!("{}:/var/www/site → This computer:/Users/demo/Projects", host),
                    total: Some(total),
                    watch: Some((String::new(), vec![])),
                    remote: None,
                    started: ago(started),
                    done_bytes: done,
                    speed,
                    last_tick: (now, done),
                    end: end.map(|(e, r)| (ago(e), r)),
                };
                self.sftp.transfers = vec![
                    mk(1, "notes.txt", 12_480, 12_480, 0.0, 40, Some((39, Ok(())))),
                    mk(2, "dump.sql", 48_210_000, 30_854_400, 2_400_000.0, 13, None),
                ];
                self.sftp.next_transfer = 2;
            }
        }

        // collect finished background work
        let mut got: Option<ListResult> = None;
        if let Some(rx) = &self.sftp.remote.rx {
            if let Ok(r) = rx.try_recv() {
                got = Some(r);
            }
        }
        if let Some(r) = got {
            self.sftp.remote.rx = None;
            apply(&mut self.sftp.remote, r);
        }
        if self.sftp.remote.loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        let mut lgot: Option<ListResult> = None;
        if let Some(rx) = &self.sftp.local.rx {
            if let Ok(r) = rx.try_recv() {
                lgot = Some(r);
            }
        }
        if let Some(r) = lgot {
            self.sftp.local.rx = None;
            apply(&mut self.sftp.local, r);
        }
        if self.sftp.local.loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        let mut op_done: Option<OpResult> = None;
        if let Some(rx) = &self.sftp.op_rx {
            if let Ok(r) = rx.try_recv() {
                op_done = Some(r);
            }
        }
        if let Some(r) = op_done {
            self.sftp.op_rx = None;
            self.sftp.busy = None;
            self.sftp.end_transfer(r.ok, &r.msg);
            self.notify(r.msg, if r.ok { GREEN() } else { RED() });
            if r.refresh_local && !self.demo {
                let p = self.sftp.local.path.clone();
                self.sftp.navigate(Side::Local, if p.is_empty() { "~".into() } else { p }, Mode::Stay, &ctx);
            }
            if r.refresh_remote {
                let p = self.sftp.remote.path.clone();
                self.sftp.navigate(Side::Remote, if p.is_empty() { "~".into() } else { p }, Mode::Stay, &ctx);
            }
        }
        let mut saved: Vec<OpResult> = Vec::new();
        if let Some(rx) = &self.sftp.watch_rx {
            while let Ok(r) = rx.try_recv() {
                saved.push(r);
            }
        }
        for r in saved {
            self.notify(r.msg, if r.ok { GREEN() } else { RED() });
            if r.refresh_remote && self.sftp.host.is_some() {
                let p = self.sftp.remote.path.clone();
                self.sftp.navigate(Side::Remote, if p.is_empty() { "~".into() } else { p }, Mode::Stay, &ctx);
            }
        }
        if self.sftp.busy.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if !self.demo {
            for t in self.sftp.transfers.iter_mut().filter(|t| t.running()) {
                t.tick();
            }
        }

        // host still there?
        if let Some(id) = self.sftp.host {
            if self.vault.host(Some(id)).is_none() {
                self.sftp.host = None;
            }
        }

        let Some(host_id) = self.sftp.host else {
            self.sftp_picker(ui, &ctx);
            return;
        };
        let host = self.vault.host(Some(host_id)).cloned().unwrap();

        if let Some(id) = self.sftp.left_host {
            if self.vault.host(Some(id)).is_none() {
                self.sftp_act(Side::Local, SAct::LeftLocal, &ctx);
            }
        }
        if self.sftp.lcfg.is_none() && self.sftp.local.path.is_empty() {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            self.sftp.navigate(Side::Local, home, Mode::Stay, &ctx);
        }

        let full_all = ui.available_rect_before_wrap();
        ui.allocate_rect(full_all, Sense::hover());
        let panel_h = panel_height(&self.sftp).min((full_all.height() - 200.0 - BAR_H).max(0.0));
        let full = Rect::from_min_max(full_all.min, Pos2::new(full_all.right(), full_all.bottom() - BAR_H - panel_h));
        let half = (full.width() / 2.0).floor();
        let left = Rect::from_min_size(full.min, Vec2::new(half, full.height()));
        let right = Rect::from_min_size(Pos2::new(full.left() + half, full.top()), Vec2::new(full.width() - half, full.height()));
        let mut acts: Vec<(Side, SAct)> = Vec::new();
        let busy = self.sftp.busy.clone();
        let tz = self.tz;
        let color = icon_color(&host.address);
        let letter = host.display_label().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let lhost = self.sftp.lcfg.is_some();
        let lh = self.sftp.left_host.and_then(|id| self.vault.host(Some(id)).cloned());
        if lhost && self.sftp.local.path.is_empty() && !self.sftp.local.loading && self.sftp.local.error.is_none() && !self.demo {
            self.sftp.navigate(Side::Local, "~".into(), Mode::Stay, &ctx);
        }
        match &lh {
            Some(h) => {
                let l = h.display_label().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                draw_pane(ui, left, Side::Local, true, &self.sftp.lcfg, &mut self.sftp.local, &h.display_label(), &l, icon_color(&h.address), busy.as_deref(), tz, &mut acts);
            }
            None => draw_pane(ui, left, Side::Local, false, &None, &mut self.sftp.local, "Local", "icon:folder", Color32::from_rgb(0x4A, 0x4F, 0x6A), busy.as_deref().filter(|_| false), tz, &mut acts),
        }
        draw_pane(ui, right, Side::Remote, lhost, &self.sftp.cfg, &mut self.sftp.remote, &host.display_label(), &letter, color, busy.as_deref(), tz, &mut acts);
        ui.painter().line_segment([Pos2::new(right.left(), full.top()), Pos2::new(right.left(), full.bottom())], Stroke::new(1.0, BORDER()));
        if panel_h > 0.0 {
            draw_transfers(ui, Rect::from_min_max(Pos2::new(full_all.left(), full.bottom()), Pos2::new(full_all.right(), full.bottom() + panel_h)), &mut self.sftp);
        }
        let (ls, rs) = (pane_status(&self.sftp.local), pane_status(&self.sftp.remote));
        draw_status_bar(ui, Rect::from_min_max(Pos2::new(full_all.left(), full_all.bottom() - BAR_H), full_all.right_bottom()), right.left(), ls, rs, &mut self.sftp);

        // cover while the first connection to the host is being made
        let connecting = crate::ui::force_connecting() || (self.sftp.remote.loading && self.sftp.remote.entries.is_empty() && self.sftp.remote.error.is_none());
        if connecting {
            let secs = self.sftp.connect_started.get_or_insert_with(std::time::Instant::now).elapsed().as_secs();
            if crate::ui::connecting_overlay(ui, full, &format!("Connecting to {}", host.display_label()), &crate::ui::connecting_hint(secs)) {
                self.sftp.host = None;
                self.sftp.remote = Pane::default();
                self.sftp.connect_started = None;
            }
        } else {
            self.sftp.connect_started = None;
        }

        // cover the left pane while its host is being reached
        if let Some(h) = &lh {
            let lc = crate::ui::force_connecting() || (self.sftp.local.loading && self.sftp.local.entries.is_empty() && self.sftp.local.error.is_none());
            if lc {
                let secs = self.sftp.lconnect_started.get_or_insert_with(std::time::Instant::now).elapsed().as_secs();
                if crate::ui::connecting_overlay(ui, left, &format!("Connecting to {}", h.display_label()), &crate::ui::connecting_hint(secs)) {
                    self.sftp_act(Side::Local, SAct::LeftLocal, &ctx);
                }
            } else {
                self.sftp.lconnect_started = None;
            }
        }

        // a host that refuses the login: ask for the password in a window
        if self.sftp.auth.is_none() {
            let r_bad = !self.sftp.remote.loading && self.sftp.remote.error.as_deref().map(is_auth_error).unwrap_or(false);
            let l_bad = lhost && !self.sftp.local.loading && self.sftp.local.error.as_deref().map(is_auth_error).unwrap_or(false);
            let forced = self.demo && crate::ui::force_password();
            if r_bad || l_bad || forced {
                let left = !r_bad && l_bad;
                let c = if left { self.sftp.lcfg.clone() } else { self.sftp.cfg.clone() };
                let wrong = (r_bad || l_bad) && c.as_ref().map(|c| !c.secret.is_empty()).unwrap_or(false);
                self.sftp.auth = Some(SAuth { left, wrong, text: String::new(), remember: false });
            }
        }
        if let Some(mut a) = self.sftp.auth.take() {
            let (rect, who, hid) = if a.left { (left, lh.as_ref().map(|h| h.display_label()).unwrap_or_default(), self.sftp.left_host) } else { (right, host.display_label(), self.sftp.host) };
            match crate::ui::password_modal(&ctx, rect, &who, false, a.wrong, &mut a.text, Some(&mut a.remember)) {
                crate::ui::PwAnswer::Waiting => self.sftp.auth = Some(a),
                crate::ui::PwAnswer::Cancel => {
                    if a.left {
                        self.sftp_act(Side::Local, SAct::LeftLocal, &ctx);
                    } else {
                        self.sftp.host = None;
                        self.sftp.cfg = None;
                        self.sftp.remote = Pane::default();
                        self.sftp.connect_started = None;
                    }
                }
                crate::ui::PwAnswer::Submit => {
                    let side = if a.left { Side::Local } else { Side::Remote };
                    let cur = if a.left { self.sftp.lcfg.clone() } else { self.sftp.cfg.clone() };
                    if let Some(cur) = cur {
                        let mut c = (*cur).clone();
                        c.secret = a.text.clone();
                        let c = Arc::new(c);
                        if a.left {
                            self.sftp.lcfg = Some(c);
                        } else {
                            self.sftp.cfg = Some(c);
                        }
                        if a.remember {
                            if let Some(h) = self.vault.hosts.iter_mut().find(|x| Some(x.id) == hid) {
                                h.password = a.text.clone();
                                self.dirty = true;
                            }
                        }
                        let p = self.sftp.pane(side);
                        let path = if p.path.is_empty() { "~".to_string() } else { p.path.clone() };
                        p.error = None;
                        self.sftp.navigate(side, path, Mode::Stay, &ctx);
                    }
                }
            }
        }

        // dragging files from one pane to the other
        if let Some(from) = self.sftp.drag {
            let (target, tname) = if from == Side::Local { (right, host.display_label()) } else { (left, lh.as_ref().map(|h| h.display_label()).unwrap_or_else(|| "Local".into())) };
            let (mut pos, mut down, released, esc) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.any_down(), i.pointer.any_released(), i.key_pressed(egui::Key::Escape)));
            if self.demo && std::env::var("SSHTUP_DEMO_DRAG").is_ok() {
                // screenshot runs have no pointer: pretend the files are being held over the other pane
                pos = Some(target.center() + Vec2::new(-60.0, -150.0));
                down = true;
            }
            let names = selected_sorted(self.sftp.pane(from));
            if esc || (!down && !released) || names.is_empty() {
                self.sftp.drag = None;
            } else if let Some(pos) = pos {
                let over = draw_drag_overlay(&ctx, from, target, &tname, &names, pos);
                if released {
                    self.sftp.drag = None;
                    if over {
                        acts.push((from, SAct::Transfer));
                    }
                } else {
                    ctx.request_repaint();
                }
            }
        }

        for (side, a) in acts {
            self.sftp_act(side, a, &ctx);
        }
        self.sftp_dialogs(&ctx);
        self.sftp_left_picker(&ctx);
    }

    fn sftp_picker(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        page_header(ui, "SFTP", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.sftp.filter).hint_text("Filter hosts").desired_width(220.0).margin(Vec2::new(10.0, 7.0)));
        });
        label_muted(ui, "Pick a host to browse its files next to the ones on this computer.");
        ui.add_space(8.0);
        let q = self.sftp.filter.trim().to_lowercase();
        let hosts: Vec<Host> = self.vault.hosts.iter().filter(|h| h.protocol == Protocol::Ssh && (q.is_empty() || h.label.to_lowercase().contains(&q) || h.address.to_lowercase().contains(&q))).cloned().collect();
        if hosts.is_empty() {
            empty_state(ui, "No SSH hosts", "Add a host first.");
        }
        let mut go: Option<Host> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let w = ui.available_width().min(760.0) - 1.0;
            for h in &hosts {
                let letter = h.display_label().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                let (c, _) = item_card(ui, Vec2::new(w, 56.0), &h.display_label(), &h.description(&self.vault.identities), &letter, icon_color(&h.address), None, "Open", 48.0, false);
                if c.clicked() {
                    go = Some(h.clone());
                }
            }
        });
        if let Some(h) = go {
            self.sftp_open(&h, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gnu_listing() {
        let out = "/home/me\ntotal 8\ndrwxr-xr-x 2 me me 4096 1794000000 my docs\n-rw-r--r-- 1 me me 120 1794100000 a.txt\nlrwxrwxrwx 1 me me 5 1794100000 link -> a.txt\n";
        let (p, e) = parse_listing(out).unwrap();
        assert_eq!(p, "/home/me");
        assert_eq!(e.len(), 3);
        assert!(e[0].is_dir && e[0].name == "my docs");
        assert_eq!(e[1].size, 120);
        assert_eq!(e[1].kind, "txt");
        assert_eq!(e[2].name, "link");
    }

    #[test]
    fn parses_bsd_listing() {
        let out = "/Users/me\ntotal 0\ndrwxr-xr-x  3 me  staff  96 Nov 14 02:12 Desktop\n-rw-r--r--@ 1 me  staff  10 Nov  3  2025 notes.md\n";
        let (_, e) = parse_listing(out).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].mtime_text, "Nov 14 02:12");
        assert_eq!(e[1].name, "notes.md");
        assert_eq!(e[1].perms, "-rw-r--r--");
    }

    #[test]
    fn list_script_runs_locally() {
        let dir = std::env::temp_dir().join("tc_sftp_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub dir")).unwrap();
        std::fs::write(dir.join("it's.txt"), b"hello").unwrap();
        let out = Command::new("sh").arg("-c").arg(list_script(&dir.to_string_lossy())).output().unwrap();
        let (_, e) = parse_listing(&String::from_utf8_lossy(&out.stdout)).unwrap();
        assert!(e.iter().any(|x| x.name == "sub dir" && x.is_dir));
        assert!(e.iter().any(|x| x.name == "it's.txt" && x.size == 5));
    }

    #[test]
    fn typed_paths() {
        assert_eq!(resolve_path(Side::Remote, "/var/www", "/etc/"), "/etc");
        assert_eq!(resolve_path(Side::Remote, "/var/www", "site"), "/var/www/site");
        assert_eq!(resolve_path(Side::Remote, "/var/www", "~/logs"), "~/logs");
        assert_eq!(resolve_path(Side::Local, "/tmp", "sub"), "/tmp/sub");
        assert!(list_script("~/my dir").starts_with("cd -- \"$HOME\"/'my dir'"));
        let home = std::env::var("HOME").unwrap();
        assert_eq!(resolve_path(Side::Local, "/tmp", "~"), home);
    }

    #[test]
    fn dates_and_sizes() {
        assert_eq!(fmt_sz(999), "999 B");
        assert_eq!(fmt_sz(2_410_000), "2.41 MB");
        assert_eq!(fmt_us(0, 0), "1/1/1970, 12:00 AM");
        assert_eq!(parent("/a/b"), "/a");
        assert_eq!(parent("/a"), "/");
    }
}

#[cfg(test)]
mod path_ac_tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn splits_at_the_last_slash() {
        assert_eq!(split_typed("/var/www/si"), ("/var/www/".into(), "si".into()));
        assert_eq!(split_typed("/var/"), ("/var/".into(), String::new()));
        assert_eq!(split_typed("docs"), (String::new(), "docs".into()));
        assert_eq!(split_typed("~"), ("~/".into(), String::new()));
        assert_eq!(split_typed("~/pro"), ("~/".into(), "pro".into()));
    }

    #[test]
    fn matches_start_of_name_in_any_case_sorted() {
        let n = names(&["Site", "staging", "backups", "shared"]);
        assert_eq!(pick_matches(&n, "s", false), vec!["shared", "Site", "staging"]);
        assert_eq!(pick_matches(&n, "SI", false), vec!["Site"]);
        assert!(pick_matches(&n, "x", false).is_empty());
    }

    #[test]
    fn hidden_folders_only_when_asked_for() {
        let n = names(&[".git", ".config", "src"]);
        assert_eq!(pick_matches(&n, "", false), vec!["src"]);
        assert_eq!(pick_matches(&n, ".", false), vec![".config", ".git"]);
        assert_eq!(pick_matches(&n, "", true), vec![".config", ".git", "src"]);
    }

    #[test]
    fn same_folder_ignores_trailing_slash() {
        assert!(same_dir("/var/www/", "/var/www"));
        assert!(same_dir("/", "/"));
        assert!(!same_dir("/var", "/var/www"));
    }
}

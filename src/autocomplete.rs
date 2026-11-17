//! Command autocomplete for the terminal: a small suggestion list under the cursor.
//!
//! The app only sees the keys the person presses, not the shell's own edit line, so the typed line
//! is a best guess. It is checked against what the terminal shows on the cursor row; whenever the
//! two disagree (history recall, shell completion, cursor moves, a password prompt) the list hides
//! until the next Enter.

use crate::model::data_dir;
use eframe::egui::{Color32, FontId, Key, Modifiers, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const MAX_SUGGESTIONS: usize = 6;
pub const MAX_HISTORY: usize = 500;
/// How long a typed key may go without showing on screen before the list is hidden (slow links).
const ECHO_GRACE: Duration = Duration::from_millis(700);

/// Common commands offered when nothing from this host's history or the snippets matches.
pub const COMMON: &[&str] = &[
    "ls -la",
    "ls -lh",
    "cd ..",
    "cd ~",
    "pwd",
    "clear",
    "cat /etc/os-release",
    "cat /etc/hosts",
    "df -h",
    "du -sh *",
    "free -h",
    "top",
    "htop",
    "ps aux",
    "ps aux | grep ",
    "kill -9 ",
    "uptime",
    "whoami",
    "uname -a",
    "history",
    "grep -rn ",
    "find . -name ",
    "tail -f /var/log/syslog",
    "tail -f /var/log/nginx/error.log",
    "tail -n 100 ",
    "less ",
    "nano ",
    "vim ",
    "mkdir -p ",
    "rm -rf ",
    "cp -r ",
    "mv ",
    "chmod +x ",
    "chmod 644 ",
    "chown -R ",
    "ln -s ",
    "tar -czvf archive.tar.gz ",
    "tar -xzvf ",
    "unzip ",
    "curl -I ",
    "curl -fsSL ",
    "wget ",
    "ping -c 4 ",
    "ssh-keygen -t ed25519",
    "ssh ",
    "scp ",
    "rsync -avz ",
    "netstat -tulpn",
    "ss -tulpn",
    "ip a",
    "ip route",
    "sudo ",
    "sudo apt update",
    "sudo apt upgrade -y",
    "sudo apt install ",
    "sudo systemctl status ",
    "sudo systemctl restart ",
    "sudo systemctl enable ",
    "sudo journalctl -u ",
    "sudo journalctl -f",
    "sudo reboot",
    "sudo ufw status",
    "systemctl list-units --type=service",
    "journalctl -xe",
    "docker ps",
    "docker ps -a",
    "docker images",
    "docker logs -f ",
    "docker exec -it ",
    "docker compose up -d",
    "docker compose down",
    "docker compose logs -f",
    "docker system prune",
    "kubectl get pods",
    "kubectl get nodes",
    "kubectl logs -f ",
    "kubectl describe pod ",
    "git status",
    "git add .",
    "git add -A",
    "git commit -m \"",
    "git push",
    "git pull",
    "git checkout ",
    "git switch ",
    "git branch",
    "git log --oneline",
    "git diff",
    "git stash",
    "git clone ",
    "npm install",
    "npm run build",
    "npm run dev",
    "python3 -m venv venv",
    "pip install ",
    "crontab -e",
    "crontab -l",
    "env",
    "export ",
    "which ",
    "man ",
    "exit",
];

/// Printed on the host to read its own shell history (bash, zsh and fish files in the home folder).
pub const REMOTE_SCRIPT: &str = r#"for f in "$HOME/.bash_history" "$HOME/.zsh_history" "$HOME/.local/share/fish/fish_history"; do if [ -r "$f" ]; then printf '@@FILE\n'; tail -n 1500 "$f" 2>/dev/null; printf '\n'; fi; done"#;

const SECRET_WORDS: &[&str] = &["password", "passwd", "secret", "token", "api_key", "apikey", "api-key", "authorization", "bearer ", "private_key", "sshpass", "--pass", "pwd=", "credential"];

fn usable_remote(cmd: &str) -> bool {
    let l = cmd.to_lowercase();
    cmd.len() >= 2 && cmd.len() <= 200 && !cmd.ends_with('\\') && !SECRET_WORDS.iter().any(|w| l.contains(w))
}

/// Turns the output of [`REMOTE_SCRIPT`] into commands, most recent first, without anything that
/// looks like it carries a secret.
pub fn parse_remote_history(out: &str) -> Vec<String> {
    let mut files: Vec<Vec<String>> = vec![];
    for line in out.lines() {
        if line.trim_end() == "@@FILE" {
            files.push(vec![]);
            continue;
        }
        let Some(cur) = files.last_mut() else { continue };
        let cmd: &str = if let Some(r) = line.strip_prefix(": ") {
            // zsh extended history: ": 1700000000:0;command"
            match r.split_once(';') {
                Some((meta, c)) if !meta.is_empty() && meta.chars().all(|ch| ch.is_ascii_digit() || ch == ':') => c,
                _ => continue,
            }
        } else if let Some(r) = line.strip_prefix("- cmd: ") {
            r // fish
        } else if line.starts_with(' ') || line.starts_with('\t') {
            continue; // fish "when:" / "paths:" lines
        } else if line.len() > 1 && line.starts_with('#') && line[1..].chars().all(|c| c.is_ascii_digit()) {
            continue; // bash time stamps
        } else {
            line
        };
        let cmd = cmd.trim();
        if usable_remote(cmd) {
            cur.push(cmd.to_string());
        }
    }
    for f in files.iter_mut() {
        f.reverse();
    }
    let mut out: Vec<String> = vec![];
    let mut i = 0;
    while out.len() < MAX_HISTORY && files.iter().any(|f| i < f.len()) {
        for f in &files {
            if let Some(c) = f.get(i) {
                if !out.contains(c) {
                    out.push(c.clone());
                }
            }
        }
        i += 1;
    }
    out.truncate(MAX_HISTORY);
    out
}

#[derive(Clone, PartialEq, Debug)]
pub struct Sugg {
    pub text: String,
    /// "history", "snippet" or "common"
    pub kind: &'static str,
}

/// Where suggestions come from, handed to the terminal for one frame.
pub struct AcSource<'a> {
    /// Commands run before on this host, most recent first.
    pub history: &'a [String],
    /// One-line snippet scripts.
    pub snippets: &'a [String],
}

#[derive(Default)]
pub struct AcState {
    /// The line the person is typing, as far as the app can tell.
    pub line: String,
    /// The shell's edit line can no longer be followed (cursor moved, history recalled, ...).
    pub lost: bool,
    pub dismissed: bool,
    pub sel: usize,
    pub typed_at: Option<Instant>,
    /// Commands finished with Enter since the app last collected them.
    pub new_commands: Vec<String>,
    pub shown: Vec<Sugg>,
    /// Where the rows were painted last frame (for clicks).
    pub rects: Vec<Rect>,
    /// Screenshot demo: the sample text was typed.
    pub demo_done: bool,
}

pub fn suggest(line: &str, src: &AcSource) -> Vec<Sugg> {
    let mut out: Vec<Sugg> = vec![];
    if line.trim().is_empty() {
        return out;
    }
    let mut add = |text: &str, kind: &'static str, out: &mut Vec<Sugg>| {
        if out.len() < MAX_SUGGESTIONS && text.len() > line.len() && text.starts_with(line) && !out.iter().any(|s| s.text == text) {
            out.push(Sugg { text: text.to_string(), kind });
        }
    };
    for h in src.history {
        add(h, "history", &mut out);
    }
    for sn in src.snippets {
        add(sn, "snippet", &mut out);
    }
    for c in COMMON {
        add(c, "common", &mut out);
    }
    out
}

/// True when the cursor row ends the way a password prompt does (nothing typed there is shown).
pub fn password_like(row_before_cursor: &str) -> bool {
    let t = row_before_cursor.trim_end().to_lowercase();
    t.ends_with(':') && (t.contains("password") || t.contains("passphrase") || t.contains("passcode") || t.contains("pin"))
}

/// Does the terminal show the typed line right before the cursor?
pub fn echoes(row_before_cursor: &str, line: &str) -> bool {
    let l = line.trim_end();
    !l.is_empty() && row_before_cursor.trim_end().ends_with(l)
}

impl AcState {
    pub fn reset(&mut self) {
        self.line.clear();
        self.lost = false;
        self.dismissed = false;
        self.sel = 0;
        self.shown.clear();
    }

    fn touch(&mut self) {
        self.typed_at = Some(Instant::now());
        self.dismissed = false;
        self.sel = 0;
    }

    pub fn typed_text(&mut self, t: &str) {
        self.line.push_str(t);
        self.touch();
    }

    pub fn pasted(&mut self, t: &str) {
        if t.contains('\n') || t.contains('\r') {
            self.lost = true;
        } else {
            self.line.push_str(t);
        }
        self.touch();
    }

    /// A key that was sent to the host (letters arrive as text, not here).
    pub fn key(&mut self, key: Key, m: Modifiers, echo_ok: bool) {
        if m.ctrl && !m.alt {
            match key {
                Key::C | Key::U => self.reset(),
                Key::W => {
                    let t = self.line.trim_end().to_string();
                    self.line = match t.rfind(' ') {
                        Some(i) => t[..=i].to_string(),
                        None => String::new(),
                    };
                    self.touch();
                }
                _ => self.lost = true,
            }
            return;
        }
        if m.alt || m.mac_cmd {
            if !self.line.is_empty() {
                self.lost = true;
            }
            return;
        }
        match key {
            Key::Enter => {
                let cmd = self.line.trim().to_string();
                if !self.lost && echo_ok && !cmd.is_empty() {
                    self.new_commands.push(cmd);
                }
                self.reset();
            }
            Key::Backspace => {
                self.line.pop();
                self.touch();
            }
            Key::ArrowUp | Key::ArrowDown => self.lost = true,
            Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End | Key::Delete | Key::PageUp | Key::PageDown | Key::Tab => {
                if !self.line.is_empty() {
                    self.lost = true;
                }
            }
            _ => {}
        }
    }

    /// Takes suggestion `i`: returns the text still to be sent to the host.
    pub fn accept(&mut self, i: usize) -> Option<String> {
        let s = self.shown.get(i)?.clone();
        let rest = s.text.get(self.line.len()..)?.to_string();
        self.line = s.text;
        self.shown.clear();
        self.sel = 0;
        self.touch();
        self.dismissed = true; // stays closed until the next key
        Some(rest)
    }

    /// Recomputes the list for this frame.
    pub fn refresh(&mut self, src: Option<&AcSource>, blocked: bool, echo_ok: bool) {
        let recent = self.typed_at.map(|t| t.elapsed() < ECHO_GRACE).unwrap_or(false);
        let ok = src.is_some() && !blocked && !self.lost && !self.dismissed && !self.line.trim().is_empty() && (echo_ok || recent);
        if !ok {
            self.shown.clear();
            self.rects.clear();
            return;
        }
        let new = suggest(&self.line, src.unwrap());
        if new != self.shown {
            self.shown = new;
            if self.sel >= self.shown.len() {
                self.sel = 0;
            }
        }
    }
}

/// Paints the list under (or over) the cursor. `anchor` is the cursor's (row, column).
pub fn draw(painter: &Painter, st: &mut AcState, font: &FontId, cs: Vec2, origin: Pos2, anchor: (u16, u16), area: Rect, fg: Color32) {
    st.rects.clear();
    if st.shown.is_empty() {
        return;
    }
    let n = st.shown.len();
    let row_h = cs.y + 8.0;
    let small = FontId::proportional(11.0);
    let muted = Color32::from_rgb(130, 136, 160);
    let accent = Color32::from_rgb(33, 150, 243);
    let mut text_w: f32 = 0.0;
    let mut tag_w: f32 = 0.0;
    for s in &st.shown {
        text_w = text_w.max(painter.layout_no_wrap(s.text.clone(), font.clone(), fg).size().x);
        tag_w = tag_w.max(painter.layout_no_wrap(s.kind.to_string(), small.clone(), muted).size().x);
    }
    let w = (text_w + tag_w + 44.0).max(240.0).min(area.width() - 12.0);
    let h = n as f32 * row_h + 8.0;
    let line_cols = st.line.chars().count() as u16;
    let mut x = origin.x + anchor.1.saturating_sub(line_cols) as f32 * cs.x - 8.0;
    x = x.min(area.right() - w - 6.0).max(area.left() + 6.0);
    let below = origin.y + (anchor.0 as f32 + 1.0) * cs.y + 4.0;
    let y = if below + h > area.bottom() - 4.0 { (origin.y + anchor.0 as f32 * cs.y - h - 4.0).max(area.top() + 4.0) } else { below };
    let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
    painter.rect_filled(rect.translate(Vec2::new(0.0, 3.0)).expand(1.0), 10.0, Color32::from_black_alpha(90));
    painter.rect_filled(rect, 8.0, Color32::from_rgb(28, 31, 46));
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, Color32::from_rgb(58, 63, 88)), StrokeKind::Inside);
    for (i, s) in st.shown.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(rect.left() + 4.0, rect.top() + 4.0 + i as f32 * row_h), Vec2::new(w - 8.0, row_h));
        if i == st.sel {
            painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 70));
        }
        let typed = s.text.get(..st.line.len()).unwrap_or("");
        let rest = s.text.get(st.line.len()..).unwrap_or("");
        let g1 = painter.layout_no_wrap(typed.to_string(), font.clone(), muted);
        let tx = r.left() + 8.0;
        let ty = r.center().y - g1.size().y / 2.0;
        let w1 = g1.size().x;
        painter.galley(Pos2::new(tx, ty), g1, muted);
        let g2 = painter.layout_no_wrap(rest.to_string(), font.clone(), fg);
        painter.galley(Pos2::new(tx + w1, ty), g2, fg);
        let gt = painter.layout_no_wrap(s.kind.to_string(), small.clone(), muted);
        painter.galley(Pos2::new(r.right() - 8.0 - gt.size().x, r.center().y - gt.size().y / 2.0), gt, muted);
        st.rects.push(r);
    }
}

// ------------------------------------------------------------------ history --

/// Commands run on each host (key 0 = local terminals), most recent first, kept on this computer.
/// When the vault is encrypted the file is encrypted with the vault's data key.
#[derive(Default)]
pub struct History {
    map: HashMap<u64, Vec<String>>,
    /// Hosts whose own shell history was already read.
    done: HashSet<u64>,
    pub dirty: bool,
    key: Option<crate::vaultcrypt::LogKey>,
    /// False when the file on disk could not be read (encrypted and no key): never overwrite it then.
    readable: bool,
}

fn history_path() -> std::path::PathBuf {
    data_dir().join("history.json")
}

impl History {
    pub fn load(key: Option<&crate::vaultcrypt::LogKey>) -> Self {
        Self::load_from(&history_path(), key)
    }

    pub fn load_from(path: &std::path::Path, key: Option<&crate::vaultcrypt::LogKey>) -> Self {
        let bytes = std::fs::read(path).unwrap_or_default();
        let plain: Option<Vec<u8>> = if crate::vaultcrypt::is_sealed(&bytes) {
            key.and_then(|k| crate::vaultcrypt::unseal(k, "history", &bytes).ok())
        } else {
            Some(bytes.clone())
        };
        let readable = plain.is_some();
        let raw: HashMap<String, Vec<String>> = plain.and_then(|p| serde_json::from_slice(&p).ok()).unwrap_or_default();
        let done = raw.get("remote_read").map(|v| v.iter().filter_map(|x| x.parse::<u64>().ok()).collect()).unwrap_or_default();
        let map = raw.into_iter().filter_map(|(k, v)| k.parse::<u64>().ok().map(|k| (k, v))).collect();
        Self { map, done, dirty: false, key: key.cloned(), readable }
    }

    /// Switches between encrypted and plain storage; call `save` afterwards to rewrite the file.
    pub fn set_key(&mut self, key: Option<crate::vaultcrypt::LogKey>) {
        self.key = key;
        self.readable = true;
    }

    pub fn get(&self, host: u64) -> &[String] {
        self.map.get(&host).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn add(&mut self, host: u64, cmd: &str) {
        let v = self.map.entry(host).or_default();
        v.retain(|c| c != cmd);
        v.insert(0, cmd.to_string());
        v.truncate(MAX_HISTORY);
        self.dirty = true;
    }

    pub fn remote_read(&self, host: u64) -> bool {
        self.done.contains(&host)
    }

    /// Adds what the host's own shell history held, after what was typed here.
    pub fn merge_remote(&mut self, host: u64, cmds: Vec<String>) {
        let v = self.map.entry(host).or_default();
        for c in cmds {
            if !v.contains(&c) {
                v.push(c);
            }
        }
        v.truncate(MAX_HISTORY);
        self.done.insert(host);
        self.dirty = true;
    }

    /// How many commands are remembered, over all hosts.
    pub fn count(&self) -> usize {
        self.map.values().map(|v| v.len()).sum()
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.map.clear();
        self.dirty = true;
    }

    pub fn save(&mut self) -> bool {
        self.save_to(&history_path())
    }

    pub fn save_to(&mut self, path: &std::path::Path) -> bool {
        self.dirty = false;
        if !self.readable {
            return false; // an encrypted file we could not open must not be replaced by an empty one
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let mut raw: HashMap<String, Vec<String>> = self.map.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        raw.insert("remote_read".into(), self.done.iter().map(|h| h.to_string()).collect());
        let Ok(json) = serde_json::to_vec(&raw) else { return false };
        let json = zeroize::Zeroizing::new(json);
        let bytes = match &self.key {
            Some(k) => match crate::vaultcrypt::seal(k, "history", &json) {
                Ok(b) => b,
                Err(_) => return false, // never fall back to plain text
            },
            None => json.to_vec(),
        };
        crate::model::write_private(path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src<'a>(h: &'a [String], s: &'a [String]) -> AcSource<'a> {
        AcSource { history: h, snippets: s }
    }

    #[test]
    fn history_first_then_snippets_then_common() {
        let h = vec!["git status -sb".to_string(), "git stash list".to_string()];
        let s = vec!["git switch main".to_string()];
        let out = suggest("git s", &src(&h, &s));
        assert_eq!(out[0].text, "git status -sb");
        assert_eq!(out[0].kind, "history");
        assert_eq!(out[2].kind, "snippet");
        assert!(out.iter().any(|x| x.kind == "common" && x.text == "git status"));
        assert!(out.len() <= MAX_SUGGESTIONS);
        assert!(suggest("", &src(&h, &s)).is_empty());
        assert!(suggest("git status -sb", &src(&h, &s)).iter().all(|x| x.text != "git status -sb"));
    }

    #[test]
    fn follows_typing_and_accepts_the_rest() {
        let h = vec!["docker compose up -d".to_string()];
        let mut st = AcState::default();
        st.typed_text("dock");
        st.refresh(Some(&src(&h, &[])), false, true);
        assert_eq!(st.shown[0].text, "docker compose up -d");
        assert_eq!(st.accept(0).as_deref(), Some("er compose up -d"));
        assert_eq!(st.line, "docker compose up -d");
        st.refresh(Some(&src(&h, &[])), false, true);
        assert!(st.shown.is_empty(), "closed after accepting");
        st.key(Key::Enter, Modifiers::NONE, true);
        assert_eq!(st.new_commands, vec!["docker compose up -d".to_string()]);
    }

    #[test]
    fn hides_when_it_cannot_follow_the_line() {
        let mut st = AcState::default();
        st.typed_text("git s");
        st.key(Key::ArrowLeft, Modifiers::NONE, true);
        assert!(st.lost);
        st.refresh(Some(&src(&[], &[])), false, true);
        assert!(st.shown.is_empty());
        // Enter ends it and nothing is recorded
        st.key(Key::Enter, Modifiers::NONE, true);
        assert!(st.new_commands.is_empty());
        assert!(!st.lost);
        // history recall on an empty line
        st.key(Key::ArrowUp, Modifiers::NONE, true);
        assert!(st.lost);
    }

    #[test]
    fn never_records_what_the_screen_did_not_echo() {
        let mut st = AcState::default();
        st.typed_text("hunter2");
        st.key(Key::Enter, Modifiers::NONE, false);
        assert!(st.new_commands.is_empty());
        assert!(password_like("[sudo] password for me: "));
        assert!(password_like("Enter passphrase for key '/home/me/.ssh/id':"));
        assert!(!password_like("me@host:~$ ls"));
        assert!(echoes("me@host:~$ git s", "git s"));
        assert!(!echoes("me@host:~$ ", "git s"));
    }

    #[test]
    fn ctrl_keys_and_backspace() {
        let mut st = AcState::default();
        st.typed_text("ls -la /tmp");
        st.key(Key::W, Modifiers::CTRL, true);
        assert_eq!(st.line, "ls -la ");
        st.key(Key::Backspace, Modifiers::NONE, true);
        assert_eq!(st.line, "ls -la");
        st.key(Key::C, Modifiers::CTRL, true);
        assert!(st.line.is_empty() && !st.lost);
        st.typed_text("x");
        st.key(Key::A, Modifiers::CTRL, true);
        assert!(st.lost);
    }

    #[test]
    fn reads_bash_zsh_and_fish_history_and_skips_secrets() {
        let dir = std::env::temp_dir().join(format!("tc_hist_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".local/share/fish")).unwrap();
        std::fs::write(dir.join(".bash_history"), "#1700000000\nls -la\nmysql -u root -pSECRET --password=abc\ncd /var/log\n").unwrap();
        std::fs::write(dir.join(".zsh_history"), ": 1700000001:0;git status\n: 1700000002:0;export API_TOKEN=xyz\n: 1700000003:0;docker ps\n").unwrap();
        std::fs::write(dir.join(".local/share/fish/fish_history"), "- cmd: htop\n  when: 1700000004\n- cmd: ls -la\n  when: 1700000005\n  paths:\n    - /x\n").unwrap();
        let out = std::process::Command::new("sh").arg("-c").arg(REMOTE_SCRIPT).env("HOME", &dir).output().unwrap();
        let cmds = parse_remote_history(&String::from_utf8_lossy(&out.stdout));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(cmds.contains(&"cd /var/log".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"docker ps".to_string()) && cmds.contains(&"git status".to_string()) && cmds.contains(&"htop".to_string()), "{cmds:?}");
        assert_eq!(cmds.iter().filter(|c| *c == "ls -la").count(), 1, "no repeats");
        assert!(cmds.iter().all(|c| !c.to_lowercase().contains("secret") && !c.contains("TOKEN") && !c.contains("password") && !c.starts_with('#') && !c.contains("when:")), "{cmds:?}");
        let mut h = History::default();
        h.add(7, "typed here");
        h.merge_remote(7, cmds);
        assert_eq!(h.get(7)[0], "typed here");
        assert!(h.remote_read(7) && !h.remote_read(8));
    }

    #[test]
    fn history_keeps_newest_first_without_repeats() {
        let mut h = History::default();
        h.add(3, "ls");
        h.add(3, "pwd");
        h.add(3, "ls");
        assert_eq!(h.get(3), &["ls".to_string(), "pwd".to_string()]);
        assert!(h.get(9).is_empty());
        h.clear();
        assert!(h.get(3).is_empty());
    }

    #[test]
    fn history_file_is_encrypted_with_a_key_and_never_overwritten_without_it() {
        let dir = std::env::temp_dir().join(format!("tc_hist_enc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("history.json");
        let key = crate::vaultcrypt::LogKey::random().unwrap();

        let mut h = History::load_from(&p, Some(&key));
        h.add(7, "mysql -u root");
        assert!(h.save_to(&p));
        let raw = std::fs::read(&p).unwrap();
        assert!(crate::vaultcrypt::is_sealed(&raw) && !String::from_utf8_lossy(&raw).contains("mysql"));
        assert_eq!(History::load_from(&p, Some(&key)).get(7), ["mysql -u root".to_string()]);

        // opened without the key: empty, and saving must not destroy the encrypted file
        let mut locked = History::load_from(&p, None);
        assert!(locked.get(7).is_empty());
        locked.add(7, "x");
        assert!(!locked.save_to(&p));
        assert_eq!(std::fs::read(&p).unwrap(), raw);

        // turning encryption off rewrites it readable
        let mut h = History::load_from(&p, Some(&key));
        h.set_key(None);
        assert!(h.save_to(&p));
        assert!(String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("mysql"));
        assert_eq!(History::load_from(&p, None).get(7), ["mysql -u root".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

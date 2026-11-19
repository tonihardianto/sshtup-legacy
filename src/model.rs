//! Data the app keeps: hosts, groups, keys, identities, forwarding rules, snippets and settings.
//! The data the app keeps: hosts, groups, keys, snippets and settings.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const EDITORS: [(&str, &str); 4] = [("system", "System default"), ("vscode", "Visual Studio Code"), ("sublime", "Sublime Text"), ("textedit", "TextEdit")];

pub fn editor_label(key: &str) -> &'static str {
    EDITORS.iter().find(|e| e.0 == key).map(|e| e.1).unwrap_or(EDITORS[0].1)
}

pub const DEFAULT_SSH_PORT: u16 = 22;
pub const DEFAULT_TELNET_PORT: u16 = 23;
pub const DEFAULT_KEEPALIVE_SECS: u32 = 60;
pub const DEFAULT_SCROLLBACK: usize = 40_000;
pub const DEFAULT_FONT_SIZE: f32 = 14.0;
pub const MIN_FONT_SIZE: f32 = 4.0;
pub const MAX_FONT_SIZE: f32 = 40.0;
pub const DEFAULT_SCHEME: &str = "Default Dark";
pub const DEFAULT_TERM_TYPE: &str = "xterm-256color";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Protocol {
    #[default]
    Ssh,
    Telnet,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Group {
    pub id: u64,
    pub name: String,
    pub parent: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Host {
    pub id: u64,
    pub label: String,
    pub address: String,
    pub group: Option<u64>,
    pub tags: Vec<String>,
    pub protocol: Protocol,
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub key: Option<u64>,
    pub identity: Option<u64>,
    pub agent_forwarding: bool,
    pub keep_alive: Option<u32>,
    pub startup_snippet: Option<u64>,
    pub jump_host: Option<u64>,
    pub color_scheme: Option<String>,
    pub font_size: Option<f32>,
    pub last_used: u64,
    /// Shown in the Pinned section at the top of the Hosts screen.
    #[serde(default)]
    pub pinned: bool,
}

impl Host {
    pub fn display_label(&self) -> String {
        if self.label.trim().is_empty() { self.address.clone() } else { self.label.clone() }
    }
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(match self.protocol {
            Protocol::Ssh => DEFAULT_SSH_PORT,
            Protocol::Telnet => DEFAULT_TELNET_PORT,
        })
    }
    /// The line shown under a host: protocols, user name, tags.
    pub fn description(&self, identities: &[Identity]) -> String {
        let mut parts: Vec<String> = vec![match self.protocol {
            Protocol::Ssh => "ssh".into(),
            Protocol::Telnet => "telnet".into(),
        }];
        let user = self.resolved_username(identities);
        if !user.is_empty() {
            parts.push(user);
        }
        for t in &self.tags {
            if !parts.contains(t) {
                parts.push(t.clone());
            }
        }
        parts.join(", ")
    }
    pub fn resolved_username(&self, identities: &[Identity]) -> String {
        if let Some(i) = self.identity.and_then(|id| identities.iter().find(|x| x.id == id)) {
            if !i.username.is_empty() {
                return i.username.clone();
            }
        }
        self.username.clone()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SshKey {
    pub id: u64,
    pub label: String,
    /// Path of the private key file on disk.
    pub path: String,
    pub passphrase: String,
    pub public_key: String,
    /// Send the key file itself to the other devices (off unless the person asks). The path stays per device.
    #[serde(default)]
    pub sync_contents: bool,
    /// When the key was added (seconds since 1970); 0 for keys saved before this was recorded.
    #[serde(default)]
    pub added: u64,
    /// Made by the app (otherwise it was imported or pointed at).
    #[serde(default)]
    pub generated: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Identity {
    pub id: u64,
    pub label: String,
    pub username: String,
    pub password: String,
    pub key: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PfType {
    #[default]
    Local,
    Remote,
    Dynamic,
}

impl PfType {
    pub fn name(self) -> &'static str {
        match self {
            PfType::Local => "Local Rule",
            PfType::Remote => "Remote Rule",
            PfType::Dynamic => "Dynamic Rule",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PortForward {
    pub id: u64,
    pub label: String,
    pub kind: PfType,
    pub host: Option<u64>,
    pub bound_address: String,
    pub local_port: u16,
    pub hostname: String,
    pub remote_port: u16,
    pub last_used: u64,
}

impl PortForward {
    /// A short description of the rule.
    pub fn describe(&self, host_label: &str) -> String {
        match self.kind {
            PfType::Local => format!("Local:{} → {} → {}:{}", self.local_port, host_label, self.hostname, self.remote_port),
            PfType::Remote => format!("Port {} on {} → This device → {}:{}", self.local_port, host_label, self.hostname, self.remote_port),
            PfType::Dynamic => format!("SOCKS proxy on local port {} through {}", self.local_port, host_label),
        }
    }
    pub fn display_label(&self, host_label: &str) -> String {
        if self.label.trim().is_empty() { self.describe(host_label) } else { self.label.clone() }
    }
    pub fn bound(&self) -> String {
        if self.bound_address.is_empty() { "127.0.0.1".into() } else { self.bound_address.clone() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Snippet {
    pub id: u64,
    pub label: String,
    pub script: String,
    pub package: String,
    pub last_used: u64,
}

impl Snippet {
    pub fn display_label(&self) -> String {
        if !self.label.trim().is_empty() {
            return self.label.clone();
        }
        self.script.lines().find(|l| l.chars().any(|c| c.is_alphanumeric())).unwrap_or("").to_string()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Look of the app itself: "dark", "light" or "system" (the terminal has its own `color_scheme`).
    pub app_theme: String,
    /// "inter", "dmsans", "system" or "default": the font of the app's own text.
    pub ui_font: String,
    pub color_scheme: String,
    pub font_size: f32,
    /// Terminal font by name (see `fonts.rs`); empty = the built-in Hack.
    pub font_family: String,
    /// Height of a terminal row, as a multiple of the font's own line height.
    pub line_height: f32,
    /// "block", "bar" or "underline".
    pub cursor_shape: String,
    pub cursor_blink: bool,
    pub scrollback: usize,
    /// Older setting, read only when `bell` is empty (settings saved before Off / Sound / Flash existed).
    pub bell_sound: bool,
    /// "off", "sound" or "flash"; empty = follow `bell_sound`.
    pub bell: String,
    /// Where the floating card sits on a terminal: top-right, top-left, bottom-right, bottom-left or hidden.
    pub card_position: String,
    pub option_is_meta: bool,
    pub right_click_to_paste: bool,
    pub brighten_bold_text: bool,
    pub autoreconnect: bool,
    pub keep_alive: u32,
    pub terminal_type: String,
    pub grid_layout: bool,
    pub sort_by: String,
    pub sort_reverse: bool,
    pub default_editor: String,
    pub autocomplete: bool,
    pub remote_history: bool,
    /// The welcome screen has been dealt with.
    pub onboarded: bool,
    /// What the user told the welcome screen to call them.
    pub user_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            app_theme: "dark".into(),
            ui_font: "inter".into(),
            color_scheme: DEFAULT_SCHEME.into(),
            font_size: DEFAULT_FONT_SIZE,
            font_family: String::new(),
            line_height: 1.0,
            cursor_shape: "block".into(),
            cursor_blink: true,
            scrollback: DEFAULT_SCROLLBACK,
            bell_sound: false,
            bell: String::new(),
            card_position: "top-right".into(),
            option_is_meta: false,
            right_click_to_paste: false,
            brighten_bold_text: false,
            autoreconnect: true,
            keep_alive: DEFAULT_KEEPALIVE_SECS,
            terminal_type: DEFAULT_TERM_TYPE.into(),
            grid_layout: true,
            sort_by: "Date".into(),
            sort_reverse: true,
            default_editor: "system".into(),
            autocomplete: true,
            remote_history: true,
            onboarded: false,
            user_name: String::new(),
        }
    }
}

impl Settings {
    /// What the terminal does on a bell: "off", "sound" or "flash".
    pub fn bell_mode(&self) -> &str {
        match self.bell.as_str() {
            "sound" | "flash" | "off" => &self.bell,
            _ if self.bell_sound => "sound",
            _ => "off",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Vault {
    pub next_id: u64,
    pub groups: Vec<Group>,
    pub hosts: Vec<Host>,
    pub keys: Vec<SshKey>,
    pub identities: Vec<Identity>,
    pub forwards: Vec<PortForward>,
    pub snippets: Vec<Snippet>,
    pub settings: Settings,
    #[serde(default)]
    pub team: Vec<Member>,
    /// Random key that encrypts session logs. Only ever stored inside the encrypted vault.
    #[serde(default)]
    pub log_key: Option<String>,
    /// 2 = every id is random (needed for sync); anything lower still has the old counting ids.
    #[serde(default)]
    pub id_schema: u32,
    /// Sync account on this device, when sync is on.
    #[serde(default)]
    pub sync: Option<crate::sync::SyncConfig>,
}

/// A person invited to the team. Kept on this computer only.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Member {
    pub id: u64,
    pub email: String,
    pub role: String,
    pub invited_at: u64,
}

pub const ROLES: [&str; 3] = ["Admin", "Member", "Viewer"];

impl Vault {
    /// A random id that no item in this vault uses. Random, so two devices never hand out the same one.
    pub fn new_id(&mut self) -> u64 {
        loop {
            let mut b = [0u8; 8];
            getrandom::fill(&mut b).expect("system random");
            let id = u64::from_le_bytes(b);
            if id != 0 && !self.id_in_use(id) {
                return id;
            }
        }
    }
    pub fn id_in_use(&self, id: u64) -> bool {
        self.groups.iter().any(|x| x.id == id)
            || self.hosts.iter().any(|x| x.id == id)
            || self.keys.iter().any(|x| x.id == id)
            || self.identities.iter().any(|x| x.id == id)
            || self.forwards.iter().any(|x| x.id == id)
            || self.snippets.iter().any(|x| x.id == id)
    }
    pub fn host(&self, id: Option<u64>) -> Option<&Host> {
        id.and_then(|i| self.hosts.iter().find(|h| h.id == i))
    }
    pub fn group_path(&self, mut id: Option<u64>) -> Vec<&Group> {
        let mut out = vec![];
        while let Some(g) = id.and_then(|i| self.groups.iter().find(|g| g.id == i)) {
            out.push(g);
            id = g.parent;
            if out.len() > 32 {
                break;
            }
        }
        out.reverse();
        out
    }
}

/// A host name or user name that ssh, sftp or telnet could read as a command-line option
/// (`-oProxyCommand=...`), or that holds control characters.
pub fn unsafe_conn_part(s: &str) -> bool {
    s.starts_with('-') || s.chars().any(|c| c.is_control())
}

/// Writes a file that only this user can read, from the first byte on, replacing it atomically.
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> bool {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let ok = o.open(&tmp).and_then(|mut f| f.write_all(bytes).and_then(|_| f.sync_all())).is_ok() && std::fs::rename(&tmp, path).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&tmp);
    }
    ok
}

/// ssh creates known_hosts readable by everyone; keep it to this user (it lists every host used).
pub fn tighten_known_hosts() {
    let p = data_dir().join("known_hosts");
    let _ = std::fs::create_dir_all(data_dir());
    if !p.exists() {
        let _ = write_private(&p, b"");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SSHTUP_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let new = PathBuf::from(&home).join(".sshtup");
    // an earlier build kept its data in ~/.termius_clone: move it over once
    let old = PathBuf::from(&home).join(".termius_clone");
    if !new.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &new);
    }
    new
}

/// What is on disk: a readable vault, or an encrypted one waiting for the master password.
pub enum Loaded {
    Ready(Vault),
    Locked(Vec<u8>),
}

pub fn vault_path() -> PathBuf {
    data_dir().join("vault.json")
}

pub fn load_vault() -> Loaded {
    let bytes = std::fs::read(vault_path()).unwrap_or_default();
    if crate::vaultcrypt::is_encrypted(&bytes) {
        return Loaded::Locked(bytes);
    }
    Loaded::Ready(serde_json::from_slice(&bytes).unwrap_or_default())
}

/// Writes the vault, encrypted when a key is given. Returns false when it could not be saved.
pub fn save_vault(v: &Vault, key: Option<&crate::vaultcrypt::VaultKey>) -> bool {
    let d = data_dir();
    let _ = std::fs::create_dir_all(&d);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    }
    let Ok(json) = serde_json::to_vec_pretty(v) else { return false };
    let json = zeroize::Zeroizing::new(json);
    let bytes = match key {
        Some(k) => match k.encrypt(&json) {
            Ok(b) => b,
            // never fall back to writing the plaintext when encryption was asked for
            Err(_) => return false,
        },
        None => json.to_vec(),
    };
    let tmp = d.join("vault.json.tmp");
    // created private from the start, so the contents are never readable by others, even briefly
    let written = {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&tmp).and_then(|mut f| f.write_all(&bytes).and_then(|_| f.sync_all())).is_ok()
    };
    written && std::fs::rename(&tmp, vault_path()).is_ok()
}

/// Sample content shown only in `--screenshot` runs (never written to disk).
pub fn demo_vault() -> Vault {
    let mut v = Vault::default();
    let prod = v.new_id();
    v.groups.push(Group { id: prod, name: "Production".into(), parent: None });
    let lab = v.new_id();
    v.groups.push(Group { id: lab, name: "Home lab".into(), parent: None });
    // four real key files in a temporary folder, so the Keychain page can show what it reads from them
    let kdir = std::env::temp_dir().join("sshtup-demo-keys");
    let _ = std::fs::create_dir_all(&kdir);
    let samples: [(&str, &str, &str, &str, u64, bool); 4] = [
        ("personal-laptop", include_str!("../testdata/keys/ed_enc"), include_str!("../testdata/keys/ed_enc.pub"), "personal-laptop", 1_760_227_200, true),
        ("deploy-github", include_str!("../testdata/keys/ed_plain"), include_str!("../testdata/keys/ed_plain.pub"), "deploy-github", 1_788_393_600, false),
        ("legacy-nas", include_str!("../testdata/keys/rsa_pem"), include_str!("../testdata/keys/rsa_pem.pub"), "legacy-nas", 1_780_876_800, false),
        ("backup-key", include_str!("../testdata/keys/rsa4096_enc"), include_str!("../testdata/keys/rsa4096_enc.pub"), "backup-key", 1_776_729_600, true),
    ];
    let mut key_ids = vec![];
    for (label, private, public, file, added, generated) in samples {
        let path = kdir.join(file);
        let _ = std::fs::write(&path, private);
        let id = v.new_id();
        v.keys.push(SshKey { id, label: label.into(), path: path.to_string_lossy().into_owned(), public_key: public.trim().to_string(), added, generated, ..Default::default() });
        key_ids.push(id);
    }
    let ident = v.new_id();
    v.identities.push(Identity { id: ident, label: "Deploy user".into(), username: "deploy".into(), password: String::new(), key: Some(key_ids[1]) });
    let ident2 = v.new_id();
    v.identities.push(Identity { id: ident2, label: "Home lab admin".into(), username: "admin".into(), password: "x".into(), key: None });
    let items: [(&str, &str, &str, Option<u64>, &[&str]); 5] = [
        ("web-01", "203.0.113.10", "deploy", Some(prod), &["web"]),
        ("db-primary", "203.0.113.20", "postgres", Some(prod), &["db"]),
        ("raspberry-pi", "192.168.1.42", "pi", Some(lab), &[]),
        ("nas", "192.168.1.50", "admin", Some(lab), &["storage"]),
        ("staging", "198.51.100.7", "ubuntu", None, &[]),
    ];
    for (i, (l, a, u, g, t)) in items.iter().enumerate() {
        let id = v.new_id();
        v.hosts.push(Host {
            id,
            label: l.to_string(),
            address: a.to_string(),
            group: *g,
            username: u.to_string(),
            tags: t.iter().map(|s| s.to_string()).collect(),
            last_used: 1000 - i as u64,
            key: Some(key_ids[match i { 0..=2 => 0, 3 => 2, _ => 1 }]),
            ..Default::default()
        });
    }
    let sid = v.new_id();
    v.snippets.push(Snippet { id: sid, label: "Disk usage".into(), script: "df -h".into(), ..Default::default() });
    let h = v.hosts[0].id;
    let fid = v.new_id();
    v.forwards.push(PortForward { id: fid, kind: PfType::Local, host: Some(h), bound_address: "127.0.0.1".into(), local_port: 8080, hostname: "localhost".into(), remote_port: 80, ..Default::default() });
    for (email, role) in [("maria@example.com", "Admin"), ("jonas@example.com", "Member"), ("sam@example.com", "Viewer")] {
        let id = v.new_id();
        v.team.push(Member { id, email: email.into(), role: role.into(), invited_at: now_secs() - 86_400 });
    }
    v
}

#[cfg(test)]
mod vault_file_tests {
    use super::*;
    use crate::vaultcrypt::{decrypt, VaultKey};

    #[test]
    fn encrypted_save_and_load() {
        let dir = std::env::temp_dir().join(format!("tc_vault_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: this is the only test that touches the data folder variable
        unsafe { std::env::set_var("SSHTUP_HOME", &dir) };

        let mut v = Vault::default();
        v.hosts.push(Host { id: 1, label: "web".into(), address: "10.0.0.1".into(), password: "hunter2-secret".into(), ..Default::default() });
        let key = VaultKey::with_cost("master-pass", 64, 1, 1).unwrap();
        assert!(save_vault(&v, Some(&key)));

        let raw = std::fs::read(vault_path()).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("hunter2-secret"), "password must not be readable on disk");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(vault_path()).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        }
        match load_vault() {
            Loaded::Locked(b) => {
                let (json, _) = decrypt(&b, "master-pass").unwrap();
                let back: Vault = serde_json::from_slice(&json).unwrap();
                assert_eq!(back.hosts[0].password, "hunter2-secret");
            }
            Loaded::Ready(_) => panic!("an encrypted vault must load as locked"),
        }

        // removing encryption writes readable JSON again
        assert!(save_vault(&v, None));
        assert!(matches!(load_vault(), Loaded::Ready(x) if x.hosts.len() == 1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod conn_part_tests {
    use super::unsafe_conn_part;

    #[test]
    fn options_and_control_characters_are_flagged() {
        assert!(unsafe_conn_part("-oProxyCommand=touch x"));
        assert!(unsafe_conn_part("-J"));
        assert!(unsafe_conn_part("host\nname"));
        assert!(!unsafe_conn_part("203.0.113.10"));
        assert!(!unsafe_conn_part("web-01.example.com"));
        assert!(!unsafe_conn_part("deploy"));
        assert!(!unsafe_conn_part(""));
    }
}

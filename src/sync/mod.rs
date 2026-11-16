//! Sync with a server: which account this device uses, and how the vault is kept in step.
pub mod ids;
pub mod keyfiles;
pub mod runtime;
pub mod state;
pub mod teams;
pub mod ui;

use serde::{Deserialize, Serialize};

/// Sync account on this device. Lives inside the (encrypted) vault, so these keys are never on disk in the clear.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SyncConfig {
    pub server: String,
    pub email: String,
    pub device_id: String,
    pub token: String,
    pub allow_private_http: bool,
    /// hex, 32 bytes: encrypts every item
    #[serde(default)]
    pub sync_key: String,
    /// hex, 32 bytes: this account's X25519 secret (for team keys)
    #[serde(default)]
    pub x_secret: String,
    #[serde(default)]
    pub x_public: String,
    /// teams this account is in, as last seen on the server
    #[serde(default)]
    pub teams: Vec<teams::TeamCfg>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Sync keeps its secrets inside the encrypted vault, so it refuses to start without one.
pub fn can_enable(vault_is_encrypted: bool, server: &str, allow_private_http: bool) -> Result<(), String> {
    if !vault_is_encrypted {
        return Err("Turn on the master password first (Settings → Security). Sync stores its keys inside the encrypted vault.".into());
    }
    sync_client::url::check_url(server, allow_private_http)
}

/// Before the first sync: back the vault file up and give every item a random id.
pub fn prepare_vault(v: &mut crate::model::Vault, vault_file: &std::path::Path) -> Result<usize, String> {
    if v.id_schema < 2 && vault_file.exists() && !ids::backup_file(vault_file) {
        return Err("Could not make a backup of the vault, so nothing was changed.".into());
    }
    Ok(ids::migrate_ids(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_without_encrypted_vault() {
        let e = can_enable(false, "https://sync.example.com", false).unwrap_err();
        assert!(e.contains("master password"));
        assert!(can_enable(true, "https://sync.example.com", false).is_ok());
    }

    #[test]
    fn refuses_plain_http_public_host() {
        assert!(can_enable(true, "http://sync.example.com", false).is_err());
        assert!(can_enable(true, "http://localhost:8787", false).is_ok());
    }

    #[test]
    fn enable_creates_backup_and_migrates_ids() {
        let dir = std::env::temp_dir().join(format!("tc_prep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("vault.json");
        std::fs::write(&f, b"original").unwrap();
        let mut v = crate::model::Vault::default();
        v.hosts.push(crate::model::Host { id: 1, ..Default::default() });
        assert_eq!(prepare_vault(&mut v, &f), Ok(1));
        assert_eq!(std::fs::read(dir.join("vault.json.bak")).unwrap(), b"original");
        assert!(v.hosts[0].id > 1_000_000);
        // second time: nothing to do, the backup is not overwritten
        std::fs::write(&f, b"changed").unwrap();
        assert_eq!(prepare_vault(&mut v, &f), Ok(0));
        assert_eq!(std::fs::read(dir.join("vault.json.bak")).unwrap(), b"original");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hex_round_trip() {
        let k = [0xabu8; 32];
        assert_eq!(from_hex32(&to_hex(&k)), Some(k));
        assert_eq!(from_hex32("zz"), None);
    }
}

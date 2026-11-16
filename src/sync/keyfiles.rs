//! Private key files that the person chose to sync. Everything here is off unless `SshKey.sync_contents` is set.

use crate::model::{data_dir, write_private, Vault};
use base64::Engine as _;
use std::path::{Path, PathBuf};

/// A key file is a few KB; refuse anything big instead of syncing the wrong file.
const MAX_KEY_FILE: usize = 64 * 1024;

pub fn expand(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest),
        None => p.to_string(),
    }
}

pub fn keys_dir() -> PathBuf {
    data_dir().join("keys")
}

/// {"contents": "<base64>"} for a readable, small file; None otherwise.
pub fn export_json(path: &Path) -> Option<Vec<u8>> {
    let bytes = zeroize::Zeroizing::new(std::fs::read(path).ok()?);
    if bytes.is_empty() || bytes.len() > MAX_KEY_FILE {
        return None;
    }
    serde_json::to_vec(&serde_json::json!({ "contents": base64::engine::general_purpose::STANDARD.encode(&*bytes) })).ok()
}

/// Writes the key file for `id` under `dir`, readable only by the owner.
pub fn import_key_in(dir: &Path, id: u64, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let p = dir.join(format!("{id:016x}"));
    if write_private(&p, bytes) {
        Ok(p)
    } else {
        Err(std::io::Error::other("could not write the key file"))
    }
}

fn file_for(id: u64) -> PathBuf {
    keys_dir().join(format!("{id:016x}"))
}

/// A key file arrived (Some) or was switched off on another device (None).
pub fn apply_keyfile(v: &mut Vault, id: u64, json: Option<&[u8]>) {
    match json {
        Some(j) => {
            let bytes = serde_json::from_slice::<serde_json::Value>(j)
                .ok()
                .and_then(|x| x["contents"].as_str().and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok()))
                .filter(|b| !b.is_empty() && b.len() <= MAX_KEY_FILE);
            if let Some(b) = bytes {
                let _ = import_key_in(&keys_dir(), id, &b);
            }
        }
        None => {
            let f = file_for(id);
            let _ = std::fs::remove_file(&f);
            if let Some(k) = v.keys.iter_mut().find(|k| k.id == id) {
                if Path::new(&expand(&k.path)) == f {
                    k.path.clear();
                }
            }
        }
    }
    fix_paths(v);
}

/// A synced key whose file we received but whose path is empty or missing points at our copy.
pub fn fix_paths(v: &mut Vault) {
    for k in v.keys.iter_mut().filter(|k| k.sync_contents) {
        let f = file_for(k.id);
        if f.exists() && (k.path.trim().is_empty() || !Path::new(&expand(&k.path)).exists()) {
            k.path = f.to_string_lossy().into_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SshKey;
    use crate::sync::state::{item_id, SyncState, VaultStore};
    use sync_client::engine::Store;

    fn vault_with_key(path: &str, sync: bool) -> Vault {
        let mut v = Vault::default();
        v.keys.push(SshKey { id: 9, label: "k".into(), path: path.into(), sync_contents: sync, ..Default::default() });
        v
    }
    fn snap(v: &mut Vault, s: &mut SyncState, now: u64) -> Vec<sync_client::engine::LocalItem> {
        VaultStore { vault: v, state: s, device: "d".into() }.snapshot(now)
    }

    #[test]
    fn default_does_not_sync_contents() {
        let f = std::env::temp_dir().join(format!("tc_kf1_{}", std::process::id()));
        std::fs::write(&f, b"PRIVATE").unwrap();
        let mut v = vault_with_key(f.to_str().unwrap(), false);
        let items = snap(&mut v, &mut SyncState::default(), 1);
        assert!(items.iter().all(|i| !i.id.starts_with("keyfile:")));
        assert!(items.iter().all(|i| !String::from_utf8_lossy(i.json.as_deref().unwrap_or(&[])).contains("\"contents\"")));
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn key_path_is_not_part_of_what_syncs() {
        let mut v = vault_with_key("/home/me/.ssh/id", false);
        let items = snap(&mut v, &mut SyncState::default(), 1);
        let k = items.iter().find(|i| i.id == item_id("key", 9)).unwrap();
        assert!(!String::from_utf8_lossy(k.json.as_ref().unwrap()).contains("/home/me"));
        // changing the path on this device is not an edit to sync
        let mut s = SyncState::default();
        let a = snap(&mut v, &mut s, 1);
        let done = a.iter().map(|i| (i.id.clone(), i.stamp.clone())).collect();
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.mark_synced("u", done);
        v.keys[0].path = "/elsewhere".into();
        assert!(!snap(&mut v, &mut s, 9).iter().any(|i| i.dirty));
    }

    #[test]
    fn toggled_on_sends_contents_then_toggle_off_sends_tombstone() {
        let f = std::env::temp_dir().join(format!("tc_kf2_{}", std::process::id()));
        std::fs::write(&f, b"PRIVATE").unwrap();
        let mut v = vault_with_key(f.to_str().unwrap(), true);
        let mut s = SyncState::default();
        let items = snap(&mut v, &mut s, 1);
        let kf = items.iter().find(|i| i.id == item_id("keyfile", 9)).expect("keyfile item");
        assert!(kf.json.is_some() && kf.dirty);
        let done = items.iter().map(|i| (i.id.clone(), i.stamp.clone())).collect();
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.mark_synced("u", done);
        v.keys[0].sync_contents = false;
        let items = snap(&mut v, &mut s, 5);
        let kf = items.iter().find(|i| i.id == item_id("keyfile", 9)).unwrap();
        assert!(kf.stamp.deleted && kf.json.is_none() && kf.dirty);
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn import_writes_0600() {
        let d = std::env::temp_dir().join(format!("tc_kf3_{}", std::process::id()));
        let p = import_key_in(&d, 0xabc, b"secret").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"secret");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(p.parent().unwrap(), d);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn export_refuses_big_or_missing_files() {
        assert!(export_json(Path::new("/definitely/not/here")).is_none());
        let f = std::env::temp_dir().join(format!("tc_kf4_{}", std::process::id()));
        std::fs::write(&f, vec![1u8; MAX_KEY_FILE + 1]).unwrap();
        assert!(export_json(&f).is_none());
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn apply_ignores_an_id_that_is_not_a_plain_number() {
        let mut v = Vault::default();
        let it = sync_client::engine::LocalItem { id: "keyfile:../../etc/passwd".into(), scope: "u".into(), stamp: sync_core::merge::Stamp { updated_at: 1, deleted: false, device: "x".into() }, json: Some(b"{\"contents\":\"QQ==\"}".to_vec()), dirty: false };
        VaultStore { vault: &mut v, state: &mut SyncState::default(), device: "d".into() }.apply(vec![it], 1);
        assert!(v.keys.is_empty());
    }
}

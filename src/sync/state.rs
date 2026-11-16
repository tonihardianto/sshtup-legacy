//! What this device remembers about sync: the last state of every item (to notice edits and
//! deletions without touching the rest of the app), the server position, and replaced local copies.

use crate::model::*;
use crate::vaultcrypt::{self, LogKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use sync_client::engine::{LocalItem, Store};
use sync_core::merge::Stamp;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Tracked {
    /// sha256 of the item's JSON when it was last seen ("" for a deleted item)
    pub hash: String,
    pub updated_at: u64,
    pub device: String,
    pub scope: String,
    pub deleted: bool,
    /// the server has exactly this version
    pub synced: bool,
}

/// A local version that lost against a newer one from another device.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Replaced {
    pub id: String,
    pub at: u64,
    pub json: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SyncState {
    pub items: HashMap<String, Tracked>,
    pub cursors: HashMap<String, u64>,
    pub replaced: Vec<Replaced>,
    /// Tombstones for the scope an item moved out of ("<scope>|<item id>"), until the server has them.
    #[serde(default)]
    pub leaving: HashMap<String, Tracked>,
    /// Live items that arrived from a team scope for an id this device holds in another scope. They wait for the
    /// old scope's deletion (written by the same person) before they may replace anything.
    #[serde(default)]
    pub parked: HashMap<String, Parked>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Parked {
    pub scope: String,
    pub updated_at: u64,
    pub device: String,
    /// the item's JSON text
    pub json: String,
}

const MAX_PARKED: usize = 200;

const MAX_REPLACED: usize = 50;

pub fn state_path() -> PathBuf {
    data_dir().join("sync_state.bin")
}

impl SyncState {
    pub fn load(key: &LogKey) -> Self {
        Self::load_from(&state_path(), key)
    }
    pub fn load_from(path: &Path, key: &LogKey) -> Self {
        let bytes = std::fs::read(path).unwrap_or_default();
        vaultcrypt::unseal(key, "sync-state", &bytes).ok().and_then(|p| serde_json::from_slice(&p).ok()).unwrap_or_default()
    }
    pub fn save(&self, key: &LogKey) -> bool {
        self.save_to(&state_path(), key)
    }
    pub fn save_to(&self, path: &Path, key: &LogKey) -> bool {
        let Ok(json) = serde_json::to_vec(self) else { return false };
        let json = zeroize::Zeroizing::new(json);
        match vaultcrypt::seal(key, "sync-state", &json) {
            Ok(b) => write_private(path, &b),
            Err(_) => false,
        }
    }
    pub fn remember_replaced(&mut self, items: &[LocalItem], now: u64) {
        for i in items {
            if let Some(j) = &i.json {
                self.replaced.push(Replaced { id: i.id.clone(), at: now, json: String::from_utf8_lossy(j).into_owned() });
            }
        }
        let n = self.replaced.len();
        if n > MAX_REPLACED {
            self.replaced.drain(..n - MAX_REPLACED);
        }
    }
}

pub(super) fn hash_of(bytes: &[u8]) -> String {
    hash(bytes)
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn item_id(kind: &str, id: u64) -> String {
    format!("{kind}:{id:016x}")
}

fn split_id(s: &str) -> Option<(&str, u64)> {
    let (k, h) = s.split_once(':')?;
    Some((k, u64::from_str_radix(h, 16).ok()?))
}

/// Every syncable item of the vault as (id string, JSON bytes).
pub(super) fn current(v: &Vault) -> Vec<(String, Vec<u8>)> {
    fn all<T: Serialize>(kind: &str, list: &[T], id: fn(&T) -> u64, out: &mut Vec<(String, Vec<u8>)>) {
        for x in list {
            if let Ok(j) = serde_json::to_vec(x) {
                out.push((item_id(kind, id(x)), j));
            }
        }
    }
    let mut out = Vec::new();
    all("group", &v.groups, |x| x.id, &mut out);
    all("host", &v.hosts, |x| x.id, &mut out);
    // a key's path belongs to the device, so it is left out of what is synced
    let keys: Vec<SshKey> = v.keys.iter().cloned().map(|mut k| {
        k.path.clear();
        k
    }).collect();
    all("key", &keys, |x| x.id, &mut out);
    for k in v.keys.iter().filter(|k| k.sync_contents && !k.path.trim().is_empty()) {
        if let Some(j) = super::keyfiles::export_json(std::path::Path::new(&super::keyfiles::expand(&k.path))) {
            out.push((item_id("keyfile", k.id), j));
        }
    }
    all("identity", &v.identities, |x| x.id, &mut out);
    all("forward", &v.forwards, |x| x.id, &mut out);
    all("snippet", &v.snippets, |x| x.id, &mut out);
    out
}

fn current_one(v: &Vault, id: &str) -> Vec<u8> {
    current(v).into_iter().find(|(i, _)| i == id).map(|(_, j)| j).unwrap_or_default()
}

fn put<T: serde::de::DeserializeOwned>(list: &mut Vec<T>, id: fn(&T) -> u64, want: u64, bytes: &[u8]) -> bool {
    let Ok(x) = serde_json::from_slice::<T>(bytes) else { return false };
    if id(&x) != want {
        return false; // the content does not match the id it claims: ignore it
    }
    match list.iter().position(|e| id(e) == want) {
        Some(i) => list[i] = x,
        None => list.push(x),
    }
    true
}

fn drop_item<T>(list: &mut Vec<T>, id: fn(&T) -> u64, want: u64) {
    list.retain(|e| id(e) != want);
}

/// The vault seen as a set of synced items.
pub struct VaultStore<'a> {
    pub vault: &'a mut Vault,
    pub state: &'a mut SyncState,
    pub device: String,
}

impl Store for VaultStore<'_> {
    fn snapshot(&mut self, now: u64) -> Vec<LocalItem> {
        let mut seen = std::collections::HashSet::new();
        for (id, json) in current(self.vault) {
            seen.insert(id.clone());
            let h = hash(&json);
            let t = self.state.items.entry(id).or_default();
            if t.hash != h || t.deleted {
                *t = Tracked { hash: h, updated_at: now, device: self.device.clone(), scope: if t.scope.is_empty() { "u".into() } else { t.scope.clone() }, deleted: false, synced: false };
            }
        }
        for (id, t) in self.state.items.iter_mut() {
            if !seen.contains(id) && !t.deleted {
                *t = Tracked { hash: String::new(), updated_at: now, device: self.device.clone(), scope: t.scope.clone(), deleted: true, synced: false };
            }
        }
        let json: HashMap<String, Vec<u8>> = current(self.vault).into_iter().collect();
        let leaving = self.state.leaving.iter().filter(|(_, t)| !t.synced).filter_map(|(k, t)| {
            let (scope, id) = k.split_once('|')?;
            Some(LocalItem { id: id.to_string(), scope: scope.to_string(), stamp: Stamp { updated_at: t.updated_at, deleted: true, device: t.device.clone() }, json: None, dirty: true })
        });
        let leaving: Vec<LocalItem> = leaving.collect();
        self.state
            .items
            .iter()
            .map(|(id, t)| LocalItem {
                id: id.clone(),
                scope: t.scope.clone(),
                stamp: Stamp { updated_at: t.updated_at, deleted: t.deleted, device: t.device.clone() },
                json: if t.deleted { None } else { json.get(id).cloned() },
                dirty: !t.synced,
            })
            .chain(leaving)
            .collect()
    }

    fn apply(&mut self, items: Vec<LocalItem>, _now: u64) {
        let mut queue: std::collections::VecDeque<LocalItem> = items.into();
        while let Some(it) = queue.pop_front() {
            let Some((kind, id)) = split_id(&it.id) else { continue };
            // A team member can write any id into the team's scope, including the id of one of my personal items.
            // Only my own deletion in the old scope proves that the item really moved, so until it arrives the
            // newcomer waits. Items coming from my personal scope are trusted: nobody else can write there.
            let held_elsewhere = self.state.items.get(&it.id).map(|t| !t.deleted && !t.scope.is_empty() && t.scope != it.scope).unwrap_or(false);
            if it.json.is_some() && it.scope != "u" && held_elsewhere {
                if self.state.parked.len() < MAX_PARKED || self.state.parked.contains_key(&it.id) {
                    let json = String::from_utf8_lossy(it.json.as_deref().unwrap_or_default()).into_owned();
                    self.state.parked.insert(it.id.clone(), Parked { scope: it.scope.clone(), updated_at: it.stamp.updated_at, device: it.stamp.device.clone(), json });
                }
                continue;
            }
            let was_live_in = self.state.items.get(&it.id).filter(|t| !t.deleted).map(|t| t.scope.clone());
            // a deletion from the scope this item moved out of is not a deletion of the item
            if it.json.is_none() && self.state.items.get(&it.id).map(|t| !t.deleted && !t.scope.is_empty() && t.scope != it.scope).unwrap_or(false) {
                continue;
            }
            if self.state.leaving.get(&format!("{}|{}", it.scope, it.id)).map(|l| !l.synced).unwrap_or(false) && it.json.is_some() {
                // we are in the middle of moving it out of that scope; our move wins
                continue;
            }
            // the item was edited here after the snapshot was taken: keep the edit, the next round settles it
            let here = current(self.vault).into_iter().find(|(i, _)| *i == it.id).map(|(_, j)| hash(&j));
            let seen = self.state.items.get(&it.id).map(|t| (t.hash.clone(), t.deleted));
            let edited_since = match (&here, &seen) {
                (Some(h), Some((th, false))) => h != th,
                (Some(_), Some((_, true))) | (Some(_), None) => true,
                (None, Some((_, false))) => true,
                _ => false,
            };
            if edited_since {
                continue;
            }
            let v = &mut *self.vault;
            if kind == "keyfile" {
                super::keyfiles::apply_keyfile(v, id, it.json.as_deref());
                let h = it.json.as_ref().map(|_| hash(&current_one(v, &it.id))).unwrap_or_default();
                self.state.items.insert(it.id, Tracked { hash: h, updated_at: it.stamp.updated_at, device: it.stamp.device, scope: it.scope, deleted: it.json.is_none(), synced: true });
                continue;
            }
            // keep this device's own path when a key arrives
            let local_path = v.keys.iter().find(|k| k.id == id).map(|k| k.path.clone());
            let ok = match (&it.json, kind) {
                (None, "group") => {
                    drop_item(&mut v.groups, |x| x.id, id);
                    true
                }
                (None, "host") => {
                    drop_item(&mut v.hosts, |x| x.id, id);
                    true
                }
                (None, "key") => {
                    drop_item(&mut v.keys, |x| x.id, id);
                    true
                }
                (None, "identity") => {
                    drop_item(&mut v.identities, |x| x.id, id);
                    true
                }
                (None, "forward") => {
                    drop_item(&mut v.forwards, |x| x.id, id);
                    true
                }
                (None, "snippet") => {
                    drop_item(&mut v.snippets, |x| x.id, id);
                    true
                }
                (Some(j), "group") => put(&mut v.groups, |x| x.id, id, j),
                (Some(j), "host") => put(&mut v.hosts, |x| x.id, id, j),
                (Some(j), "key") => {
                    let done = put(&mut v.keys, |x| x.id, id, j);
                    if let Some(k) = v.keys.iter_mut().find(|k| k.id == id) {
                        k.path = local_path.unwrap_or_default();
                    }
                    super::keyfiles::fix_paths(v);
                    done
                }
                (Some(j), "identity") => put(&mut v.identities, |x| x.id, id, j),
                (Some(j), "forward") => put(&mut v.forwards, |x| x.id, id, j),
                (Some(j), "snippet") => put(&mut v.snippets, |x| x.id, id, j),
                _ => false,
            };
            if !ok {
                continue;
            }
            // what we store is what the vault would produce itself, so it does not look edited next time
            let h = if it.json.is_some() { current(self.vault).into_iter().find(|(i, _)| *i == it.id).map(|(_, j)| hash(&j)).unwrap_or_default() } else { String::new() };
            let (id_text, scope_text, deleted) = (it.id.clone(), it.scope.clone(), it.json.is_none());
            self.state.items.insert(it.id, Tracked { hash: h, updated_at: it.stamp.updated_at, device: it.stamp.device, scope: it.scope, deleted, synced: true });
            // the old scope has now deleted the item: a newcomer that was waiting for that may come in
            if deleted && was_live_in.as_deref() == Some(scope_text.as_str()) {
                if let Some(p) = self.state.parked.remove(&id_text) {
                    queue.push_front(LocalItem { id: id_text, scope: p.scope, stamp: Stamp { updated_at: p.updated_at, deleted: false, device: p.device }, json: Some(p.json.into_bytes()), dirty: false });
                }
            }
        }
    }

    fn mark_synced(&mut self, scope: &str, done: Vec<(String, Stamp)>) {
        let json: HashMap<String, Vec<u8>> = current(self.vault).into_iter().collect();
        for (id, st) in done {
            if let Some(l) = self.state.leaving.get_mut(&format!("{scope}|{id}")) {
                if l.updated_at <= st.updated_at {
                    l.synced = true;
                }
                if self.state.items.get(&id).map(|t| t.scope != scope).unwrap_or(true) {
                    continue;
                }
            }
            let Some(t) = self.state.items.get_mut(&id) else { continue };
            if t.scope != scope && !t.scope.is_empty() {
                continue;
            }
            // an edit made after the snapshot stays unsynced
            let unchanged = if t.deleted { !json.contains_key(&id) } else { json.get(&id).map(|j| hash(j) == t.hash).unwrap_or(false) };
            if unchanged && t.updated_at >= st.updated_at {
                t.updated_at = st.updated_at;
                t.device = st.device;
                t.synced = true;
            }
        }
        self.state.leaving.retain(|_, l| !l.synced);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        let mut v = Vault::default();
        v.id_schema = 2;
        v.hosts.push(Host { id: 11, label: "web".into(), password: "hunter2-secret".into(), ..Default::default() });
        v.snippets.push(Snippet { id: 12, label: "ls".into(), script: "ls".into(), ..Default::default() });
        v
    }
    fn store<'a>(v: &'a mut Vault, s: &'a mut SyncState) -> VaultStore<'a> {
        VaultStore { vault: v, state: s, device: "dev".into() }
    }
    fn find(items: &[LocalItem], id: &str) -> LocalItem {
        items.iter().find(|i| i.id == id).cloned().unwrap()
    }

    #[test]
    fn unchanged_item_keeps_old_updated_at() {
        let (mut v, mut s) = (vault(), SyncState::default());
        let a = find(&store(&mut v, &mut s).snapshot(100), &item_id("host", 11));
        let b = find(&store(&mut v, &mut s).snapshot(900), &item_id("host", 11));
        assert_eq!((a.stamp.updated_at, b.stamp.updated_at), (100, 100));
    }

    #[test]
    fn edited_item_gets_new_updated_at() {
        let (mut v, mut s) = (vault(), SyncState::default());
        let id = item_id("host", 11);
        let snap = store(&mut v, &mut s).snapshot(100);
        let done: Vec<_> = snap.iter().map(|i| (i.id.clone(), i.stamp.clone())).collect();
        store(&mut v, &mut s).mark_synced("u", done);
        assert!(!find(&store(&mut v, &mut s).snapshot(150), &id).dirty);
        v.hosts[0].label = "web2".into();
        let it = find(&store(&mut v, &mut s).snapshot(200), &id);
        assert_eq!((it.stamp.updated_at, it.dirty), (200, true));
    }

    #[test]
    fn removed_item_becomes_tombstone() {
        let (mut v, mut s) = (vault(), SyncState::default());
        store(&mut v, &mut s).snapshot(100);
        v.snippets.clear();
        let it = find(&store(&mut v, &mut s).snapshot(300), &item_id("snippet", 12));
        assert!(it.stamp.deleted && it.json.is_none() && it.dirty);
        assert_eq!(it.stamp.updated_at, 300);
    }

    #[test]
    fn apply_adds_edits_and_removes() {
        let (mut v, mut s) = (vault(), SyncState::default());
        store(&mut v, &mut s).snapshot(100);
        let newhost = Host { id: 77, label: "new".into(), ..Default::default() };
        let mut edited = v.hosts[0].clone();
        edited.label = "renamed".into();
        let mk = |id: String, json: Option<Vec<u8>>| LocalItem { id, scope: "u".into(), stamp: Stamp { updated_at: 500, deleted: json.is_none(), device: "other".into() }, json, dirty: false };
        store(&mut v, &mut s).apply(
            vec![
                mk(item_id("host", 77), Some(serde_json::to_vec(&newhost).unwrap())),
                mk(item_id("host", 11), Some(serde_json::to_vec(&edited).unwrap())),
                mk(item_id("snippet", 12), None),
            ],
            600,
        );
        assert!(v.hosts.iter().any(|h| h.id == 77));
        assert_eq!(v.hosts.iter().find(|h| h.id == 11).unwrap().label, "renamed");
        assert!(v.snippets.is_empty());
        // applied items do not look edited afterwards
        let snap = store(&mut v, &mut s).snapshot(900);
        assert!(snap.iter().all(|i| !i.dirty || i.id == item_id("key", 0)), "{:?}", snap.iter().filter(|i| i.dirty).map(|i| &i.id).collect::<Vec<_>>());
    }

    #[test]
    fn apply_ignores_content_that_does_not_match_its_id() {
        let (mut v, mut s) = (vault(), SyncState::default());
        let liar = Host { id: 5, ..Default::default() };
        let it = LocalItem { id: item_id("host", 6), scope: "u".into(), stamp: Stamp { updated_at: 1, deleted: false, device: "x".into() }, json: Some(serde_json::to_vec(&liar).unwrap()), dirty: false };
        store(&mut v, &mut s).apply(vec![it, LocalItem { id: "nonsense".into(), scope: "u".into(), stamp: Stamp { updated_at: 1, deleted: false, device: "x".into() }, json: Some(b"{".to_vec()), dirty: false }], 2);
        assert_eq!(v.hosts.len(), 1);
    }

    #[test]
    fn apply_skips_items_edited_since_snapshot() {
        let (mut v, mut s) = (vault(), SyncState::default());
        store(&mut v, &mut s).snapshot(100);
        v.hosts[0].label = "typed a moment ago".into();
        let mut remote = v.hosts[0].clone();
        remote.label = "from another device".into();
        let it = LocalItem { id: item_id("host", 11), scope: "u".into(), stamp: Stamp { updated_at: 500, deleted: false, device: "o".into() }, json: Some(serde_json::to_vec(&remote).unwrap()), dirty: false };
        let gone = LocalItem { id: item_id("snippet", 12), scope: "u".into(), stamp: Stamp { updated_at: 500, deleted: true, device: "o".into() }, json: None, dirty: false };
        v.snippets[0].label = "also edited".into();
        store(&mut v, &mut s).apply(vec![it, gone], 600);
        assert_eq!(v.hosts[0].label, "typed a moment ago");
        assert_eq!(v.snippets.len(), 1);
    }

    #[test]
    fn edit_after_snapshot_stays_unsynced() {
        let (mut v, mut s) = (vault(), SyncState::default());
        let snap = store(&mut v, &mut s).snapshot(100);
        v.hosts[0].label = "changed while the network was busy".into();
        let done: Vec<_> = snap.iter().map(|i| (i.id.clone(), i.stamp.clone())).collect();
        store(&mut v, &mut s).mark_synced("u", done);
        assert!(find(&store(&mut v, &mut s).snapshot(200), &item_id("host", 11)).dirty);
    }

    #[test]
    fn state_round_trip_encrypted() {
        let key = LogKey::random().unwrap();
        let (mut v, mut s) = (vault(), SyncState::default());
        store(&mut v, &mut s).snapshot(100);
        s.cursors.insert("u".into(), 42);
        let p = std::env::temp_dir().join(format!("tc_state_{}.bin", std::process::id()));
        assert!(s.save_to(&p, &key));
        let back = SyncState::load_from(&p, &key);
        assert_eq!(back.cursors["u"], 42);
        assert_eq!(back.items.len(), s.items.len());
        let other = SyncState::load_from(&p, &LogKey::random().unwrap());
        assert!(other.items.is_empty(), "wrong key must not read the state");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn secrets_not_in_state_file_plaintext() {
        let key = LogKey::random().unwrap();
        let (mut v, mut s) = (vault(), SyncState::default());
        let snap = store(&mut v, &mut s).snapshot(100);
        s.remember_replaced(&snap, 5); // replaced copies hold the full item, password included
        let p = std::env::temp_dir().join(format!("tc_state2_{}.bin", std::process::id()));
        assert!(s.save_to(&p, &key));
        let raw = std::fs::read(&p).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("hunter2-secret"));
        assert!(!String::from_utf8_lossy(&raw).contains("password"));
        let _ = std::fs::remove_file(p);
    }
}

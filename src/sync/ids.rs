//! Old vaults count their ids 1, 2, 3...; two devices would hand out the same ones. This gives every item a random id.

use crate::model::Vault;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Gives every item a random id and rewrites everything that points at it.
/// A pointer to something that no longer exists becomes empty.
/// Returns how many ids changed (0 when the vault was already migrated).
pub fn migrate_ids(v: &mut Vault) -> usize {
    if v.id_schema >= 2 {
        return 0;
    }
    let mut taken: HashSet<u64> = HashSet::new();
    let mut fresh = || loop {
        let mut b = [0u8; 8];
        getrandom::fill(&mut b).expect("system random");
        let id = u64::from_le_bytes(b);
        if id > 1_000_000 && taken.insert(id) {
            return id;
        }
    };
    fn renumber<T>(items: &mut [T], id: fn(&mut T) -> &mut u64, fresh: &mut impl FnMut() -> u64) -> HashMap<u64, u64> {
        items
            .iter_mut()
            .map(|x| {
                let slot = id(x);
                let new = fresh();
                let old = std::mem::replace(slot, new);
                (old, new)
            })
            .collect()
    }
    fn remap(r: &mut Option<u64>, m: &HashMap<u64, u64>) {
        *r = r.and_then(|o| m.get(&o).copied());
    }
    let groups = renumber(&mut v.groups, |x| &mut x.id, &mut fresh);
    let hosts = renumber(&mut v.hosts, |x| &mut x.id, &mut fresh);
    let keys = renumber(&mut v.keys, |x| &mut x.id, &mut fresh);
    let idents = renumber(&mut v.identities, |x| &mut x.id, &mut fresh);
    let snippets = renumber(&mut v.snippets, |x| &mut x.id, &mut fresh);
    let forwards = renumber(&mut v.forwards, |x| &mut x.id, &mut fresh);
    for g in &mut v.groups {
        remap(&mut g.parent, &groups);
    }
    for h in &mut v.hosts {
        remap(&mut h.group, &groups);
        remap(&mut h.key, &keys);
        remap(&mut h.identity, &idents);
        remap(&mut h.startup_snippet, &snippets);
        remap(&mut h.jump_host, &hosts);
    }
    for i in &mut v.identities {
        remap(&mut i.key, &keys);
    }
    for f in &mut v.forwards {
        remap(&mut f.host, &hosts);
    }
    v.id_schema = 2;
    groups.len() + hosts.len() + keys.len() + idents.len() + snippets.len() + forwards.len()
}

/// Copies `path` to `<path>.bak`, readable only by the owner. Returns false when it could not.
pub fn backup_file(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false };
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    use std::io::Write;
    o.open(&bak).and_then(|mut f| f.write_all(&bytes)).is_ok()
}

pub fn backup_before_migration() -> bool {
    backup_file(&crate::model::vault_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::collections::HashSet;

    fn old_vault() -> Vault {
        let mut v = Vault::default();
        v.groups = vec![Group { id: 1, name: "g1".into(), parent: None }, Group { id: 2, name: "g2".into(), parent: Some(1) }];
        v.keys = vec![SshKey { id: 3, label: "k".into(), ..Default::default() }];
        v.identities = vec![Identity { id: 4, label: "i".into(), key: Some(3), ..Default::default() }];
        v.snippets = vec![Snippet { id: 5, label: "s".into(), ..Default::default() }];
        v.hosts = vec![
            Host { id: 6, label: "jump".into(), group: Some(2), ..Default::default() },
            Host { id: 7, label: "web".into(), group: Some(1), key: Some(3), identity: Some(4), startup_snippet: Some(5), jump_host: Some(6), ..Default::default() },
        ];
        v.forwards = vec![PortForward { id: 8, host: Some(7), ..Default::default() }];
        v.next_id = 8;
        v
    }

    fn all_ids(v: &Vault) -> Vec<u64> {
        let mut o: Vec<u64> = v.groups.iter().map(|x| x.id).collect();
        o.extend(v.hosts.iter().map(|x| x.id));
        o.extend(v.keys.iter().map(|x| x.id));
        o.extend(v.identities.iter().map(|x| x.id));
        o.extend(v.forwards.iter().map(|x| x.id));
        o.extend(v.snippets.iter().map(|x| x.id));
        o
    }

    #[test]
    fn all_ids_become_unique_random() {
        let mut v = old_vault();
        assert_eq!(migrate_ids(&mut v), 8);
        let ids = all_ids(&v);
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 8);
        assert!(ids.iter().all(|i| *i > 1000), "ids still look like counters: {ids:?}");
        assert_eq!(v.id_schema, 2);
    }

    #[test]
    fn references_follow() {
        let mut v = old_vault();
        migrate_ids(&mut v);
        let web = v.hosts.iter().find(|h| h.label == "web").unwrap();
        let jump = v.hosts.iter().find(|h| h.label == "jump").unwrap();
        assert_eq!(web.jump_host, Some(jump.id));
        assert_eq!(web.key, Some(v.keys[0].id));
        assert_eq!(web.identity, Some(v.identities[0].id));
        assert_eq!(web.startup_snippet, Some(v.snippets[0].id));
        assert_eq!(web.group, Some(v.groups[0].id));
        assert_eq!(jump.group, Some(v.groups[1].id));
        assert_eq!(v.groups[1].parent, Some(v.groups[0].id));
        assert_eq!(v.identities[0].key, Some(v.keys[0].id));
        assert_eq!(v.forwards[0].host, Some(web.id));
    }

    #[test]
    fn dangling_reference_becomes_none_not_panic() {
        let mut v = old_vault();
        v.hosts[1].jump_host = Some(999);
        v.forwards[0].host = Some(998);
        migrate_ids(&mut v);
        assert_eq!(v.hosts[1].jump_host, None);
        assert_eq!(v.forwards[0].host, None);
    }

    #[test]
    fn idempotent() {
        let mut v = old_vault();
        migrate_ids(&mut v);
        let before = all_ids(&v);
        assert_eq!(migrate_ids(&mut v), 0);
        assert_eq!(all_ids(&v), before);
    }

    #[test]
    fn new_id_never_collides_with_existing() {
        let mut v = old_vault();
        let mut seen: HashSet<u64> = all_ids(&v).into_iter().collect();
        for _ in 0..10_000 {
            let id = v.new_id();
            assert!(seen.insert(id), "collision on {id}");
            v.snippets.push(Snippet { id, ..Default::default() });
        }
    }

    #[test]
    fn backup_copies_the_file_privately() {
        let dir = std::env::temp_dir().join(format!("tc_ids_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("vault.json");
        std::fs::write(&src, b"hello").unwrap();
        assert!(backup_file(&src));
        let bak = dir.join("vault.json.bak");
        assert_eq!(std::fs::read(&bak).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&bak).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

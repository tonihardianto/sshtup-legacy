//! Team scopes on this device: which items belong to which team, moving items between "mine" and a team,
//! and what happens when this account is no longer in a team.

use super::state::{current, item_id, SyncState};
use crate::model::Vault;
use serde::{Deserialize, Serialize};

/// A team this device syncs, as last seen on the server. Lives in the encrypted vault.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TeamCfg {
    pub id: i64,
    pub name: String,
    pub epoch: u32,
    pub owner: bool,
    /// owner, admin, editor or viewer; empty in a vault saved before roles (an editor)
    #[serde(default)]
    pub role: String,
    /// hex, 32 bytes
    pub key: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MoveErr {
    NotFound,
    Same,
    /// Moving this puts a secret in front of every team member. Say so and ask again with `confirmed`.
    NeedsConfirm(String),
}

pub fn scope_of(team: i64) -> String {
    format!("t:{team}")
}

/// What the person must agree to before the item moves, if anything.
pub fn warning_for(vault: &Vault, id: &str) -> Option<String> {
    let (kind, n) = id.split_once(':')?;
    let n = u64::from_str_radix(n, 16).ok()?;
    match kind {
        "identity" => vault.identities.iter().find(|x| x.id == n).filter(|x| !x.password.is_empty() || x.key.is_some()).map(|x| format!("Every team member will be able to read the password and key of the identity \"{}\".", x.label)),
        "host" => vault.hosts.iter().find(|x| x.id == n).filter(|x| !x.password.is_empty()).map(|x| format!("Every team member will be able to read the saved password of \"{}\".", x.label)),
        "key" => vault.keys.iter().find(|x| x.id == n).filter(|x| !x.passphrase.is_empty() || x.sync_contents).map(|x| format!("Every team member will be able to read the passphrase{} of the key \"{}\".", if x.sync_contents { " and the key file" } else { "" }, x.label)),
        _ => None,
    }
}

/// Puts one item in another scope ("u" for personal, "t:<id>" for a team). The old scope gets a tombstone,
/// the new scope gets the item sealed with that scope's key on the next round.
pub fn move_to_scope(state: &mut SyncState, vault: &Vault, id: &str, scope: &str, confirmed: bool, device: &str, now: u64) -> Result<(), MoveErr> {
    let here = current(vault).into_iter().find(|(i, _)| i == id).ok_or(MoveErr::NotFound)?;
    let old = state.items.get(id).map(|t| t.scope.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "u".into());
    if old == scope {
        return Err(MoveErr::Same);
    }
    if scope != "u" && !confirmed {
        if let Some(w) = warning_for(vault, id) {
            return Err(MoveErr::NeedsConfirm(w));
        }
    }
    let mut t = state.items.get(id).cloned().unwrap_or_default();
    if t.hash.is_empty() {
        t.hash = super::state::hash_of(&here.1);
    }
    t.scope = scope.to_string();
    t.updated_at = now;
    t.device = device.to_string();
    t.deleted = false;
    t.synced = false;
    state.items.insert(id.to_string(), t);
    state.leaving.remove(&format!("{scope}|{id}"));
    state.leaving.insert(format!("{old}|{id}"), super::state::Tracked { hash: String::new(), updated_at: now, device: device.to_string(), scope: old, deleted: true, synced: false });
    Ok(())
}

/// Items of a team that this account can no longer reach become personal items again (the person already has the copies).
pub fn release_team(state: &mut SyncState, scope: &str, now: u64) -> usize {
    state.cursors.remove(scope);
    state.leaving.retain(|k, _| !k.starts_with(&format!("{scope}|")));
    let mut n = 0;
    for t in state.items.values_mut().filter(|t| t.scope == scope && !t.deleted) {
        t.scope = "u".into();
        t.updated_at = now;
        t.synced = false;
        n += 1;
    }
    for t in state.items.values_mut().filter(|t| t.scope == scope && t.deleted) {
        t.scope = "u".into();
        t.synced = true;
    }
    n
}

/// The group, its sub-groups, their hosts, and the keys and identities those hosts rely on: what "share this group" moves.
pub fn group_items(vault: &Vault, gid: u64) -> Vec<String> {
    let mut groups = vec![gid];
    let mut i = 0;
    while i < groups.len() {
        let g = groups[i];
        let kids: Vec<u64> = vault.groups.iter().filter(|x| x.parent == Some(g) && !groups.contains(&x.id)).map(|x| x.id).collect();
        groups.extend(kids);
        i += 1;
    }
    let mut out: Vec<String> = groups.iter().map(|g| item_id("group", *g)).collect();
    let (mut keys, mut idents) = (vec![], vec![]);
    for h in vault.hosts.iter().filter(|h| h.group.map(|g| groups.contains(&g)).unwrap_or(false)) {
        out.push(item_id("host", h.id));
        keys.extend(h.key);
        idents.extend(h.identity);
    }
    for n in &idents {
        if let Some(k) = vault.identities.iter().find(|x| x.id == *n).and_then(|x| x.key) {
            keys.push(k);
        }
    }
    idents.sort();
    idents.dedup();
    keys.sort();
    keys.dedup();
    out.extend(idents.into_iter().filter(|n| vault.identities.iter().any(|x| x.id == *n)).map(|n| item_id("identity", n)));
    out.extend(keys.into_iter().filter(|n| vault.keys.iter().any(|x| x.id == *n)).map(|n| item_id("key", n)));
    out
}

/// Moves a whole group (see `group_items`) to `scope`. Items already there are left alone. One warning covers every secret
/// that would become readable; nothing moves until `confirmed`. Returns how many items moved.
pub fn share_group(state: &mut SyncState, vault: &Vault, gid: u64, scope: &str, confirmed: bool, device: &str, now: u64) -> Result<usize, MoveErr> {
    let scope_now = |state: &SyncState, id: &str| state.items.get(id).map(|t| t.scope.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "u".into());
    let ids: Vec<String> = group_items(vault, gid).into_iter().filter(|i| scope_now(state, i) != scope).collect();
    if scope != "u" && !confirmed {
        let n = ids.iter().filter(|i| warning_for(vault, i).is_some()).count();
        if n > 0 {
            return Err(MoveErr::NeedsConfirm(format!("{n} item{} in this group hold saved passwords, passphrases or keys. Every team member will be able to read them.", if n == 1 { "" } else { "s" })));
        }
    }
    let mut moved = 0;
    for id in ids {
        match move_to_scope(state, vault, &id, scope, true, device, now) {
            Ok(()) => moved += 1,
            Err(MoveErr::Same) | Err(MoveErr::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(moved)
}

/// A new item lands in a group that is shared: it joins the same team. Returns the scope it joined.
pub fn inherit_group_scope(state: &mut SyncState, vault: &Vault, kind: &str, id: u64, device: &str, now: u64) -> Option<String> {
    let group = match kind {
        "host" => vault.hosts.iter().find(|h| h.id == id)?.group?,
        "group" => vault.groups.iter().find(|g| g.id == id)?.parent?,
        _ => return None,
    };
    let scope = state.items.get(&item_id("group", group)).map(|t| t.scope.clone()).filter(|s| s.starts_with("t:"))?;
    move_to_scope(state, vault, &item_id(kind, id), &scope, true, device, now).ok()?;
    Some(scope)
}

pub fn host_item(id: u64) -> String {
    item_id("host", id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use crate::sync::state::VaultStore;
    use sync_client::engine::Store;

    fn vault() -> Vault {
        let mut v = Vault { id_schema: 2, ..Default::default() };
        v.hosts.push(Host { id: 11, label: "web".into(), ..Default::default() });
        v.hosts.push(Host { id: 12, label: "db".into(), password: "pw-secret".into(), ..Default::default() });
        v.identities.push(Identity { id: 13, label: "deploy".into(), username: "u".into(), password: "p".into(), key: None });
        v
    }

    fn group_vault() -> Vault {
        let mut v = Vault { id_schema: 2, ..Default::default() };
        v.groups.push(Group { id: 1, name: "Prod".into(), parent: None });
        v.groups.push(Group { id: 2, name: "Web".into(), parent: Some(1) });
        v.groups.push(Group { id: 3, name: "Other".into(), parent: None });
        v.keys.push(SshKey { id: 20, label: "k".into(), ..Default::default() });
        v.identities.push(Identity { id: 21, label: "deploy".into(), username: "u".into(), password: String::new(), key: Some(20) });
        v.hosts.push(Host { id: 30, label: "a".into(), group: Some(1), key: Some(20), ..Default::default() });
        v.hosts.push(Host { id: 31, label: "b".into(), group: Some(2), identity: Some(21), ..Default::default() });
        v.hosts.push(Host { id: 32, label: "c".into(), group: Some(3), ..Default::default() });
        v
    }

    #[test]
    fn group_share_covers_subgroups_hosts_and_what_they_use() {
        let mut ids = group_items(&group_vault(), 1);
        ids.sort();
        let mut want = vec![item_id("group", 1), item_id("group", 2), item_id("host", 30), item_id("host", 31), item_id("identity", 21), item_id("key", 20)];
        want.sort();
        assert_eq!(ids, want, "the other group and its host stay out");
    }

    #[test]
    fn sharing_a_group_moves_everything_and_unsharing_brings_it_back() {
        let (mut v, mut s) = (group_vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        let n = share_group(&mut s, &v, 1, "t:5", true, "d", 200).unwrap();
        assert_eq!(n, 6);
        assert!(group_items(&v, 1).iter().all(|i| s.items[i].scope == "t:5"));
        assert_eq!(s.items[&item_id("host", 32)].scope, "u");
        let n = share_group(&mut s, &v, 1, "u", false, "d", 300).unwrap();
        assert_eq!(n, 6);
        assert!(group_items(&v, 1).iter().all(|i| s.items[i].scope == "u"));
    }

    #[test]
    fn group_share_asks_once_when_secrets_would_be_exposed() {
        let (mut v, mut s) = (group_vault(), SyncState::default());
        v.hosts[0].password = "pw".into();
        v.identities[0].password = "p2".into();
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        match share_group(&mut s, &v, 1, "t:5", false, "d", 200) {
            Err(MoveErr::NeedsConfirm(w)) => assert!(w.contains("2"), "{w}"),
            other => panic!("{other:?}"),
        }
        assert!(group_items(&v, 1).iter().all(|i| s.items[i].scope == "u"), "nothing moved before the answer");
    }

    #[test]
    fn a_new_host_in_a_shared_group_joins_the_team() {
        let (mut v, mut s) = (group_vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        share_group(&mut s, &v, 1, "t:5", true, "d", 200).unwrap();
        v.hosts.push(Host { id: 40, label: "new".into(), group: Some(2), ..Default::default() });
        v.hosts.push(Host { id: 41, label: "elsewhere".into(), group: Some(3), ..Default::default() });
        assert_eq!(inherit_group_scope(&mut s, &v, "host", 40, "d", 300).as_deref(), Some("t:5"));
        assert_eq!(inherit_group_scope(&mut s, &v, "host", 41, "d", 300), None);
        assert_eq!(s.items[&item_id("host", 40)].scope, "t:5");
    }

    #[test]
    fn an_item_created_straight_into_a_team_never_exists_in_personal_scope() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        v.hosts.push(Host { id: 50, label: "brand new".into(), ..Default::default() });
        move_to_scope(&mut s, &v, &host_item(50), "t:5", true, "d", 200).unwrap();
        let snap = VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(300);
        let mine: Vec<_> = snap.iter().filter(|i| i.id == host_item(50) && i.json.is_some()).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].scope, "t:5");
    }

    #[test]
    fn move_makes_new_scope_item_and_old_scope_tombstone() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        let snap = VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(300);
        let mine: Vec<_> = snap.iter().filter(|i| i.id == host_item(11)).collect();
        assert_eq!(mine.len(), 2);
        let live = mine.iter().find(|i| i.scope == "t:5").unwrap();
        let gone = mine.iter().find(|i| i.scope == "u").unwrap();
        assert!(live.dirty && live.json.is_some() && !live.stamp.deleted);
        assert!(gone.dirty && gone.json.is_none() && gone.stamp.deleted);
    }

    #[test]
    fn move_back_to_personal_leaves_tombstone_in_the_team() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        move_to_scope(&mut s, &v, &host_item(11), "u", false, "d", 300).unwrap();
        let snap = VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(400);
        let mine: Vec<_> = snap.iter().filter(|i| i.id == host_item(11)).collect();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().any(|i| i.scope == "u" && i.json.is_some()));
        assert!(mine.iter().any(|i| i.scope == "t:5" && i.stamp.deleted));
    }

    #[test]
    fn shared_identity_requires_confirmation_flag() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        let id = item_id("identity", 13);
        assert!(matches!(move_to_scope(&mut s, &v, &id, "t:5", false, "d", 200), Err(MoveErr::NeedsConfirm(_))));
        assert!(matches!(move_to_scope(&mut s, &v, &host_item(12), "t:5", false, "d", 200), Err(MoveErr::NeedsConfirm(_))), "host with a saved password");
        assert_eq!(s.items[&id].scope, "u", "nothing moved without the flag");
        move_to_scope(&mut s, &v, &id, "t:5", true, "d", 200).unwrap();
        assert_eq!(s.items[&id].scope, "t:5");
        // a plain host, and anything going back to personal, need no confirmation
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        move_to_scope(&mut s, &v, &id, "u", false, "d", 300).unwrap();
    }

    #[test]
    fn moving_to_the_same_scope_or_a_missing_item_is_an_error() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        assert_eq!(move_to_scope(&mut s, &v, &host_item(11), "u", false, "d", 2), Err(MoveErr::Same));
        assert_eq!(move_to_scope(&mut s, &v, &host_item(999), "t:5", false, "d", 2), Err(MoveErr::NotFound));
    }

    #[test]
    fn tombstone_from_old_scope_does_not_delete_the_moved_item() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        let gone = LocalItemExt::tomb(&host_item(11), "u", 900);
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.apply(vec![gone], 1000);
        assert!(v.hosts.iter().any(|h| h.id == 11));
    }

    #[test]
    fn leaving_tombstone_is_dropped_once_the_server_has_it() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        let stamp = sync_core::merge::Stamp { updated_at: 200, deleted: true, device: "d".into() };
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.mark_synced("u", vec![(host_item(11), stamp.clone())]);
        assert!(s.leaving.is_empty());
        let live = sync_core::merge::Stamp { deleted: false, ..stamp };
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.mark_synced("t:5", vec![(host_item(11), live)]);
        assert!(s.items[&host_item(11)].synced);
    }

    #[test]
    fn release_team_turns_items_personal() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        s.cursors.insert("t:5".into(), 9);
        assert_eq!(release_team(&mut s, "t:5", 300), 1);
        assert_eq!(s.items[&host_item(11)].scope, "u");
        assert!(!s.items[&host_item(11)].synced && !s.cursors.contains_key("t:5"));
    }

    fn live(id: &str, scope: &str, t: u64, h: &Host) -> sync_client::engine::LocalItem {
        sync_client::engine::LocalItem { id: id.into(), scope: scope.into(), stamp: sync_core::merge::Stamp { updated_at: t, deleted: false, device: "o".into() }, json: Some(serde_json::to_vec(h).unwrap()), dirty: false }
    }

    #[test]
    fn team_item_cannot_overwrite_a_personal_item_with_the_same_id() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        // a teammate who once saw this id pushes a host with it into the team scope
        let evil = Host { id: 11, label: "evil".into(), address: "attacker.example".into(), ..Default::default() };
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.apply(vec![live(&host_item(11), "t:5", 900, &evil)], 1000);
        assert_eq!(v.hosts.iter().find(|h| h.id == 11).unwrap().label, "web", "my personal host is untouched");
        assert_eq!(s.items[&host_item(11)].scope, "u");
        let snap = VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(1100);
        assert!(snap.iter().all(|i| i.id != host_item(11) || i.scope == "u"), "and it is not pushed to the team either");
    }

    #[test]
    fn a_move_from_my_other_device_still_arrives_in_either_order() {
        let moved = Host { id: 11, label: "web (shared)".into(), ..Default::default() };
        let tomb = LocalItemExt::tomb(&host_item(11), "u", 900);
        for order in [0, 1] {
            let (mut v, mut s) = (vault(), SyncState::default());
            VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
            let mut batch = vec![live(&host_item(11), "t:5", 900, &moved), tomb.clone()];
            if order == 1 {
                batch.reverse();
            }
            VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.apply(batch, 1000);
            assert_eq!(v.hosts.iter().find(|h| h.id == 11).map(|h| h.label.as_str()), Some("web (shared)"), "order {order}");
            assert_eq!(s.items[&host_item(11)].scope, "t:5", "order {order}");
            assert!(s.parked.is_empty());
        }
    }

    #[test]
    fn a_move_back_to_personal_needs_no_waiting() {
        let (mut v, mut s) = (vault(), SyncState::default());
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.snapshot(100);
        move_to_scope(&mut s, &v, &host_item(11), "t:5", false, "d", 200).unwrap();
        let back = Host { id: 11, label: "web (back)".into(), ..Default::default() };
        VaultStore { vault: &mut v, state: &mut s, device: "d".into() }.apply(vec![live(&host_item(11), "u", 900, &back)], 1000);
        // this device has an unsent move of its own, so it keeps it; the point is only that "u" is trusted, never parked
        assert!(s.parked.is_empty());
    }

    struct LocalItemExt;
    impl LocalItemExt {
        fn tomb(id: &str, scope: &str, t: u64) -> sync_client::engine::LocalItem {
            sync_client::engine::LocalItem { id: id.into(), scope: scope.into(), stamp: sync_core::merge::Stamp { updated_at: t, deleted: true, device: "o".into() }, json: None, dirty: false }
        }
    }
}

impl TeamCfg {
    pub fn can_write(&self) -> bool {
        sync_core::wire::roles::can_write(if self.role.is_empty() && self.owner { "owner" } else { &self.role })
    }
}

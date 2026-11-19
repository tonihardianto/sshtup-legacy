use base64::Engine as _;
use std::collections::HashMap;
use sync_client::account::{login, register, Session};
use sync_client::api::Api;
use sync_client::engine::{Engine, LocalItem, Store};
use sync_client::teams::*;
use sync_core::merge::Stamp;
use sync_server::{create_invite, spawn_test_server};

#[derive(Default)]
struct Mem {
    items: HashMap<String, LocalItem>,
    device: String,
}
impl Store for Mem {
    fn snapshot(&mut self, _now: u64) -> Vec<LocalItem> {
        self.items.values().cloned().collect()
    }
    fn apply(&mut self, items: Vec<LocalItem>, _now: u64) {
        for i in items {
            self.items.insert(i.id.clone(), i);
        }
    }
    fn mark_synced(&mut self, _scope: &str, done: Vec<(String, Stamp)>) {
        for (id, st) in done {
            if let Some(i) = self.items.get_mut(&id) {
                i.stamp = st;
                i.dirty = false;
            }
        }
    }
}
impl Mem {
    fn edit(&mut self, scope: &str, id: &str, text: &str, t: u64) {
        self.items.insert(id.into(), LocalItem { id: id.into(), scope: scope.into(), stamp: Stamp { updated_at: t, deleted: false, device: self.device.clone() }, json: Some(text.into()), dirty: true });
    }
}

const PW: &str = "a long enough password";
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

struct World {
    base: String,
    db: sync_server::db::Db,
}
fn world() -> World {
    let (base, db) = spawn_test_server();
    World { base, db }
}
fn user(w: &World, email: &str) -> (Session, Api) {
    register(&Api::new(&w.base, None), email, PW, &create_invite(&w.db)).unwrap();
    let s = login(&Api::new(&w.base, None), email, PW, "dev").unwrap();
    let api = Api::new(&w.base, Some(s.token.clone()));
    (s, api)
}
fn share(w: &World, owner: &(Session, Api), member: &(Session, Api), member_email: &str) -> i64 {
    let _ = w;
    let team = create_team(&owner.1, &owner.0.secrets.x_public, "ops").unwrap();
    let who = lookup(&owner.1, team, member_email).unwrap();
    let teams = my_teams(&owner.1, &owner.0.secrets.x_secret).unwrap();
    let key = teams[0].key.clone().expect("owner opens own key");
    add_member(&owner.1, team, &who, &key, "editor").unwrap();
    let _ = member;
    team
}

#[test]
fn member_reads_shared_host() {
    let w = world();
    let (o, m) = (user(&w, "o@x.io"), user(&w, "m@x.io"));
    let team = share(&w, &o, &m, "m@x.io");
    let scope = format!("t:{team}");
    let ok = my_teams(&o.1, &o.0.secrets.x_secret).unwrap()[0].key.clone().unwrap();
    let mk = my_teams(&m.1, &m.0.secrets.x_secret).unwrap()[0].key.clone().unwrap();
    assert_eq!(*ok, *mk, "both sides hold the same team key");
    let mut a = Engine::new(Mem { device: "a".into(), ..Default::default() }, Api::new(&w.base, Some(o.0.token.clone())), "a");
    let mut b = Engine::new(Mem { device: "b".into(), ..Default::default() }, Api::new(&w.base, Some(m.0.token.clone())), "b");
    a.store.edit(&scope, "host:1", "shared server", now() - 10);
    a.tick(&scope, &ok, 1, now()).unwrap();
    b.tick(&scope, &mk, 1, now()).unwrap();
    assert_eq!(b.store.items["host:1"].json.as_deref(), Some(b"shared server".as_slice()));
    assert_eq!(b.store.items["host:1"].scope, scope);
}

#[test]
fn server_cannot_read_team_item() {
    let w = world();
    let (o, m) = (user(&w, "o@x.io"), user(&w, "m@x.io"));
    let team = share(&w, &o, &m, "m@x.io");
    let scope = format!("t:{team}");
    let key = my_teams(&o.1, &o.0.secrets.x_secret).unwrap()[0].key.clone().unwrap();
    let mut a = Engine::new(Mem { device: "a".into(), ..Default::default() }, Api::new(&w.base, Some(o.0.token.clone())), "a");
    a.store.edit(&scope, "host:1", "top secret hostname", now() - 10);
    a.tick(&scope, &key, 1, now()).unwrap();
    let (blob, t, d, e): (String, i64, bool, u32) = w
        .db
        .with(|c| c.query_row("SELECT blob,updated_at,deleted,epoch FROM records WHERE scope=?1 AND id='host:1'", [&scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))))
        .unwrap();
    let blob = base64::engine::general_purpose::STANDARD.decode(blob).unwrap();
    let meta = sync_core::seal::ItemMeta { scope: scope.clone(), id: "host:1".into(), updated_at: t as u64, deleted: d, epoch: e };
    // everything the server (or any other account) could hold: sync keys, secrets, public keys, tokens
    let guesses: Vec<[u8; 32]> = vec![*o.0.secrets.sync_key, *m.0.secrets.sync_key, *o.0.secrets.x_secret, o.0.secrets.x_public, [0; 32]];
    for g in guesses {
        assert!(sync_core::seal::open_item(&g, &meta, &blob).is_err());
    }
    assert!(!String::from_utf8_lossy(&blob).contains("top secret"));
    assert!(sync_core::seal::open_item(&key, &meta, &blob).is_ok(), "sanity: the team key opens it");
}

#[test]
fn fingerprint_shown_matches_other_side() {
    let w = world();
    let (o, m) = (user(&w, "o@x.io"), user(&w, "m@x.io"));
    let team = create_team(&o.1, &o.0.secrets.x_public, "ops").unwrap();
    let shown_to_owner = lookup(&o.1, team, "m@x.io").unwrap().fingerprint;
    assert_eq!(shown_to_owner, own_fingerprint(&m.0.secrets.x_public));
    assert_ne!(shown_to_owner, own_fingerprint(&o.0.secrets.x_public));
    // the member list shows the same number
    let list = my_teams(&o.1, &o.0.secrets.x_secret).unwrap();
    assert_eq!(list[0].members[0].fingerprint, own_fingerprint(&o.0.secrets.x_public));
}

#[test]
fn looking_someone_up_gives_them_nothing_until_the_key_is_sent() {
    let w = world();
    let (o, m) = (user(&w, "o@x.io"), user(&w, "m@x.io"));
    let team = create_team(&o.1, &o.0.secrets.x_public, "ops").unwrap();
    lookup(&o.1, team, "m@x.io").unwrap();
    assert!(my_teams(&m.1, &m.0.secrets.x_secret).unwrap().is_empty());
    let mut b = Engine::new(Mem::default(), Api::new(&w.base, Some(m.0.token.clone())), "b");
    assert!(b.tick(&format!("t:{team}"), &[0; 32], 1, now()).is_err());
}

#[test]
fn a_person_is_a_member_with_the_key_right_after_the_owner_adds_them() {
    let w = world();
    let (o, m) = (user(&w, "o@x.io"), user(&w, "m@x.io"));
    let team = share(&w, &o, &m, "m@x.io");
    let mine = my_teams(&m.1, &m.0.secrets.x_secret).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, team);
    assert!(mine[0].ready() && mine[0].key.is_some(), "no accept step: the key is already theirs");
}

// ---- removal and rotation ----

fn teams_of(u: &(Session, Api)) -> Vec<TeamView> {
    my_teams(&u.1, &u.0.secrets.x_secret).unwrap()
}
fn engine(w: &World, u: &(Session, Api), dev: &str) -> Engine<Mem, Api> {
    Engine::new(Mem { device: dev.into(), ..Default::default() }, Api::new(&w.base, Some(u.0.token.clone())), dev)
}
fn join(o: &(Session, Api), m: &(Session, Api), team: i64, email: &str) {
    let who = lookup(&o.1, team, email).unwrap();
    let key = teams_of(o)[0].key.clone().unwrap();
    add_member(&o.1, team, &who, &key, "editor").unwrap();
    let _ = m;
}
/// owner + `removed` + `kept`, with `n` items already in the team
fn three(n: usize) -> (World, (Session, Api), (Session, Api), (Session, Api), i64, Vec<String>) {
    let w = world();
    let (o, r, k) = (user(&w, "o@x.io"), user(&w, "r@x.io"), user(&w, "k@x.io"));
    let team = create_team(&o.1, &o.0.secrets.x_public, "ops").unwrap();
    join(&o, &r, team, "r@x.io");
    join(&o, &k, team, "k@x.io");
    let scope = format!("t:{team}");
    let key = teams_of(&o)[0].key.clone().unwrap();
    let mut a = engine(&w, &o, "a");
    let ids: Vec<String> = (0..n).map(|i| format!("host:{i}")).collect();
    for id in &ids {
        a.store.edit(&scope, id, &format!("secret {id}"), now() - 100);
    }
    a.tick(&scope, &key, 1, now()).unwrap();
    (w, o, r, k, team, ids)
}
fn uid_of(o: &(Session, Api), email: &str) -> i64 {
    teams_of(o)[0].members.iter().find(|m| m.email == email).unwrap().user_id
}

#[test]
fn removed_member_cannot_pull_team_scope() {
    let (w, o, r, _k, team, _) = three(2);
    let old = teams_of(&r)[0].key.clone().unwrap();
    remove_member_and_rotate(&o.1, &teams_of(&o)[0], uid_of(&o, "r@x.io"), &o.0.secrets.x_secret).unwrap();
    let mut e = engine(&w, &r, "r");
    assert!(e.tick(&format!("t:{team}"), &old, 1, now()).is_err());
    assert!(teams_of(&r).is_empty());
}

#[test]
fn removed_member_cannot_decrypt_items_written_after_rotation() {
    let (w, o, r, _k, team, _) = three(1);
    let old = teams_of(&r)[0].key.clone().unwrap();
    remove_member_and_rotate(&o.1, &teams_of(&o)[0], uid_of(&o, "r@x.io"), &o.0.secrets.x_secret).unwrap();
    let t = &teams_of(&o)[0];
    assert!(t.ready() && t.epoch == 2);
    let scope = format!("t:{team}");
    let mut a = engine(&w, &o, "a");
    a.store.edit(&scope, "host:new", "written after", now() - 5);
    a.tick(&scope, t.key.as_ref().unwrap(), 2, now()).unwrap();
    // every blob in the team, as the server holds it
    let rows: Vec<(String, i64, bool, u32, String)> = w
        .db
        .with(|c| c.prepare("SELECT id,updated_at,deleted,epoch,blob FROM records WHERE scope=?1").unwrap().query_map([&scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap().flatten().collect());
    assert_eq!(rows.len(), 2);
    for (id, up, del, ep, blob) in rows {
        let blob = base64::engine::general_purpose::STANDARD.decode(blob).unwrap();
        let meta = sync_core::seal::ItemMeta { scope: scope.clone(), id, updated_at: up as u64, deleted: del, epoch: ep };
        assert_eq!(ep, 2, "everything is under the new epoch");
        assert!(sync_core::seal::open_item(&old, &meta, &blob).is_err(), "the key the removed member had opens nothing");
        assert!(sync_core::seal::open_item(t.key.as_ref().unwrap(), &meta, &blob).is_ok());
    }
}

#[test]
fn remaining_members_read_all_items_after_rotation() {
    let (w, o, _r, k, team, ids) = three(5);
    remove_member_and_rotate(&o.1, &teams_of(&o)[0], uid_of(&o, "r@x.io"), &o.0.secrets.x_secret).unwrap();
    let t = &teams_of(&k)[0];
    assert!(t.ready() && t.epoch == 2);
    let mut b = engine(&w, &k, "k");
    let r = b.tick(&format!("t:{team}"), t.key.as_ref().unwrap(), 2, now()).unwrap();
    assert_eq!(r.skipped_bad, 0);
    for id in ids {
        assert_eq!(b.store.items[&id].json.as_deref(), Some(format!("secret {id}").as_bytes()));
    }
}

#[test]
fn stale_epoch_push_rejected_then_client_refetches_key() {
    let (w, o, r, k, team, _) = three(1);
    let scope = format!("t:{team}");
    let old = teams_of(&k)[0].key.clone().unwrap();
    // the removal has happened but the owner has not rotated yet
    o.1.remove_member(team, uid_of(&o, "r@x.io")).unwrap();
    let mut b = engine(&w, &k, "k");
    b.store.edit(&scope, "host:mine", "from a member", now() - 5);
    let rep = b.tick(&scope, &old, 1, now()).unwrap();
    assert!(rep.rejected.iter().any(|x| x.reason == "stale_epoch"), "{:?}", rep.rejected);
    let t = &teams_of(&k)[0];
    assert!(!t.ready(), "the member sees the key is behind and waits");
    // the owner rotates; the member fetches the new key and the push goes through
    resume_rotation(&o.1, team, &o.0.secrets.x_secret, Some(&teams_of(&o)[0].key.clone().unwrap())).unwrap();
    let t = &teams_of(&k)[0];
    assert!(t.ready());
    let rep = b.tick(&scope, t.key.as_ref().unwrap(), t.epoch, now()).unwrap();
    assert!(rep.rejected.is_empty() && rep.pushed == 1, "{:?}", rep.rejected);
    let _ = r;
}

#[test]
fn rotation_interrupted_midway_can_resume() {
    let (w, o, _r, k, team, ids) = three(5);
    let old = teams_of(&o)[0].key.clone().unwrap();
    o.1.remove_member(team, uid_of(&o, "r@x.io")).unwrap();
    // step 1 (new keys to members) happens, step 2 stops after one batch of two items
    let t = teams_of(&o)[0].clone();
    let members: Vec<_> = t.members.iter().map(|m| sync_core::wire::RotateMember { user_id: m.user_id, sealed_team_key: base64::engine::general_purpose::STANDARD.encode(sync_core::boxes::seal_to(&m.pubkey, &[7u8; 32])) }).collect();
    o.1.rotate(team, &sync_core::wire::RotateReq { epoch: t.epoch, members }).unwrap();
    let t = teams_of(&o)[0].clone();
    assert!(t.ready() && t.stale_items == 5 && t.prev_key.is_some());
    assert_eq!(finish_rotation(&o.1, team, t.prev_key.as_ref().unwrap(), t.key.as_ref().unwrap(), t.epoch, 2, Some(1)).unwrap(), 2);
    let t = teams_of(&o)[0].clone();
    assert_eq!(t.stale_items, 3);
    assert!(t.prev_key.is_some(), "the owner still needs the old key");
    // "the app was closed": the next round finishes the job without being told anything
    resume_rotation(&o.1, team, &o.0.secrets.x_secret, None).unwrap();
    let t = teams_of(&o)[0].clone();
    assert_eq!((t.stale_items, t.prev_key.is_none()), (0, true));
    let mut b = engine(&w, &k, "k");
    let kt = &teams_of(&k)[0];
    b.tick(&format!("t:{team}"), kt.key.as_ref().unwrap(), kt.epoch, now()).unwrap();
    assert_eq!(b.store.items.len(), ids.len());
    let _ = old;
}

#[test]
fn second_removal_waits_until_the_first_rotation_is_finished() {
    let (_w, o, _r, _k, team, _) = three(2);
    o.1.remove_member(team, uid_of(&o, "r@x.io")).unwrap();
    let e = o.1.remove_member(team, uid_of(&o, "k@x.io")).unwrap_err();
    assert!(e.to_string().contains("409"), "{e}");
    let _ = team;
}

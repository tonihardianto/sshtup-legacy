//! The sync round: the network part runs on a worker thread, the vault is only touched on the UI thread.

use super::state::{SyncState, VaultStore};
use super::{from_hex32, now_ms, SyncConfig};
use crate::app::App;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};
use sync_client::api::{Api, Remote};
use sync_client::engine::{Engine, LocalItem, Store};
use sync_client::SyncError;
use sync_core::merge::Stamp;

pub const INTERVAL: Duration = Duration::from_secs(30);

/// Key and epoch for one scope ("u" or "t:<id>").
#[derive(Clone)]
pub struct ScopeKey {
    pub scope: String,
    pub key: [u8; 32],
    pub epoch: u32,
    /// a viewer of a team only takes what others wrote
    pub read_only: bool,
}

/// Hands the worker the snapshot taken on the UI thread and collects what the engine wants to change.
pub struct BufferStore {
    pub snap: Vec<LocalItem>,
    pub applied: Vec<LocalItem>,
    pub synced: Vec<(String, String, Stamp)>,
}

impl Store for BufferStore {
    fn snapshot(&mut self, _now: u64) -> Vec<LocalItem> {
        self.snap.clone()
    }
    fn apply(&mut self, items: Vec<LocalItem>, _now: u64) {
        self.applied.extend(items);
    }
    fn mark_synced(&mut self, scope: &str, done: Vec<(String, Stamp)>) {
        self.synced.extend(done.into_iter().map(|(i, s)| (scope.to_string(), i, s)));
    }
}

#[derive(Default)]
pub struct RoundOut {
    pub applied: Vec<LocalItem>,
    pub synced: Vec<(String, String, Stamp)>,
    pub cursors: HashMap<String, u64>,
    pub pulled: u32,
    pub pushed: u32,
    pub skipped_bad: u32,
    pub conflicts: Vec<LocalItem>,
    pub refused: u32,
    /// teams as the server listed them at the start of the round
    pub teams: Option<Vec<sync_client::teams::TeamView>>,
    /// team scopes the server turned away (usually: removed from the team), with the reason
    pub scope_errors: Vec<(String, SyncError)>,
}

/// Tells the store what reached the server, one scope at a time.
pub fn mark_all(st: &mut impl Store, synced: Vec<(String, String, Stamp)>) {
    let mut by: HashMap<String, Vec<(String, Stamp)>> = HashMap::new();
    for (scope, id, stamp) in synced {
        by.entry(scope).or_default().push((id, stamp));
    }
    for (scope, done) in by {
        st.mark_synced(&scope, done);
    }
}

pub fn run_round<R: Remote>(remote: R, device: &str, scopes: &[ScopeKey], snap: Vec<LocalItem>, cursors: HashMap<String, u64>, now: u64) -> Result<RoundOut, SyncError> {
    let mut e = Engine::new(BufferStore { snap, applied: vec![], synced: vec![] }, remote, device);
    e.cursors = cursors;
    let mut out = RoundOut::default();
    for s in scopes {
        let r = match e.tick_as(&s.scope, &s.key, s.epoch, now, s.read_only) {
            Ok(r) => r,
            // one team going wrong must not stop personal sync
            Err(err) if s.scope != "u" && err != SyncError::Unauthorized && !matches!(err, SyncError::Network(_)) => {
                out.scope_errors.push((s.scope.clone(), err));
                continue;
            }
            Err(err) => return Err(err),
        };
        out.pulled += r.pulled;
        out.pushed += r.pushed;
        out.skipped_bad += r.skipped_bad;
        out.refused += r.rejected.iter().filter(|x| x.reason != "not_newer").count() as u32;
        out.conflicts.extend(r.conflicts);
    }
    out.applied = std::mem::take(&mut e.store.applied);
    out.synced = std::mem::take(&mut e.store.synced);
    out.cursors = e.cursors;
    Ok(out)
}

#[derive(Default)]
pub struct SyncRuntime {
    pub rx: Option<Receiver<Result<RoundOut, SyncError>>>,
    pub last_start: Option<Instant>,
    pub want_now: bool,
    pub last_ok_ms: Option<u64>,
    pub last_error: Option<String>,
    pub summary: String,
    pub state: Option<SyncState>,
    pub teams: Vec<sync_client::teams::TeamView>,
    /// the server no longer accepts this device: stop trying until the person signs in again
    pub stopped: bool,
    /// a team being closed: its items are moving back to Personal, and the team goes once the server has them
    pub delete_team_after: Option<(i64, Vec<String>)>,
    pub ui: super::ui::SyncUi,
}

pub fn should_tick(rt: &SyncRuntime, configured: bool, locked: bool, encrypted: bool, now: Instant) -> bool {
    if !configured || locked || !encrypted || rt.rx.is_some() || rt.stopped {
        return false;
    }
    rt.want_now || rt.last_start.map(|t| now.duration_since(t) >= INTERVAL).unwrap_or(true)
}

/// The text shown for a failed round. Never panics, never blocks the UI.
pub fn describe(e: &SyncError) -> String {
    e.to_string()
}

impl App {
    /// Called every frame: collects a finished round and starts the next one when it is time.
    pub fn sync_poll(&mut self, ctx: &eframe::egui::Context) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        if self.demo || self.lock.is_some() || self.vault_key.is_none() {
            return;
        }
        let Some(log_key) = self.log_key() else { return };
        if self.sync_rt.state.is_none() {
            self.sync_rt.state = Some(SyncState::load(&log_key));
        }
        if let Some(rx) = &self.sync_rt.rx {
            match rx.try_recv() {
                Ok(res) => {
                    self.sync_rt.rx = None;
                    self.sync_finish(res, &log_key);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
                Err(_) => {
                    self.sync_rt.rx = None;
                    self.sync_rt.last_error = Some("The sync round stopped unexpectedly.".into());
                }
            }
            return;
        }
        ctx.request_repaint_after(INTERVAL);
        if !should_tick(&self.sync_rt, true, false, true, Instant::now()) {
            return;
        }
        let Some(sync_key) = from_hex32(&cfg.sync_key) else {
            self.sync_rt.last_error = Some("The sync key on this device is damaged. Sign out and in again.".into());
            self.sync_rt.stopped = true;
            return;
        };
        self.sync_rt.want_now = false;
        self.sync_rt.last_start = Some(Instant::now());
        let now = now_ms();
        let snap = {
            let state = self.sync_rt.state.as_mut().unwrap();
            VaultStore { vault: &mut self.vault, state, device: cfg.device_id.clone() }.snapshot(now)
        };
        let cursors = self.sync_rt.state.as_ref().unwrap().cursors.clone();
        let x_secret = from_hex32(&cfg.x_secret);
        let (tx, rx) = channel();
        self.sync_rt.rx = Some(rx);
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let api = Api::new(&cfg.server, Some(cfg.token.clone()));
            let mut scopes = vec![ScopeKey { scope: "u".into(), key: sync_key, epoch: 1, read_only: false }];
            let mut listed = None;
            if let Some(xs) = x_secret {
                // a failure here only means no team sync this round
                let mut got = sync_client::teams::my_teams(&api, &xs);
                // an owner who was interrupted while removing someone finishes the job first
                if let Ok(teams) = &got {
                    let mut again = false;
                    for t in teams.iter().filter(|t| t.owner && (t.stale_items > 0 || t.key_epoch < t.epoch)) {
                        again |= sync_client::teams::resume_rotation(&api, t.id, &xs, None).is_ok();
                    }
                    if again {
                        got = sync_client::teams::my_teams(&api, &xs);
                    }
                }
                if let Ok(teams) = got {
                    for t in &teams {
                        if let (true, Some(k)) = (t.ready(), &t.key) {
                            scopes.push(ScopeKey { scope: super::teams::scope_of(t.id), key: **k, epoch: t.epoch, read_only: !t.can_write() });
                        }
                    }
                    listed = Some(teams);
                }
            }
            let res = run_round(api, &cfg.device_id, &scopes, snap, cursors, now).map(|mut o| {
                o.teams = listed;
                o
            });
            let _ = tx.send(res);
            ctx2.request_repaint();
        });
    }

    fn sync_finish(&mut self, res: Result<RoundOut, SyncError>, log_key: &crate::vaultcrypt::LogKey) {
        let Some(cfg) = self.vault.sync.clone() else { return };
        match res {
            Ok(out) => {
                let now = now_ms();
                let mut changed = !out.applied.is_empty();
                if let Some(teams) = out.teams {
                    let mut cfg_now = cfg.clone();
                    let old: Vec<i64> = cfg.teams.iter().map(|t| t.id).collect();
                    cfg_now.teams = teams
                        .iter()
                        .filter_map(|t| t.key.as_ref().map(|k| super::teams::TeamCfg { id: t.id, name: t.name.clone(), epoch: t.epoch, owner: t.owner, role: t.role.clone(), key: super::to_hex(&**k) }))
                        .collect();
                    for gone in old.iter().filter(|id| !teams.iter().any(|t| t.id == **id)) {
                        let n = super::teams::release_team(self.sync_rt.state.as_mut().unwrap(), &super::teams::scope_of(*gone), now);
                        self.notify(format!("You are no longer in a team you shared with. Its {n} item(s) stay on this computer as your own."), crate::ui::AMBER());
                    }
                    if cfg_now.teams.iter().map(|t| (t.id, t.epoch, t.key.clone(), t.name.clone())).collect::<Vec<_>>() != cfg.teams.iter().map(|t| (t.id, t.epoch, t.key.clone(), t.name.clone())).collect::<Vec<_>>() {
                        self.vault.sync = Some(cfg_now);
                        changed = true;
                    }
                    self.sync_rt.teams = teams;
                }
                let state = self.sync_rt.state.as_mut().unwrap();
                {
                    let mut st = VaultStore { vault: &mut self.vault, state, device: cfg.device_id.clone() };
                    st.apply(out.applied, now);
                    mark_all(&mut st, out.synced);
                }
                state.cursors = out.cursors;
                state.remember_replaced(&out.conflicts, now);
                // the team goes only once the server holds every item that was in it as a personal item
                let team_to_close = self.sync_rt.delete_team_after.as_ref().filter(|(_, ids)| out.refused == 0 && ids.iter().all(|id| state.items.get(id).map(|t| t.synced && t.scope == "u").unwrap_or(true))).map(|(tid, _)| *tid);
                if !state.save(log_key) {
                    self.sync_rt.last_error = Some("Could not save the sync state.".into());
                } else {
                    self.sync_rt.last_error = None;
                }
                if changed {
                    self.dirty = true;
                }
                self.sync_rt.last_ok_ms = Some(now);
                self.sync_rt.summary = format!("{} received, {} sent", out.pulled, out.pushed);
                if out.skipped_bad > 0 {
                    self.sync_rt.summary.push_str(&format!(", {} could not be read", out.skipped_bad));
                }
                if out.refused > 0 {
                    self.sync_rt.summary.push_str(&format!(", {} refused by the server", out.refused));
                }
                if let Some(tid) = team_to_close {
                    // everything is back in Personal on the server: now the team itself can go
                    self.sync_rt.delete_team_after = None;
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.sync_rt.ui.op = Some(rx);
                    let api = Api::new(&cfg.server, Some(cfg.token.clone()));
                    self.sync_rt.want_now = true;
                    std::thread::spawn(move || {
                        let _ = tx.send(match api.delete_team(tid) {
                            Ok(()) => super::ui::OpResult::Msg("The team was deleted. Its items are back in Personal.".into(), true),
                            Err(e) => super::ui::OpResult::Msg(e.to_string(), false),
                        });
                    });
                } else if self.sync_rt.delete_team_after.is_some() && out.refused > 0 {
                    self.sync_rt.delete_team_after = None;
                    self.sync_rt.ui.message = Some(("The server refused some items, so the team was not deleted.".into(), false));
                    self.notify("The server refused some items, so the team was not deleted.".into(), crate::ui::AMBER());
                }
                if !out.conflicts.is_empty() {
                    self.notify(format!("{} item(s) were replaced by a newer version from another device", out.conflicts.len()), crate::ui::AMBER());
                }
            }
            Err(e) => {
                if e == SyncError::Unauthorized {
                    self.sync_rt.stopped = true;
                }
                self.sync_rt.last_error = Some(describe(&e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_vault_tick_is_skipped() {
        let rt = SyncRuntime::default();
        let now = Instant::now();
        assert!(should_tick(&rt, true, false, true, now));
        assert!(!should_tick(&rt, true, true, true, now), "locked");
        assert!(!should_tick(&rt, true, false, false, now), "not encrypted");
        assert!(!should_tick(&rt, false, false, true, now), "sync not set up");
    }

    #[test]
    fn waits_for_the_interval_but_not_after_want_now() {
        let mut rt = SyncRuntime::default();
        let t0 = Instant::now();
        rt.last_start = Some(t0);
        assert!(!should_tick(&rt, true, false, true, t0 + Duration::from_secs(5)));
        assert!(should_tick(&rt, true, false, true, t0 + INTERVAL));
        rt.want_now = true;
        assert!(should_tick(&rt, true, false, true, t0 + Duration::from_secs(1)));
        rt.stopped = true;
        assert!(!should_tick(&rt, true, false, true, t0 + INTERVAL * 2));
    }

    #[test]
    fn server_down_sets_status_not_panic() {
        let r = run_round(Api::new("http://127.0.0.1:1", Some("t".into())), "d", &[ScopeKey { scope: "u".into(), key: [1; 32], epoch: 1, read_only: false }], vec![], HashMap::new(), 1);
        let e = r.err().unwrap();
        assert!(matches!(e, SyncError::Network(_)));
        assert!(describe(&e).contains("Cannot reach"));
    }

    #[test]
    fn round_against_html_error_page_is_an_error_not_a_panic() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten().take(2) {
                let mut s = s;
                let mut b = [0u8; 2048];
                let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n<html>oops</html");
            }
        });
        let r = run_round(Api::new(&format!("http://{addr}"), Some("t".into())), "d", &[ScopeKey { scope: "u".into(), key: [1; 32], epoch: 1, read_only: false }], vec![], HashMap::new(), 1);
        assert!(r.is_err());
    }

    #[test]
    fn round_against_a_truncated_response_is_an_error_not_a_panic() {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten().take(2) {
                let mut s = s;
                let mut b = [0u8; 2048];
                let _ = s.read(&mut b);
                // promises 1000 bytes, delivers 12, hangs up
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\n\r\n{\"items\":[{\"");
            }
        });
        let r = run_round(Api::new(&format!("http://{addr}"), Some("t".into())), "d", &[ScopeKey { scope: "u".into(), key: [1; 32], epoch: 1, read_only: false }], vec![], HashMap::new(), 1);
        assert!(r.is_err());
    }

    /// Two vaults, one real server: add, edit, delete on one side shows up on the other.
    #[test]
    fn two_vaults_converge_through_a_real_server() {
        use crate::model::{Host, Vault};
        let (base, db) = sync_server::spawn_test_server();
        sync_client::account::register(&Api::new(&base, None), "me@x.io", "a long enough password", &sync_server::create_invite(&db)).unwrap();
        let key = [4u8; 32];
        struct Dev {
            vault: Vault,
            state: SyncState,
            api_token: String,
            id: String,
        }
        let mk = |name: &str| {
            let s = sync_client::account::login(&Api::new(&base, None), "me@x.io", "a long enough password", name).unwrap();
            Dev { vault: Vault { id_schema: 2, ..Default::default() }, state: SyncState::default(), api_token: s.token, id: s.device_id }
        };
        let (mut a, mut b) = (mk("a"), mk("b"));
        let round = |d: &mut Dev, now: u64| {
            let snap = VaultStore { vault: &mut d.vault, state: &mut d.state, device: d.id.clone() }.snapshot(now);
            let out = run_round(Api::new(&base, Some(d.api_token.clone())), &d.id, &[ScopeKey { scope: "u".into(), key, epoch: 1, read_only: false }], snap, d.state.cursors.clone(), now).unwrap();
            let mut st = VaultStore { vault: &mut d.vault, state: &mut d.state, device: d.id.clone() };
            st.apply(out.applied, now);
            mark_all(&mut st, out.synced);
            d.state.cursors = out.cursors;
        };
        let now = now_ms();
        a.vault.hosts.push(Host { id: 5_000_001, label: "web".into(), password: "pw".into(), ..Default::default() });
        round(&mut a, now);
        round(&mut b, now + 1);
        assert_eq!(b.vault.hosts.len(), 1);
        assert_eq!(b.vault.hosts[0].password, "pw");

        b.vault.hosts[0].label = "web-renamed".into();
        round(&mut b, now + 1000);
        round(&mut a, now + 2000);
        assert_eq!(a.vault.hosts[0].label, "web-renamed");

        a.vault.hosts.clear();
        round(&mut a, now + 3000);
        round(&mut b, now + 4000);
        assert!(b.vault.hosts.is_empty());
        // and nothing is left waiting to be sent
        round(&mut a, now + 5000);
        round(&mut b, now + 5000);
        assert!(a.state.items.values().all(|t| t.synced));
        assert!(b.state.items.values().all(|t| t.synced));
    }

    struct Dev {
        vault: crate::model::Vault,
        state: SyncState,
        token: String,
        id: String,
        sync_key: [u8; 32],
        x_secret: [u8; 32],
    }
    fn dev(base: &str, email: &str, db: &sync_server::db::Db, new_account: bool) -> Dev {
        if new_account {
            sync_client::account::register(&Api::new(base, None), email, "a long enough password", &sync_server::create_invite(db)).unwrap();
        }
        let s = sync_client::account::login(&Api::new(base, None), email, "a long enough password", "d").unwrap();
        Dev { vault: crate::model::Vault { id_schema: 2, ..Default::default() }, state: SyncState::default(), token: s.token, id: s.device_id, sync_key: *s.secrets.sync_key, x_secret: *s.secrets.x_secret }
    }
    /// One round the way the app does it: personal scope plus every team the server lists.
    fn team_round(base: &str, d: &mut Dev, now: u64) -> RoundOut {
        let snap = VaultStore { vault: &mut d.vault, state: &mut d.state, device: d.id.clone() }.snapshot(now);
        let api = Api::new(base, Some(d.token.clone()));
        let teams = sync_client::teams::my_teams(&api, &d.x_secret).unwrap();
        let mut scopes = vec![ScopeKey { scope: "u".into(), key: d.sync_key, epoch: 1, read_only: false }];
        for t in &teams {
            scopes.push(ScopeKey { scope: super::super::teams::scope_of(t.id), key: **t.key.as_ref().unwrap(), epoch: t.epoch, read_only: false });
        }
        let out = run_round(Api::new(base, Some(d.token.clone())), &d.id, &scopes, snap, d.state.cursors.clone(), now).unwrap();
        let mut st = VaultStore { vault: &mut d.vault, state: &mut d.state, device: d.id.clone() };
        st.apply(out.applied.clone(), now);
        mark_all(&mut st, out.synced.clone());
        d.state.cursors = out.cursors.clone();
        out
    }

    #[test]
    fn move_to_team_reencrypts_with_team_key_and_back_to_personal() {
        use crate::model::Host;
        use crate::sync::teams::{move_to_scope, scope_of};
        let (base, db) = sync_server::spawn_test_server();
        let mut o = dev(&base, "o@x.io", &db, true);
        let mut m = dev(&base, "m@x.io", &db, true);
        let mut o2 = dev(&base, "o@x.io", &db, false); // the owner's second computer
        let oapi = Api::new(&base, Some(o.token.clone()));
        let kp = sync_core::boxes::keypair_from_secret(&o.x_secret);
        let id = sync_client::teams::create_team(&oapi, &kp.public, "ops").unwrap();
        let who = sync_client::teams::lookup(&oapi, id, "m@x.io").unwrap();
        let key = sync_client::teams::my_teams(&oapi, &o.x_secret).unwrap()[0].key.clone().unwrap();
        sync_client::teams::add_member(&oapi, id, &who, &key, "editor").unwrap();

        let now = now_ms();
        o.vault.hosts.push(Host { id: 41, label: "shared-web".into(), ..Default::default() });
        team_round(&base, &mut o, now);
        move_to_scope(&mut o.state, &o.vault, &super::super::state::item_id("host", 41), &scope_of(id), false, &o.id.clone(), now + 10).unwrap();
        team_round(&base, &mut o, now + 20);
        team_round(&base, &mut m, now + 30);
        assert_eq!(m.vault.hosts.len(), 1, "the member got it through the team scope");
        // the owner's other computer: the team copy arrives and the old personal tombstone does not delete it
        team_round(&base, &mut o2, now + 40);
        assert_eq!(o2.vault.hosts.len(), 1);
        assert_eq!(o2.state.items[&super::super::state::item_id("host", 41)].scope, scope_of(id));
        // the server holds the personal copy only as a tombstone
        let live_in_u: i64 = db.with(|c| c.query_row("SELECT COUNT(*) FROM records WHERE scope LIKE 'u:%' AND id LIKE 'host:%' AND deleted=0", [], |r| r.get(0))).unwrap();
        assert_eq!(live_in_u, 0);

        // and back
        move_to_scope(&mut o.state, &o.vault, &super::super::state::item_id("host", 41), "u", false, &o.id.clone(), now + 50).unwrap();
        team_round(&base, &mut o, now + 60);
        team_round(&base, &mut m, now + 70);
        assert!(m.vault.hosts.is_empty(), "the member lost it when it went back to personal");
        team_round(&base, &mut o2, now + 80);
        assert_eq!(o2.vault.hosts.len(), 1);
        assert_eq!(o2.state.items[&super::super::state::item_id("host", 41)].scope, "u");
        assert!(o.state.items.values().all(|t| t.synced) && o.state.leaving.is_empty());
    }
}

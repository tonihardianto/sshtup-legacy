use crate::api::Remote;
use crate::SyncError;
use std::collections::HashMap;
use sync_core::merge::{newer, Stamp};
use sync_core::seal::{open_item, seal_item, ItemMeta};
use sync_core::wire::{Rejected, WireItem};

/// One synced thing as the local vault sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalItem {
    /// "host:<hex>", "snippet:<hex>" ...
    pub id: String,
    /// "u" or "t:<team id>"
    pub scope: String,
    pub stamp: Stamp,
    /// None = deleted (a tombstone)
    pub json: Option<Vec<u8>>,
    /// changed here since it was last in step with the server
    pub dirty: bool,
}

pub trait Store {
    /// Every item, with `dirty` and the current stamp. Edits and deletions made since the last sync show up here.
    fn snapshot(&mut self, now: u64) -> Vec<LocalItem>;
    /// Take what the server has: add, replace or delete. These items are in step with the server afterwards.
    fn apply(&mut self, items: Vec<LocalItem>, now: u64);
    /// These (id, stamp) pairs are now on the server.
    fn mark_synced(&mut self, scope: &str, done: Vec<(String, Stamp)>);
}

#[derive(Debug, Default)]
pub struct TickReport {
    pub pulled: u32,
    pub pushed: u32,
    pub skipped_bad: u32,
    /// local versions that were replaced by a newer one from another device
    pub conflicts: Vec<LocalItem>,
    pub rejected: Vec<Rejected>,
}

pub struct Engine<S: Store, R: Remote> {
    pub store: S,
    pub remote: R,
    pub device: String,
    pub cursors: HashMap<String, u64>,
}

const PAGE: u32 = 200;
const PUSH_CHUNK: usize = 500;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

impl<S: Store, R: Remote> Engine<S, R> {
    pub fn new(store: S, remote: R, device: &str) -> Self {
        Engine { store, remote, device: device.to_string(), cursors: HashMap::new() }
    }

    /// One round for one scope: take what is new on the server, then send what changed here.
    pub fn tick(&mut self, scope: &str, key: &[u8; 32], epoch: u32, now: u64) -> Result<TickReport, SyncError> {
        self.tick_as(scope, key, epoch, now, false)
    }

    /// Like `tick`; with `read_only` the round only takes (a viewer of a team has nothing to send).
    pub fn tick_as(&mut self, scope: &str, key: &[u8; 32], epoch: u32, now: u64, read_only: bool) -> Result<TickReport, SyncError> {
        use base64::Engine as _;
        let mut report = TickReport::default();
        let local: HashMap<String, LocalItem> = self.store.snapshot(now).into_iter().filter(|i| i.scope == scope).map(|i| (i.id.clone(), i)).collect();

        // 1. pull
        let mut incoming: HashMap<String, LocalItem> = HashMap::new();
        let mut cursor = self.cursors.get(scope).copied().unwrap_or(0);
        loop {
            let page = self.remote.pull(scope, cursor, PAGE)?;
            for (_, w) in &page.items {
                let meta = ItemMeta { scope: scope.to_string(), id: w.id.clone(), updated_at: w.updated_at, deleted: w.deleted, epoch: w.epoch };
                let plain = b64().decode(&w.blob).ok().and_then(|blob| open_item(key, &meta, &blob).ok());
                let Some(plain) = plain else {
                    report.skipped_bad += 1;
                    continue;
                };
                let stamp = Stamp { updated_at: w.updated_at, deleted: w.deleted, device: w.device.clone() };
                if incoming.get(&w.id).map(|i| !newer(&stamp, &i.stamp)).unwrap_or(false) {
                    continue;
                }
                let wins = local.get(&w.id).map(|l| newer(&stamp, &l.stamp)).unwrap_or(true);
                if wins {
                    incoming.insert(w.id.clone(), LocalItem { id: w.id.clone(), scope: scope.to_string(), stamp, json: if w.deleted { None } else { Some(plain.to_vec()) }, dirty: false });
                }
            }
            if page.next_seq > cursor {
                cursor = page.next_seq;
            }
            if !page.more {
                break;
            }
        }
        for id in incoming.keys() {
            if let Some(l) = local.get(id).filter(|l| l.dirty) {
                report.conflicts.push(l.clone());
            }
        }
        report.pulled = incoming.len() as u32;
        if !incoming.is_empty() {
            self.store.apply(incoming.values().cloned().collect(), now);
        }
        self.cursors.insert(scope.to_string(), cursor);

        // 2. push what changed here and did not just lose to the server
        let mut out: Vec<(LocalItem, WireItem)> = Vec::new();
        for l in local.values().filter(|l| !read_only && l.dirty && !incoming.contains_key(&l.id)) {
            // a clock that runs ahead must not make the server refuse the item
            let updated_at = l.stamp.updated_at.min(now);
            let meta = ItemMeta { scope: scope.to_string(), id: l.id.clone(), updated_at, deleted: l.stamp.deleted, epoch };
            let blob = seal_item(key, &meta, l.json.as_deref().unwrap_or(&[]));
            let mut item = l.clone();
            item.stamp.updated_at = updated_at;
            out.push((item, WireItem { id: l.id.clone(), updated_at, deleted: l.stamp.deleted, epoch, device: self.device.clone(), blob: b64().encode(blob) }));
        }
        for chunk in out.chunks(PUSH_CHUNK) {
            let resp = self.remote.push(scope, chunk.iter().map(|c| c.1.clone()).collect())?;
            let done: Vec<(String, Stamp)> = chunk.iter().filter(|c| !resp.rejected.iter().any(|r| r.id == c.0.id)).map(|c| (c.0.id.clone(), Stamp { device: self.device.clone(), ..c.0.stamp.clone() })).collect();
            report.pushed += done.len() as u32;
            if resp.rejected.iter().any(|r| r.reason == "not_newer") {
                // the server holds a newer copy that this device skipped past: fetch it again next round
                self.cursors.insert(scope.to_string(), 0);
            }
            report.rejected.extend(resp.rejected);
            self.store.mark_synced(scope, done);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use sync_core::wire::{PullResp, PushResp};

    const KEY: [u8; 32] = [3; 32];

    /// A server that behaves like the real one, in memory.
    #[derive(Default)]
    struct MemServer {
        seq: u64,
        rows: Vec<(u64, WireItem)>,
        now: u64,
        corrupt: bool,
    }
    #[derive(Clone)]
    struct Mem(Rc<RefCell<MemServer>>);
    impl Remote for Mem {
        fn push(&mut self, _scope: &str, items: Vec<WireItem>) -> Result<PushResp, SyncError> {
            let mut s = self.0.borrow_mut();
            let mut resp = PushResp::default();
            for it in items {
                if it.updated_at > s.now + 300_000 {
                    resp.rejected.push(Rejected { id: it.id, reason: "future".into() });
                    continue;
                }
                if let Some((_, old)) = s.rows.iter().find(|r| r.1.id == it.id) {
                    let a = Stamp { updated_at: it.updated_at, deleted: it.deleted, device: it.device.clone() };
                    let b = Stamp { updated_at: old.updated_at, deleted: old.deleted, device: old.device.clone() };
                    if !newer(&a, &b) {
                        resp.rejected.push(Rejected { id: it.id, reason: "not_newer".into() });
                        continue;
                    }
                }
                s.seq += 1;
                let seq = s.seq;
                s.rows.retain(|r| r.1.id != it.id);
                s.rows.push((seq, it));
                resp.accepted += 1;
            }
            Ok(resp)
        }
        fn pull(&mut self, _scope: &str, since: u64, limit: u32) -> Result<PullResp, SyncError> {
            let s = self.0.borrow();
            let mut rows: Vec<(u64, WireItem)> = s.rows.iter().filter(|r| r.0 > since).cloned().collect();
            rows.sort_by_key(|r| r.0);
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            if s.corrupt {
                for r in rows.iter_mut() {
                    r.1.blob = "!!not base64!!".into();
                }
            }
            let next = rows.last().map(|r| r.0).unwrap_or(since);
            Ok(PullResp { items: rows, next_seq: next, more })
        }
    }

    #[derive(Default)]
    struct MemStore {
        items: HashMap<String, LocalItem>,
        device: String,
    }
    impl MemStore {
        fn edit(&mut self, id: &str, text: &str, t: u64) {
            let dev = self.device.clone();
            self.items.insert(id.into(), LocalItem { id: id.into(), scope: "u".into(), stamp: Stamp { updated_at: t, deleted: false, device: dev }, json: Some(text.as_bytes().to_vec()), dirty: true });
        }
        fn delete(&mut self, id: &str, t: u64) {
            let dev = self.device.clone();
            let i = self.items.get_mut(id).unwrap();
            i.stamp = Stamp { updated_at: t, deleted: true, device: dev };
            i.json = None;
            i.dirty = true;
        }
        fn text(&self, id: &str) -> Option<String> {
            self.items.get(id).and_then(|i| i.json.as_ref()).map(|j| String::from_utf8(j.clone()).unwrap())
        }
    }
    impl Store for MemStore {
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
                    if i.stamp.updated_at >= st.updated_at {
                        i.stamp = st;
                        i.dirty = false;
                    }
                }
            }
        }
    }

    fn pair() -> (Engine<MemStore, Mem>, Engine<MemStore, Mem>, Mem) {
        let srv = Mem(Rc::new(RefCell::new(MemServer { now: 1_000_000, ..Default::default() })));
        let a = Engine::new(MemStore { device: "a".into(), ..Default::default() }, srv.clone(), "a");
        let b = Engine::new(MemStore { device: "b".into(), ..Default::default() }, srv.clone(), "b");
        (a, b, srv)
    }
    fn tick(e: &mut Engine<MemStore, Mem>, now: u64) -> TickReport {
        e.tick("u", &KEY, 1, now).unwrap()
    }

    #[test]
    fn new_item_pushed_and_pulled_by_other_device() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:1", "one", 10);
        assert_eq!(tick(&mut a, 100).pushed, 1);
        assert_eq!(tick(&mut b, 100).pulled, 1);
        assert_eq!(b.store.text("host:1").as_deref(), Some("one"));
        assert!(!b.store.items["host:1"].dirty);
    }
    #[test]
    fn a_read_only_round_pulls_but_sends_nothing() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:1", "one", 10);
        tick(&mut a, 100);
        b.store.edit("host:2", "two", 20);
        let r = b.tick_as("u", &KEY, 1, 100, true).unwrap();
        assert_eq!((r.pulled, r.pushed), (1, 0));
        assert!(b.store.items["host:2"].dirty);
        assert_eq!(tick(&mut a, 100).pulled, 0);
    }
    #[test]
    fn edit_propagates() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:1", "one", 10);
        tick(&mut a, 100);
        tick(&mut b, 100);
        b.store.edit("host:1", "two", 20);
        tick(&mut b, 200);
        tick(&mut a, 200);
        assert_eq!(a.store.text("host:1").as_deref(), Some("two"));
    }
    #[test]
    fn delete_propagates_as_tombstone() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:1", "one", 10);
        tick(&mut a, 100);
        tick(&mut b, 100);
        a.store.delete("host:1", 30);
        tick(&mut a, 300);
        tick(&mut b, 300);
        assert!(b.store.items["host:1"].stamp.deleted);
        assert_eq!(b.store.text("host:1"), None);
    }
    #[test]
    fn delete_vs_edit_newer_wins_same_on_both() {
        for delete_is_newer in [true, false] {
            let (mut a, mut b, _) = pair();
            a.store.edit("host:1", "one", 10);
            tick(&mut a, 100);
            tick(&mut b, 100);
            let (td, te) = if delete_is_newer { (50, 40) } else { (40, 50) };
            a.store.delete("host:1", td);
            b.store.edit("host:1", "edited", te);
            tick(&mut a, 500);
            tick(&mut b, 500);
            tick(&mut a, 600);
            tick(&mut b, 600);
            assert_eq!(a.store.text("host:1"), b.store.text("host:1"));
            assert_eq!(a.store.text("host:1").is_none(), delete_is_newer);
        }
    }
    #[test]
    fn bad_blob_is_skipped_and_counted() {
        let (mut a, mut b, srv) = pair();
        a.store.edit("host:1", "one", 10);
        a.store.edit("host:2", "two", 11);
        tick(&mut a, 100);
        srv.0.borrow_mut().corrupt = true;
        let r = tick(&mut b, 100);
        assert_eq!((r.skipped_bad, r.pulled), (2, 0));
        srv.0.borrow_mut().corrupt = false;
        // a truncated blob for one item must not stop the rest
        {
            let mut s = srv.0.borrow_mut();
            s.seq += 1;
            let seq = s.seq;
            s.rows.push((seq, WireItem { id: "host:3".into(), updated_at: 5, deleted: false, epoch: 1, device: "a".into(), blob: "QUJD".into() }));
        }
        a.store.edit("host:4", "four", 12);
        tick(&mut a, 200);
        let r = tick(&mut b, 200);
        assert_eq!(r.skipped_bad, 1);
        assert_eq!(b.store.text("host:4").as_deref(), Some("four"));
    }
    #[test]
    fn first_sync_unions_both_sides() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:a1", "A", 10);
        b.store.edit("host:b1", "B", 11);
        tick(&mut a, 100);
        tick(&mut b, 100);
        tick(&mut a, 100);
        assert_eq!(a.store.text("host:b1").as_deref(), Some("B"));
        assert_eq!(b.store.text("host:a1").as_deref(), Some("A"));
    }
    #[test]
    fn local_overwritten_item_reported_as_conflict() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:1", "one", 10);
        tick(&mut a, 100);
        tick(&mut b, 100);
        b.store.edit("host:1", "from b", 50);
        tick(&mut b, 200);
        a.store.edit("host:1", "from a", 40);
        let r = tick(&mut a, 300);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].json.as_deref(), Some(b"from a".as_slice()));
        assert_eq!(a.store.text("host:1").as_deref(), Some("from b"));
    }
    #[test]
    fn future_stamp_is_clamped_to_now() {
        let (mut a, mut b, srv) = pair();
        srv.0.borrow_mut().now = 1_000_000;
        a.store.edit("host:1", "one", 9_000_000); // clock far ahead
        let r = tick(&mut a, 1_000_000);
        assert_eq!((r.pushed, r.rejected.len()), (1, 0));
        assert_eq!(a.store.items["host:1"].stamp.updated_at, 1_000_000);
        tick(&mut b, 1_000_000);
        assert_eq!(b.store.text("host:1").as_deref(), Some("one"));
    }
    #[test]
    fn clock_skewed_device_still_converges() {
        let (mut a, mut b, _) = pair();
        a.store.edit("host:1", "one", 10);
        tick(&mut a, 1_000_000);
        tick(&mut b, 1_000_000);
        // b's clock is behind: its edit has an older stamp than what it already holds
        b.store.edit("host:1", "late", 5);
        let r = tick(&mut b, 1_000_100);
        assert!(r.conflicts.is_empty() || b.store.text("host:1").is_some());
        tick(&mut a, 1_000_200);
        tick(&mut b, 1_000_300);
        assert_eq!(a.store.text("host:1"), b.store.text("host:1"));
    }
}

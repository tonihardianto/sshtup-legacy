use std::collections::HashMap;
use sync_client::api::Api;
use sync_client::engine::{Engine, LocalItem, Store};
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
                if i.stamp.updated_at >= st.updated_at {
                    i.stamp = st;
                    i.dirty = false;
                }
            }
        }
    }
}
impl Mem {
    fn edit(&mut self, id: &str, text: &str, t: u64) {
        self.items.insert(id.into(), LocalItem { id: id.into(), scope: "u".into(), stamp: Stamp { updated_at: t, deleted: false, device: self.device.clone() }, json: Some(text.into()), dirty: true });
    }
}

fn login_device(base: &str, name: &str) -> Api {
    use base64::Engine as _;
    let b = base64::engine::general_purpose::STANDARD;
    let api = Api::new(base, None);
    let r = api.login(&sync_core::wire::LoginReq { email: "a@x.io".into(), auth_key: b.encode([1u8; 32]), device_name: name.into() }).unwrap();
    Api::new(base, Some(r.token))
}

#[test]
fn two_devices_converge_against_real_server() {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    Api::new(&base, None)
        .register(&sync_core::wire::RegisterReq {
            email: "a@x.io".into(),
            invite_code: code,
            salt: b64.encode([1u8; 16]),
            auth_key: b64.encode([1u8; 32]),
            wrapped_sync_key: "w".into(),
            wrapped_recovery_key: "w".into(),
            recovery_auth: b64.encode([2u8; 32]),
            pubkey: "p".into(),
            wrapped_privkey: "p".into(),
        })
        .unwrap();
    let key = [9u8; 32];
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let mut a = Engine::new(Mem { device: "a".into(), ..Default::default() }, login_device(&base, "A"), "a");
    let mut b = Engine::new(Mem { device: "b".into(), ..Default::default() }, login_device(&base, "B"), "b");

    a.store.edit("host:1", "one", now - 100);
    let r = a.tick("u", &key, 1, now).unwrap();
    assert_eq!(r.pushed, 1);
    b.tick("u", &key, 1, now).unwrap();
    assert_eq!(b.store.items["host:1"].json.as_deref(), Some(b"one".as_slice()));

    b.store.edit("host:1", "two", now - 50);
    b.tick("u", &key, 1, now).unwrap();
    a.tick("u", &key, 1, now).unwrap();
    assert_eq!(a.store.items["host:1"].json.as_deref(), Some(b"two".as_slice()));

    // the wrong key cannot read it: items are skipped, not applied
    let mut c = Engine::new(Mem { device: "c".into(), ..Default::default() }, login_device(&base, "C"), "c");
    let r = c.tick("u", &[1u8; 32], 1, now).unwrap();
    assert_eq!((r.skipped_bad, c.store.items.len()), (1, 0));
}

use sync_client::account::{login, register};
use sync_client::api::Api;
use sync_client::SyncError;
use sync_server::{create_invite, spawn_test_server};

#[test]
fn register_then_login_on_two_devices_gives_same_keys() {
    let (base, db) = spawn_test_server();
    let recovery = register(&Api::new(&base, None), "me@x.io", "a long enough password", &create_invite(&db)).unwrap();
    assert!(sync_core::recovery::parse_recovery_key(&recovery).is_some());
    let a = login(&Api::new(&base, None), "me@x.io", "a long enough password", "laptop").unwrap();
    let b = login(&Api::new(&base, None), "me@x.io", "a long enough password", "phone").unwrap();
    assert_eq!(*a.secrets.sync_key, *b.secrets.sync_key);
    assert_eq!(a.secrets.x_public, b.secrets.x_public);
    assert_ne!(a.token, b.token);
}

#[test]
fn wrong_password_is_unauthorized() {
    let (base, db) = spawn_test_server();
    register(&Api::new(&base, None), "me@x.io", "a long enough password", &create_invite(&db)).unwrap();
    let e = login(&Api::new(&base, None), "me@x.io", "another long password", "x").err().unwrap();
    assert_eq!(e, SyncError::Unauthorized);
}

#[test]
fn short_password_refused_before_any_request() {
    let (base, db) = spawn_test_server();
    let e = register(&Api::new(&base, None), "me@x.io", "short", &create_invite(&db)).err().unwrap();
    assert!(matches!(e, SyncError::Rejected(_)));
}

#[test]
fn server_down_is_a_network_error_not_a_panic() {
    let e = login(&Api::new("http://127.0.0.1:1", None), "me@x.io", "a long enough password", "x").err().unwrap();
    assert!(matches!(e, SyncError::Network(_)));
}

fn real_account() -> (String, String, sync_server::db::Db) {
    let (base, db) = spawn_test_server();
    let recovery = register(&Api::new(&base, None), "me@x.io", "a long enough password", &create_invite(&db)).unwrap();
    (base, recovery, db)
}

/// Puts one sealed item on the server with this account's sync key and returns what a reader must find.
fn push_one(base: &str, sess: &sync_client::account::Session) {
    use base64::Engine as _;
    let mut api = Api::new(base, Some(sess.token.clone()));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let meta = sync_core::ItemMeta { scope: "u".into(), id: "host:1".into(), updated_at: now - 5, deleted: false, epoch: 1 };
    let blob = sync_core::seal_item(&sess.secrets.sync_key, &meta, b"hello");
    let w = sync_core::wire::WireItem { id: "host:1".into(), updated_at: now - 5, deleted: false, epoch: 1, device: "d".into(), blob: base64::engine::general_purpose::STANDARD.encode(blob) };
    use sync_client::api::Remote;
    assert_eq!(api.push("u", vec![w]).unwrap().accepted, 1);
}

fn can_read(base: &str, sess: &sync_client::account::Session) -> bool {
    use base64::Engine as _;
    use sync_client::api::Remote;
    let p = Api::new(base, Some(sess.token.clone())).pull("u", 0, 10).unwrap();
    let w = &p.items[0].1;
    let meta = sync_core::ItemMeta { scope: "u".into(), id: w.id.clone(), updated_at: w.updated_at, deleted: w.deleted, epoch: w.epoch };
    let blob = base64::engine::general_purpose::STANDARD.decode(&w.blob).unwrap();
    sync_core::open_item(&sess.secrets.sync_key, &meta, &blob).map(|p| &**p == b"hello").unwrap_or(false)
}

#[test]
fn recovery_key_restores_access_to_all_items() {
    let (base, recovery, _db) = real_account();
    let old = login(&Api::new(&base, None), "me@x.io", "a long enough password", "laptop").unwrap();
    push_one(&base, &old);
    sync_client::account::recover(&Api::new(&base, None), "me@x.io", &recovery.to_lowercase(), "a brand new password").unwrap();
    assert_eq!(login(&Api::new(&base, None), "me@x.io", "a long enough password", "x").err(), Some(SyncError::Unauthorized));
    let new = login(&Api::new(&base, None), "me@x.io", "a brand new password", "new").unwrap();
    assert!(can_read(&base, &new));
    // the old device was signed out
    assert_eq!(Api::new(&base, Some(old.token)).devices().err(), Some(SyncError::Unauthorized));
}

#[test]
fn wrong_recovery_key_rejected() {
    let (base, recovery, _db) = real_account();
    let (other, _) = sync_core::recovery::new_recovery_key();
    let e = sync_client::account::recover(&Api::new(&base, None), "me@x.io", &other, "a brand new password").err().unwrap();
    assert!(matches!(e, SyncError::Rejected(_)));
    assert!(sync_client::account::recover(&Api::new(&base, None), "me@x.io", "nonsense", "a brand new password").is_err());
    // the real one still works afterwards
    sync_client::account::recover(&Api::new(&base, None), "me@x.io", &recovery, "a brand new password").unwrap();
}

#[test]
fn password_change_keeps_data_readable_without_reencrypting() {
    let (base, _r, _db) = real_account();
    let a = login(&Api::new(&base, None), "me@x.io", "a long enough password", "a").unwrap();
    push_one(&base, &a);
    sync_client::account::change_password(&Api::new(&base, Some(a.token.clone())), &a.secrets.sync_key, "changed password!!").unwrap();
    let b = login(&Api::new(&base, None), "me@x.io", "changed password!!", "b").unwrap();
    assert_eq!(*a.secrets.sync_key, *b.secrets.sync_key);
    assert!(can_read(&base, &b));
}

#[test]
fn other_devices_logged_out_after_password_change_and_revoked_device_stops() {
    let (base, _r, _db) = real_account();
    let a = login(&Api::new(&base, None), "me@x.io", "a long enough password", "a").unwrap();
    let b = login(&Api::new(&base, None), "me@x.io", "a long enough password", "b").unwrap();
    sync_client::account::change_password(&Api::new(&base, Some(a.token.clone())), &a.secrets.sync_key, "changed password!!").unwrap();
    assert_eq!(Api::new(&base, Some(b.token.clone())).devices().err(), Some(SyncError::Unauthorized));
    // a revoked device: the engine reports Unauthorized instead of looping
    let c = login(&Api::new(&base, None), "me@x.io", "changed password!!", "c").unwrap();
    let devs = Api::new(&base, Some(a.token.clone())).devices().unwrap();
    let cid = devs.iter().find(|d| d["current"] == false).unwrap()["id"].as_i64().unwrap();
    Api::new(&base, Some(a.token.clone())).revoke_device(cid).unwrap();
    use sync_client::api::Remote;
    assert_eq!(Api::new(&base, Some(c.token)).pull("u", 0, 10).err(), Some(SyncError::Unauthorized));
}

use serde_json::{json, Value};
use sync_server::{create_invite, spawn_test_server};

fn b64(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// A client that returns 4xx/5xx answers as ordinary replies, so tests can read their status and body.
fn http() -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().http_status_as_error(false).build())
}

/// Sends `body` with the given headers; `(status, body)`, or status 0 if the server hung up.
fn send_raw(method: &str, url: &str, headers: &[(&str, String)], body: Option<Vec<u8>>) -> (u16, Vec<u8>) {
    let mut b = ureq::http::Request::builder().method(method).uri(url);
    for (k, v) in headers {
        b = b.header(*k, v);
    }
    let res = match body {
        Some(bytes) => http().run(b.body(bytes).unwrap()),
        None => http().run(b.body(()).unwrap()),
    };
    match res {
        Ok(resp) => {
            let code = resp.status().as_u16();
            (code, resp.into_body().read_to_vec().unwrap_or_default())
        }
        // the server may hang up while an oversized body is still being sent
        Err(_) => (0, Vec::new()),
    }
}

pub fn call(method: &str, url: &str, token: Option<&str>, body: Value) -> (u16, Value) {
    let mut headers = Vec::new();
    if let Some(t) = token {
        headers.push(("Authorization", format!("Bearer {t}")));
    }
    let payload = if method == "GET" || method == "DELETE" {
        None
    } else {
        headers.push(("Content-Type", "application/json".to_string()));
        Some(serde_json::to_vec(&body).unwrap())
    };
    let (code, bytes) = send_raw(method, url, &headers, payload);
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

pub fn reg_body(email: &str, code: &str, auth: u8) -> Value {
    json!({
        "email": email, "invite_code": code, "salt": b64(&[1u8; 16]), "auth_key": b64(&[auth; 32]),
        "wrapped_sync_key": "ws", "wrapped_recovery_key": "wr", "recovery_auth": b64(&[auth.wrapping_add(100); 32]), "pubkey": "pk", "wrapped_privkey": "wp"
    })
}

fn login(base: &str, email: &str, auth: u8) -> (u16, Value) {
    call("POST", &format!("{base}/v1/login"), None, json!({"email": email, "auth_key": b64(&[auth; 32]), "device_name": "laptop"}))
}

#[test]
fn register_needs_valid_invite() {
    let (base, _db) = spawn_test_server();
    let (c, _) = call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", "WRONGCODE", 1));
    assert_eq!(c, 403);
}

#[test]
fn invite_is_single_use() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    assert_eq!(call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1)).0, 200);
    assert_eq!(call("POST", &format!("{base}/v1/register"), None, reg_body("b@x.io", &code, 2)).0, 403);
}

#[test]
fn login_ok_returns_wrapped_keys() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    let (c, v) = login(&base, "a@x.io", 1);
    assert_eq!(c, 200);
    assert_eq!(v["wrapped_sync_key"], "ws");
    assert!(v["token"].as_str().unwrap().len() > 20);
}

#[test]
fn login_wrong_key_fails_same_error_as_unknown_email() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    let wrong = login(&base, "a@x.io", 9);
    let unknown = login(&base, "nobody@x.io", 1);
    assert_eq!(wrong, unknown);
    assert_eq!(wrong.0, 401);
}

#[test]
fn salt_for_unknown_email_is_stable_and_looks_real() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    let known = call("GET", &format!("{base}/v1/account/a@x.io/salt"), None, Value::Null);
    let u1 = call("GET", &format!("{base}/v1/account/ghost@x.io/salt"), None, Value::Null);
    let u2 = call("GET", &format!("{base}/v1/account/ghost@x.io/salt"), None, Value::Null);
    assert_eq!(known.0, 200);
    assert_eq!(u1, u2);
    assert_eq!(u1.1["salt"].as_str().unwrap().len(), known.1["salt"].as_str().unwrap().len());
}

#[test]
fn five_wrong_logins_block_ip() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    for i in 0..5 {
        assert_eq!(login(&base, &format!("g{i}@x.io"), 9).0, 401);
    }
    // even the right password is refused now
    assert_eq!(login(&base, "a@x.io", 1).0, 429);
}

#[test]
fn logout_invalidates_token() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    let t = login(&base, "a@x.io", 1).1["token"].as_str().unwrap().to_string();
    assert_eq!(call("POST", &format!("{base}/v1/logout"), Some(&t), json!({})).0, 200);
    assert_eq!(call("POST", &format!("{base}/v1/logout"), Some(&t), json!({})).0, 401);
}

#[test]
fn auth_hash_stored_is_not_authkey() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    let stored: Vec<u8> = db.with(|c| c.query_row("SELECT auth_hash FROM users", [], |r| r.get(0)).unwrap());
    assert_ne!(stored, vec![1u8; 32]);
    assert!(!stored.windows(32).any(|w| w == [1u8; 32]));
}

#[test]
fn oversized_body_rejected() {
    let (base, db) = spawn_test_server();
    let code = create_invite(&db);
    let mut body = reg_body("a@x.io", &code, 1);
    body["wrapped_sync_key"] = json!("x".repeat(1_000_000));
    let code = call("POST", &format!("{base}/v1/register"), None, body).0;
    assert!(code == 413 || code == 0, "got {code}");
    // and nothing was registered
    assert_eq!(login(&base, "a@x.io", 1).0, 401);
}

fn setup_user(base: &str, db: &sync_server::db::Db, email: &str, auth: u8) {
    let code = create_invite(db);
    assert_eq!(call("POST", &format!("{base}/v1/register"), None, reg_body(email, &code, auth)).0, 200);
}
fn token(base: &str, email: &str, auth: u8) -> String {
    login(base, email, auth).1["token"].as_str().unwrap().to_string()
}

#[test]
fn list_devices_shows_name_and_last_seen() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t = token(&base, "a@x.io", 1);
    let (c, v) = call("GET", &format!("{base}/v1/devices"), Some(&t), Value::Null);
    assert_eq!(c, 200);
    assert_eq!(v[0]["name"], "laptop");
    assert!(v[0]["last_seen"].as_u64().unwrap() > 0);
    assert_eq!(v[0]["current"], true);
}

#[test]
fn revoked_device_token_rejected_next_request() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t1 = token(&base, "a@x.io", 1);
    let t2 = token(&base, "a@x.io", 1);
    let (_, v) = call("GET", &format!("{base}/v1/devices"), Some(&t1), Value::Null);
    let other = v.as_array().unwrap().iter().find(|d| d["current"] == false).unwrap()["id"].as_i64().unwrap();
    assert_eq!(call("DELETE", &format!("{base}/v1/devices/{other}"), Some(&t1), Value::Null).0, 200);
    assert_eq!(call("GET", &format!("{base}/v1/devices"), Some(&t2), Value::Null).0, 401);
    assert_eq!(call("GET", &format!("{base}/v1/devices"), Some(&t1), Value::Null).0, 200);
}

#[test]
fn cannot_revoke_other_users_device() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    setup_user(&base, &db, "b@x.io", 2);
    let ta = token(&base, "a@x.io", 1);
    let tb = token(&base, "b@x.io", 2);
    let (_, v) = call("GET", &format!("{base}/v1/devices"), Some(&tb), Value::Null);
    let bid = v[0]["id"].as_i64().unwrap();
    assert_eq!(call("DELETE", &format!("{base}/v1/devices/{bid}"), Some(&ta), Value::Null).0, 404);
    assert_eq!(call("GET", &format!("{base}/v1/devices"), Some(&tb), Value::Null).0, 200);
}

#[test]
fn password_change_revokes_other_tokens() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t1 = token(&base, "a@x.io", 1);
    let t2 = token(&base, "a@x.io", 1);
    let body = json!({"salt": b64(&[3u8; 16]), "auth_key": b64(&[7u8; 32]), "wrapped_sync_key": "new-ws"});
    assert_eq!(call("POST", &format!("{base}/v1/account/password"), Some(&t1), body).0, 200);
    assert_eq!(call("GET", &format!("{base}/v1/devices"), Some(&t2), Value::Null).0, 401);
    assert_eq!(call("GET", &format!("{base}/v1/devices"), Some(&t1), Value::Null).0, 200);
    assert_eq!(login(&base, "a@x.io", 1).0, 401);
    let (c, v) = login(&base, "a@x.io", 7);
    assert_eq!(c, 200);
    assert_eq!(v["wrapped_sync_key"], "new-ws");
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}
fn item(id: &str, t: u64, deleted: bool, blob: &str) -> Value {
    json!({"id": id, "updated_at": t, "deleted": deleted, "epoch": 1, "device": "d1", "blob": blob})
}
fn push(base: &str, tok: &str, items: Vec<Value>) -> (u16, Value) {
    call("POST", &format!("{base}/v1/push"), Some(tok), json!({"scope": "u", "items": items}))
}
fn pull(base: &str, tok: &str, since: u64, limit: u32) -> (u16, Value) {
    call("GET", &format!("{base}/v1/pull?scope=u&since={since}&limit={limit}"), Some(tok), Value::Null)
}
fn one_user() -> (String, String) {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t = token(&base, "a@x.io", 1);
    // leak db so the in-memory server keeps living for the test
    std::mem::forget(db);
    (base, t)
}

#[test]
fn push_then_pull_returns_items() {
    let (base, t) = one_user();
    let (c, r) = push(&base, &t, vec![item("host:1", 10, false, "QUJD")]);
    assert_eq!((c, r["accepted"].as_u64()), (200, Some(1)));
    let (c, p) = pull(&base, &t, 0, 100);
    assert_eq!(c, 200);
    assert_eq!(p["items"][0][1]["id"], "host:1");
    assert_eq!(p["more"], false);
}

#[test]
fn pull_since_returns_only_new() {
    let (base, t) = one_user();
    push(&base, &t, vec![item("a", 1, false, "QQ==")]);
    let (_, p1) = pull(&base, &t, 0, 100);
    let next = p1["next_seq"].as_u64().unwrap();
    push(&base, &t, vec![item("b", 2, false, "Qg==")]);
    let (_, p2) = pull(&base, &t, next, 100);
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert_eq!(p2["items"][0][1]["id"], "b");
}

#[test]
fn pull_pages_with_more_flag() {
    let (base, t) = one_user();
    let items: Vec<Value> = (0..5).map(|i| item(&format!("i{i}"), 1, false, "QQ==")).collect();
    push(&base, &t, items);
    let (_, p) = pull(&base, &t, 0, 2);
    assert_eq!((p["items"].as_array().unwrap().len(), p["more"].clone()), (2, json!(true)));
    let (_, p) = pull(&base, &t, p["next_seq"].as_u64().unwrap(), 2);
    assert_eq!(p["more"], true);
    let (_, p) = pull(&base, &t, p["next_seq"].as_u64().unwrap(), 2);
    assert_eq!((p["items"].as_array().unwrap().len(), p["more"].clone()), (1, json!(false)));
}

#[test]
fn older_update_not_applied() {
    let (base, t) = one_user();
    push(&base, &t, vec![item("a", 10, false, "bmV3")]);
    let (_, r) = push(&base, &t, vec![item("a", 5, false, "b2xk")]);
    assert_eq!(r["rejected"][0]["reason"], "not_newer");
    let (_, p) = pull(&base, &t, 0, 100);
    assert_eq!(p["items"][0][1]["blob"], "bmV3");
}

#[test]
fn tombstone_kept_and_pulled() {
    let (base, t) = one_user();
    push(&base, &t, vec![item("a", 1, false, "QQ==")]);
    push(&base, &t, vec![item("a", 2, true, "QQ==")]);
    let (_, p) = pull(&base, &t, 0, 100);
    assert_eq!(p["items"].as_array().unwrap().len(), 1);
    assert_eq!(p["items"][0][1]["deleted"], true);
}

#[test]
fn future_updated_at_rejected() {
    let (base, t) = one_user();
    let (_, r) = push(&base, &t, vec![item("a", now_ms() + 10 * 60_000, false, "QQ==")]);
    assert_eq!(r["rejected"][0]["reason"], "future");
    assert_eq!(r["accepted"], 0);
}

#[test]
fn blob_over_256k_rejected() {
    let (base, t) = one_user();
    use base64::Engine;
    let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 256 * 1024 + 1]);
    let (_, r) = push(&base, &t, vec![item("a", 1, false, &big)]);
    assert_eq!(r["rejected"][0]["reason"], "too_big");
}

#[test]
fn push_over_500_items_rejected() {
    let (base, t) = one_user();
    let items: Vec<Value> = (0..501).map(|i| item(&format!("i{i}"), 1, false, "QQ==")).collect();
    assert_eq!(push(&base, &t, items).0, 400);
}

#[test]
fn user_cannot_read_other_users_scope() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    setup_user(&base, &db, "b@x.io", 2);
    let ta = token(&base, "a@x.io", 1);
    let tb = token(&base, "b@x.io", 2);
    push(&base, &ta, vec![item("secret", 1, false, "QQ==")]);
    let (_, p) = pull(&base, &tb, 0, 100);
    assert_eq!(p["items"].as_array().unwrap().len(), 0);
    let (c, _) = call("GET", &format!("{base}/v1/pull?scope=t:1&since=0"), Some(&tb), Value::Null);
    assert_eq!(c, 403);
}

#[test]
fn server_never_modifies_blob() {
    let (base, t) = one_user();
    let blob = "AAECAwQFBgcICQoLDA0ODw==";
    push(&base, &t, vec![item("a", 1, false, blob)]);
    let (_, p) = pull(&base, &t, 0, 100);
    assert_eq!(p["items"][0][1]["blob"], blob);
}

#[test]
fn pull_with_odd_since_and_limit_is_safe() {
    let (base, t) = one_user();
    assert_eq!(call("GET", &format!("{base}/v1/pull?scope=u&since=-5"), Some(&t), Value::Null).0, 400);
    assert_eq!(call("GET", &format!("{base}/v1/pull?scope=u&since=99999999999999999999"), Some(&t), Value::Null).0, 400);
    assert_eq!(call("GET", &format!("{base}/v1/pull?scope=u&since=0&limit=999999999"), Some(&t), Value::Null).0, 200);
}

#[test]
fn recover_start_unknown_email_looks_real_and_is_stable() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let known = call("GET", &format!("{base}/v1/recover/a@x.io"), None, Value::Null);
    let u1 = call("GET", &format!("{base}/v1/recover/ghost@x.io"), None, Value::Null);
    let u2 = call("GET", &format!("{base}/v1/recover/ghost@x.io"), None, Value::Null);
    assert_eq!(known.1["wrapped_recovery_key"], "wr");
    assert_eq!(u1, u2);
    assert_ne!(u1.1["wrapped_recovery_key"], known.1["wrapped_recovery_key"]);
}

#[test]
fn recover_finish_wrong_proof_is_refused_and_counted() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let body = |proof: u8| json!({"email": "a@x.io", "recovery_auth": b64(&[proof; 32]), "salt": b64(&[5u8; 16]), "auth_key": b64(&[6u8; 32]), "wrapped_sync_key": "n"});
    for _ in 0..5 {
        assert_eq!(call("POST", &format!("{base}/v1/recover"), None, body(7)).0, 401);
    }
    // blocked now, even with the right proof (auth 1 + 100)
    assert_eq!(call("POST", &format!("{base}/v1/recover"), None, body(101)).0, 429);
}

#[test]
fn recover_finish_ok_revokes_devices_and_new_password_works() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t = token(&base, "a@x.io", 1);
    let b = json!({"email": "a@x.io", "recovery_auth": b64(&[101u8; 32]), "salt": b64(&[5u8; 16]), "auth_key": b64(&[6u8; 32]), "wrapped_sync_key": "new"});
    assert_eq!(call("POST", &format!("{base}/v1/recover"), None, b).0, 200);
    assert_eq!(call("GET", &format!("{base}/v1/devices"), Some(&t), Value::Null).0, 401);
    assert_eq!(login(&base, "a@x.io", 1).0, 401);
    let (c, v) = login(&base, "a@x.io", 6);
    assert_eq!((c, v["wrapped_sync_key"].clone()), (200, json!("new")));
}

// ---- teams ----

fn two_users() -> (String, sync_server::db::Db, String, String) {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "owner@x.io", 1);
    setup_user(&base, &db, "mem@x.io", 2);
    let (a, b) = (token(&base, "owner@x.io", 1), token(&base, "mem@x.io", 2));
    (base, db, a, b)
}
fn make_team(base: &str, tok: &str) -> i64 {
    let (c, v) = call("POST", &format!("{base}/v1/teams"), Some(tok), json!({"name": "ops", "sealed_team_key": "sealed-owner"}));
    assert_eq!(c, 200);
    v["id"].as_i64().unwrap()
}
fn add_and_accept(base: &str, owner: &str, _mem: &str, team: i64) {
    assert_eq!(call("POST", &format!("{base}/v1/teams/{team}/lookup"), Some(owner), json!({"email": "mem@x.io"})).0, 200);
    assert_eq!(call("POST", &format!("{base}/v1/teams/{team}/members"), Some(owner), json!({"email": "mem@x.io", "sealed_team_key": "sealed-mem"})).0, 200);
}
fn tpush(base: &str, tok: &str, team: i64, epoch: u32, id: &str) -> (u16, Value) {
    let it = json!({"id": id, "updated_at": now_ms(), "deleted": false, "epoch": epoch, "device": "d1", "blob": "AAAA"});
    call("POST", &format!("{base}/v1/push"), Some(tok), json!({"scope": format!("t:{team}"), "items": [it]}))
}
fn tpull(base: &str, tok: &str, team: i64) -> (u16, Value) {
    call("GET", &format!("{base}/v1/pull?scope=t:{team}&since=0&limit=100"), Some(tok), Value::Null)
}

#[test]
fn owner_creates_team() {
    let (base, _db, a, _b) = two_users();
    let id = make_team(&base, &a);
    let (c, v) = call("GET", &format!("{base}/v1/teams"), Some(&a), Value::Null);
    assert_eq!(c, 200);
    assert_eq!(v["teams"][0]["id"], id);
    assert_eq!(v["teams"][0]["owner"], true);
    assert_eq!(v["teams"][0]["epoch"], 1);
    assert_eq!(v["teams"][0]["sealed_team_key"], "sealed-owner");
}

#[test]
fn non_owner_cannot_look_up_or_add() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    let (c, _) = call("POST", &format!("{base}/v1/teams/{id}/lookup"), Some(&b), json!({"email": "owner@x.io"}));
    assert_eq!(c, 403);
}

#[test]
fn adding_a_member_makes_them_a_member_at_once() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    assert_eq!(tpull(&base, &b, id).0, 403, "not a member yet");
    add_and_accept(&base, &a, &b, id);
    assert_eq!(tpull(&base, &b, id).0, 200, "no accept step");
}

#[test]
fn lookup_alone_changes_nothing() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    let (c, v) = call("POST", &format!("{base}/v1/teams/{id}/lookup"), Some(&a), json!({"email": "mem@x.io"}));
    assert_eq!((c, v["pubkey"].as_str()), (200, Some("pk")));
    assert_eq!(tpull(&base, &b, id).0, 403);
}

#[test]
fn cannot_add_an_unknown_person_or_the_same_person_twice() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    let add = |email: &str| call("POST", &format!("{base}/v1/teams/{id}/members"), Some(&a), json!({"email": email, "sealed_team_key": "s"})).0;
    assert_eq!(add("nobody@x.io"), 404);
    add_and_accept(&base, &a, &b, id);
    assert_eq!(add("mem@x.io"), 409);
}

#[test]
fn only_the_owner_adds_members() {
    let (base, db, a, b) = two_users();
    setup_user(&base, &db, "third@x.io", 3);
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    assert_eq!(call("POST", &format!("{base}/v1/teams/{id}/members"), Some(&b), json!({"email": "third@x.io", "sealed_team_key": "s"})).0, 403);
}

#[test]
fn nobody_is_added_while_the_team_key_is_being_renewed() {
    let (base, db, a, b) = two_users();
    setup_user(&base, &db, "third@x.io", 3);
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    let uid = call("GET", &format!("{base}/v1/teams"), Some(&a), Value::Null).1["teams"][0]["members"][1]["user_id"].as_i64().unwrap();
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}/members/{uid}"), Some(&a), Value::Null).0, 200);
    let (c, _) = call("POST", &format!("{base}/v1/teams/{id}/members"), Some(&a), json!({"email": "third@x.io", "sealed_team_key": "s"}));
    assert_eq!(c, 409, "the owner still holds the old key");
}

#[test]
fn member_can_pull_push_team_scope() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    assert_eq!(tpush(&base, &a, id, 1, "host:1").0, 200);
    let (c, v) = tpull(&base, &b, id);
    assert_eq!(c, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(tpush(&base, &b, id, 1, "host:2").0, 200);
}

#[test]
fn non_member_cannot_pull_team_scope() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    assert_eq!(tpull(&base, &b, id).0, 403);
    assert_eq!(tpush(&base, &b, id, 1, "host:1").0, 403);
}

#[test]
fn team_member_list_includes_pubkeys_for_fingerprint() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    let (_, v) = call("GET", &format!("{base}/v1/teams"), Some(&b), Value::Null);
    let m = v["teams"][0]["members"].as_array().unwrap();
    assert_eq!(m.len(), 2);
    assert!(m.iter().all(|x| x["pubkey"] == "pk" && x["email"].as_str().unwrap().contains('@')));
    assert_eq!(v["teams"][0]["sealed_team_key"], "sealed-mem");
    assert_eq!(v["teams"][0]["owner"], false);
}

#[test]
fn removed_member_loses_access_and_owner_cannot_be_removed() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    let uid = call("GET", &format!("{base}/v1/teams"), Some(&a), Value::Null).1["teams"][0]["members"][1]["user_id"].as_i64().unwrap();
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}/members/{uid}"), Some(&b), Value::Null).0, 403);
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}/members/{uid}"), Some(&a), Value::Null).0, 200);
    assert_eq!(tpull(&base, &b, id).0, 403);
    let me = call("GET", &format!("{base}/v1/teams"), Some(&a), Value::Null).1["teams"][0]["members"][0]["user_id"].as_i64().unwrap();
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}/members/{me}"), Some(&a), Value::Null).0, 400);
}

#[test]
fn only_the_owner_can_delete_a_team_and_it_takes_its_records_along() {
    let (base, _db, a, b) = two_users();
    let id = make_team(&base, &a);
    add_and_accept(&base, &a, &b, id);
    assert_eq!(tpush(&base, &a, id, 1, "host:1").0, 200);
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}"), Some(&b), Value::Null).0, 403);
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}"), Some(&a), Value::Null).0, 200);
    assert_eq!(tpull(&base, &a, id).0, 403);
    assert_eq!(tpull(&base, &b, id).0, 403);
    assert!(call("GET", &format!("{base}/v1/teams"), Some(&b), Value::Null).1["teams"].as_array().unwrap().is_empty());
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{id}"), Some(&a), Value::Null).0, 403);
    // the app asks about team 0 to learn whether the server can delete teams: it must answer 403, never 404
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/0"), Some(&a), Value::Null).0, 403);
}

// ---- hardening ----

fn host_port(base: &str) -> String {
    base.trim_start_matches("http://").to_string()
}

/// Cheap deterministic noise.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

#[test]
fn garbage_json_bodies_return_4xx_not_a_crash() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let tok = token(&base, "a@x.io", 1);
    let routes = ["/v1/register", "/v1/login", "/v1/push", "/v1/account/password", "/v1/recover", "/v1/teams", "/v1/teams/1/lookup", "/v1/teams/1/members", "/v1/teams/1/rotate", "/v1/teams/1/rotate/items"];
    let shapes: Vec<Vec<u8>> = vec![b"{".to_vec(), b"[]".to_vec(), b"null".to_vec(), b"{\"email\":5}".to_vec(), b"{\"scope\":\"u\",\"items\":[{\"id\":1}]}".to_vec(), vec![0xff, 0xfe, 0x00], b"{\"items\":[]".to_vec()];
    let mut n = 0;
    for seed in 0..200u64 {
        let route = routes[(seed as usize) % routes.len()];
        let body = if seed % 3 == 0 { shapes[(seed as usize / 3) % shapes.len()].clone() } else { noise(seed, (seed as usize * 7) % 300) };
        let (code, _) = send_raw("POST", &format!("{base}{route}"), &[("Content-Type", "application/json".into()), ("Authorization", format!("Bearer {tok}"))], Some(body.clone()));
        assert_ne!(code, 0, "transport error on {route}");
        assert!((400..500).contains(&code), "{route} gave {code} for {:?}", String::from_utf8_lossy(&body));
        n += 1;
    }
    assert_eq!(n, 200);
    // the server is still fine
    assert_eq!(login(&base, "a@x.io", 1).0, 200);
}

#[test]
fn huge_header_rejected() {
    use std::io::{Read, Write};
    let (base, _db) = spawn_test_server();
    let mut s = std::net::TcpStream::connect(host_port(&base)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let req = format!("GET /v1/devices HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {}\r\n\r\n", "A".repeat(200_000));
    let _ = s.write_all(req.as_bytes());
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    assert!(n == 0 || head.contains(" 431 ") || head.contains(" 400 ") || head.contains(" 401 "), "{head}");
    assert!(!head.contains(" 200 "));
    // other clients are unaffected
    let (c, _) = call("GET", &format!("{base}/v1/devices"), Some("x"), Value::Null);
    assert_eq!(c, 401);
}

#[test]
fn login_limiter_also_counts_per_account() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;
    use tower::ServiceExt;
    let db = sync_server::db::Db::memory().unwrap();
    let code = create_invite(&db);
    let app = sync_server::app(db);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let send = |ip: &str, path: &str, body: Value| {
        let addr: SocketAddr = format!("{ip}:4000").parse().unwrap();
        let req = axum::http::Request::builder().method("POST").uri(path).header("content-type", "application/json").extension(ConnectInfo(addr)).body(Body::from(body.to_string())).unwrap();
        rt.block_on(app.clone().oneshot(req)).unwrap().status().as_u16()
    };
    assert_eq!(send("10.0.0.1", "/v1/register", reg_body("a@x.io", &code, 1)), 200);
    let wrong = |ip: &str| send(ip, "/v1/login", json!({"email": "a@x.io", "auth_key": b64(&[9u8; 32]), "device_name": "x"}));
    // five guesses, each from a different address: no single address is over the limit
    for i in 1..=5 {
        assert_eq!(wrong(&format!("10.1.0.{i}")), 401);
    }
    // the right key from yet another address is now refused: the account is locked for a while
    let right = send("10.2.0.1", "/v1/login", json!({"email": "a@x.io", "auth_key": b64(&[1u8; 32]), "device_name": "x"}));
    assert_eq!(right, 429);
}

#[test]
fn pull_with_negative_or_huge_since_safe() {
    let (base, t) = one_user();
    for since in ["-1", "99999999999999999999999", "abc", "", "18446744073709551615", "9223372036854775807"] {
        let (c, _) = call("GET", &format!("{base}/v1/pull?scope=u&since={since}&limit=10"), Some(&t), Value::Null);
        assert!(c == 200 || c == 400, "since={since:?} gave {c}");
    }
    for limit in ["0", "-5", "100000", "x"] {
        let (c, v) = call("GET", &format!("{base}/v1/pull?scope=u&since=0&limit={limit}"), Some(&t), Value::Null);
        assert!(c == 200 || c == 400, "limit={limit:?} gave {c}");
        if c == 200 {
            assert!(v["items"].as_array().unwrap().len() <= 500);
        }
    }
}

#[test]
fn backup_restores_and_serves() {
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t = token(&base, "a@x.io", 1);
    assert_eq!(push(&base, &t, vec![item("host:1", now_ms() - 5, false, "AAAA")]).0, 200);
    let out = std::env::temp_dir().join(format!("tc_backup_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&out);
    db.backup_to(&out).unwrap();
    assert!(db.backup_to(&out).is_err(), "never overwrites an existing backup");
    // a new server on the copy knows the account, the token and the data
    let restored = sync_server::db::Db::open(&out).unwrap();
    let base2 = sync_server::spawn_server_with(restored);
    assert_eq!(login(&base2, "a@x.io", 1).0, 200);
    let (c, v) = pull(&base2, &t, 0, 10);
    assert_eq!(c, 200);
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_file(&out);
}

#[test]
fn relay_ticket_needs_an_account_and_an_enabled_relay() {
    use std::io::{BufRead, BufReader, Write};
    // relay switched off: signed-in accounts get a clear 404
    let (base, db) = spawn_test_server();
    setup_user(&base, &db, "a@x.io", 1);
    let t = token(&base, "a@x.io", 1);
    assert_eq!(call("POST", &format!("{base}/v1/relay/ticket"), Some(&t), json!({})).0, 404);

    // relay on: no token, no ticket; with a token the ticket opens the relay port, once
    let db = sync_server::db::Db::memory().unwrap();
    let base = sync_server::spawn_server_relay(db.clone(), true);
    setup_user(&base, &db, "a@x.io", 1);
    assert_eq!(call("POST", &format!("{base}/v1/relay/ticket"), None, json!({})).0, 401);
    let t = token(&base, "a@x.io", 1);
    let (c, v) = call("POST", &format!("{base}/v1/relay/ticket"), Some(&t), json!({}));
    assert_eq!(c, 200);
    let (ticket, port) = (v["ticket"].as_str().unwrap().to_string(), v["port"].as_u64().unwrap());
    let host = |ticket: &str| -> Value {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        writeln!(s, "{}", json!({"t":"host","ticket":ticket})).unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    assert_eq!(host(&ticket)["t"], "hosted");
    assert_eq!(host(&ticket)["t"], "error", "a ticket is good once");
}

fn login_from(base: &str, email: &str, auth: u8, ip: &str) -> u16 {
    let body = serde_json::to_vec(&json!({"email": email, "auth_key": b64(&[auth; 32]), "device_name": "laptop"})).unwrap();
    send_raw("POST", &format!("{base}/v1/login"), &[("X-Forwarded-For", ip.to_string()), ("Content-Type", "application/json".into())], Some(body)).0
}

#[test]
fn behind_a_trusted_proxy_each_client_has_its_own_failure_count() {
    let db = sync_server::db::Db::memory().unwrap();
    let base = sync_server::spawn_server_proxy(db.clone(), true);
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    for i in 0..5 {
        assert_eq!(login_from(&base, &format!("g{i}@x.io"), 9, "203.0.113.7"), 401);
    }
    assert_eq!(login_from(&base, "a@x.io", 1, "203.0.113.7"), 429, "the guesser is blocked");
    assert_eq!(login_from(&base, "a@x.io", 1, "203.0.113.8"), 200, "someone else is not");
}

#[test]
fn without_trust_proxy_the_header_changes_nothing() {
    let db = sync_server::db::Db::memory().unwrap();
    let base = sync_server::spawn_server_proxy(db.clone(), false);
    let code = create_invite(&db);
    call("POST", &format!("{base}/v1/register"), None, reg_body("a@x.io", &code, 1));
    for i in 0..5 {
        assert_eq!(login_from(&base, &format!("g{i}@x.io"), 9, &format!("203.0.113.{i}")), 401);
    }
    assert_eq!(login_from(&base, "a@x.io", 1, "203.0.113.99"), 429, "a client cannot dodge the limit by lying");
}

// ---- roles ----

fn add_as(base: &str, by: &str, email: &str, role: Option<&str>, team: i64) -> u16 {
    let mut body = json!({"email": email, "sealed_team_key": "sealed"});
    if let Some(r) = role {
        body["role"] = json!(r);
    }
    call("POST", &format!("{base}/v1/teams/{team}/members"), Some(by), body).0
}
fn role_in_list(base: &str, tok: &str, team: i64, email: &str) -> String {
    let (_, v) = call("GET", &format!("{base}/v1/teams"), Some(tok), Value::Null);
    let t = v["teams"].as_array().unwrap().iter().find(|t| t["id"] == team).unwrap().clone();
    t["members"].as_array().unwrap().iter().find(|m| m["email"] == email).unwrap()["role"].as_str().unwrap().to_string()
}
fn set_role(base: &str, by: &str, team: i64, user: i64, role: &str) -> u16 {
    call("POST", &format!("{base}/v1/teams/{team}/members/{user}/role"), Some(by), json!({"role": role})).0
}
fn user_id(base: &str, tok: &str, team: i64, email: &str) -> i64 {
    let (_, v) = call("GET", &format!("{base}/v1/teams"), Some(tok), Value::Null);
    let t = v["teams"].as_array().unwrap().iter().find(|t| t["id"] == team).unwrap().clone();
    t["members"].as_array().unwrap().iter().find(|m| m["email"] == email).unwrap()["user_id"].as_i64().unwrap()
}

#[test]
fn a_member_added_without_a_role_is_an_editor() {
    let (base, _db, a, b) = two_users();
    let team = make_team(&base, &a);
    assert_eq!(add_as(&base, &a, "mem@x.io", None, team), 200);
    assert_eq!(role_in_list(&base, &a, team, "mem@x.io"), "editor");
    assert_eq!(tpush(&base, &b, team, 1, "i1").0, 200);
    let (_, v) = call("GET", &format!("{base}/v1/teams"), Some(&b), Value::Null);
    assert_eq!(v["teams"][0]["my_role"], "editor");
    let (_, v) = call("GET", &format!("{base}/v1/teams"), Some(&a), Value::Null);
    assert_eq!(v["teams"][0]["my_role"], "owner");
}

#[test]
fn a_viewer_reads_but_cannot_push() {
    let (base, _db, a, b) = two_users();
    let team = make_team(&base, &a);
    assert_eq!(add_as(&base, &a, "mem@x.io", Some("viewer"), team), 200);
    assert_eq!(tpush(&base, &a, team, 1, "i1").0, 200);
    let (c, v) = tpush(&base, &b, team, 1, "i2");
    assert_eq!(c, 403);
    assert_eq!(v["error"], "read_only");
    assert_eq!(tpull(&base, &b, team).0, 200);
}

#[test]
fn the_owner_changes_a_role_and_the_rights_follow() {
    let (base, _db, a, b) = two_users();
    let team = make_team(&base, &a);
    assert_eq!(add_as(&base, &a, "mem@x.io", Some("editor"), team), 200);
    let uid = user_id(&base, &a, team, "mem@x.io");
    assert_eq!(set_role(&base, &a, team, uid, "viewer"), 200);
    assert_eq!(tpush(&base, &b, team, 1, "i1").0, 403);
    assert_eq!(set_role(&base, &a, team, uid, "editor"), 200);
    assert_eq!(tpush(&base, &b, team, 1, "i1").0, 200);
    // nobody changes their own role, and the owner's role is fixed
    let me = user_id(&base, &a, team, "owner@x.io");
    assert_eq!(set_role(&base, &a, team, me, "viewer"), 400);
    assert_eq!(set_role(&base, &b, team, me, "viewer"), 403);
    assert_eq!(set_role(&base, &a, team, uid, "owner"), 403);
    assert_eq!(set_role(&base, &a, team, uid, "boss"), 403);
}

#[test]
fn an_editor_cannot_manage_the_team() {
    let (base, db, a, b) = two_users();
    setup_user(&base, &db, "third@x.io", 3);
    let team = make_team(&base, &a);
    assert_eq!(add_as(&base, &a, "mem@x.io", Some("editor"), team), 200);
    assert_eq!(call("POST", &format!("{base}/v1/teams/{team}/lookup"), Some(&b), json!({"email": "third@x.io"})).0, 403);
    assert_eq!(add_as(&base, &b, "third@x.io", Some("viewer"), team), 403);
}

#[test]
fn an_admin_adds_people_but_cannot_make_admins() {
    let (base, db, a, b) = two_users();
    setup_user(&base, &db, "third@x.io", 3);
    let team = make_team(&base, &a);
    assert_eq!(add_as(&base, &a, "mem@x.io", Some("admin"), team), 200);
    assert_eq!(call("POST", &format!("{base}/v1/teams/{team}/lookup"), Some(&b), json!({"email": "third@x.io"})).0, 200);
    assert_eq!(add_as(&base, &b, "third@x.io", Some("admin"), team), 403);
    assert_eq!(add_as(&base, &b, "third@x.io", Some("viewer"), team), 200);
    let third = user_id(&base, &a, team, "third@x.io");
    assert_eq!(set_role(&base, &b, team, third, "editor"), 200);
    assert_eq!(set_role(&base, &b, team, third, "admin"), 403);
    // an admin does not touch the owner, and only the owner removes people
    let owner = user_id(&base, &a, team, "owner@x.io");
    assert_eq!(set_role(&base, &b, team, owner, "viewer"), 403);
    assert_eq!(call("DELETE", &format!("{base}/v1/teams/{team}/members/{third}"), Some(&b), Value::Null).0, 403);
}

#[test]
fn the_old_member_role_counts_as_editor() {
    let (base, db, a, b) = two_users();
    let team = make_team(&base, &a);
    assert_eq!(add_as(&base, &a, "mem@x.io", None, team), 200);
    db.with(|c| c.execute("UPDATE members SET role='member' WHERE team_id=?1 AND role!='owner'", [team]).unwrap());
    assert_eq!(role_in_list(&base, &a, team, "mem@x.io"), "editor");
    assert_eq!(tpush(&base, &b, team, 1, "i1").0, 200);
}

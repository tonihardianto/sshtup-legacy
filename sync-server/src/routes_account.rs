use crate::auth::*;
use crate::db::now_ms;
use crate::State;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State as St};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::Instant;
use sync_core::wire::{LoginReq, LoginResp, PasswordReq, RecoverFinishReq, RecoverStartResp, RegisterReq, SaltResp};

pub fn routes() -> Router<State> {
    Router::new()
        .route("/v1/account/{email}/salt", get(salt))
        .route("/v1/register", post(register))
        .route("/v1/login", post(login))
        .route("/v1/logout", post(logout))
        .route("/v1/devices", get(devices))
        .route("/v1/devices/{id}", axum::routing::delete(revoke))
        .route("/v1/account/password", post(change_password))
        .route("/v1/recover/{email}", get(recover_start))
        .route("/v1/recover", post(recover_finish))
        .layer(DefaultBodyLimit::max(64 * 1024))
}

fn norm_email(e: &str) -> Option<String> {
    let e = e.trim().to_lowercase();
    (e.len() >= 3 && e.len() <= 254 && e.contains('@')).then_some(e)
}

fn dec(s: &str, len: usize) -> Option<Vec<u8>> {
    B64.decode(s).ok().filter(|v| v.len() == len)
}

fn server_secret(c: &rusqlite::Connection) -> Vec<u8> {
    if let Ok(v) = c.query_row("SELECT v FROM meta WHERE k='salt_secret'", [], |r| r.get::<_, Vec<u8>>(0)) {
        return v;
    }
    let mut s = [0u8; 32];
    getrandom::fill(&mut s).expect("system random");
    let _ = c.execute("INSERT OR IGNORE INTO meta(k,v) VALUES('salt_secret',?1)", [s.as_slice()]);
    s.to_vec()
}

async fn salt(St(st): St<State>, Path(email): Path<String>) -> Result<Json<SaltResp>, ApiErr> {
    let email = norm_email(&email).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_email"))?;
    let salt = st.db.with(|c| {
        c.query_row("SELECT salt FROM users WHERE email=?1", [&email], |r| r.get::<_, Vec<u8>>(0)).unwrap_or_else(|_| {
            // unknown address: a stable fake salt, so nobody can probe which accounts exist
            let mut m = server_secret(c);
            m.extend_from_slice(email.as_bytes());
            sha(&m)[..16].to_vec()
        })
    });
    Ok(Json(SaltResp { salt: B64.encode(salt) }))
}

async fn register(St(st): St<State>, ConnectInfo(addr): ConnectInfo<SocketAddr>, hd: axum::http::HeaderMap, Json(r): Json<RegisterReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let ipk = ip_key(client_ip(st.trust_proxy, addr.ip(), &hd));
    let now = Instant::now();
    if st.limiter.is_blocked(&ipk, now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts"));
    }
    let email = norm_email(&r.email).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_email"))?;
    let salt = dec(&r.salt, 16).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_salt"))?;
    let auth = dec(&r.auth_key, 32).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_auth_key"))?;
    for f in [&r.wrapped_sync_key, &r.wrapped_recovery_key, &r.pubkey, &r.wrapped_privkey] {
        if f.len() > 4096 {
            return Err(err(StatusCode::BAD_REQUEST, "field_too_long"));
        }
    }
    let code_hash = sha(r.invite_code.trim().to_uppercase().as_bytes());
    let auth_hash = hash_auth(&auth);
    let recovery_auth = dec(&r.recovery_auth, 32).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_recovery_auth"))?;
    let recovery_hash = hash_auth(&recovery_auth);
    let res = st.db.with(|c| -> Result<(), StatusCode> {
        let tx = c.transaction().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let used = tx.execute("UPDATE server_invites SET used=1 WHERE code_hash=?1 AND used=0", [&code_hash]).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if used == 0 {
            return Err(StatusCode::FORBIDDEN);
        }
        tx.execute(
            "INSERT INTO users(email,salt,auth_hash,wrapped_sync_key,wrapped_recovery_key,recovery_hash,pubkey,wrapped_privkey) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![email, salt, auth_hash, r.wrapped_sync_key, r.wrapped_recovery_key, recovery_hash, r.pubkey, r.wrapped_privkey],
        )
        .map_err(|_| StatusCode::CONFLICT)?;
        tx.commit().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    });
    match res {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(code) => {
            if code == StatusCode::FORBIDDEN {
                st.limiter.fail(&ipk, now);
            }
            Err(err(code, if code == StatusCode::FORBIDDEN { "invalid_invite" } else { "cannot_register" }))
        }
    }
}

fn dummy_hash() -> &'static Vec<u8> {
    static H: OnceLock<Vec<u8>> = OnceLock::new();
    H.get_or_init(|| hash_auth(&[0u8; 32]))
}

async fn login(St(st): St<State>, ConnectInfo(addr): ConnectInfo<SocketAddr>, hd: axum::http::HeaderMap, Json(r): Json<LoginReq>) -> Result<Json<LoginResp>, ApiErr> {
    let now = Instant::now();
    let ipk = ip_key(client_ip(st.trust_proxy, addr.ip(), &hd));
    let email = norm_email(&r.email).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "bad_credentials"))?;
    let acct = format!("acct:{email}");
    if st.limiter.is_blocked(&ipk, now) || st.limiter.is_blocked(&acct, now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts"));
    }
    let auth = dec(&r.auth_key, 32);
    type Row = (i64, Vec<u8>, String, String, String, String);
    let user: Option<Row> = st.db.with(|c| {
        c.query_row("SELECT id,auth_hash,wrapped_sync_key,wrapped_recovery_key,pubkey,wrapped_privkey FROM users WHERE email=?1", [&email], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })
        .ok()
    });
    let probe = auth.clone().unwrap_or_else(|| vec![0u8; 32]);
    let ok = match &user {
        Some(u) => verify_auth(&probe, &u.1) && auth.is_some(),
        None => {
            let _ = verify_auth(&probe, dummy_hash()); // same work whether or not the account exists
            false
        }
    };
    let Some(u) = user.filter(|_| ok) else {
        st.limiter.fail(&ipk, now);
        st.limiter.fail(&acct, now);
        return Err(err(StatusCode::UNAUTHORIZED, "bad_credentials"));
    };
    let token = new_token();
    let name: String = r.device_name.chars().take(64).collect();
    let dev = st.db.with(|c| {
        c.execute("INSERT INTO devices(user_id,name,token_hash,last_seen) VALUES(?1,?2,?3,?4)", rusqlite::params![u.0, name, sha(token.as_bytes()), now_ms() as i64]).ok()?;
        Some(c.last_insert_rowid())
    });
    let Some(dev) = dev else { return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "internal")) };
    Ok(Json(LoginResp { token, device_id: dev.to_string(), wrapped_sync_key: u.2, wrapped_recovery_key: u.3, pubkey: u.4, wrapped_privkey: u.5 }))
}

async fn logout(St(st): St<State>, h: HeaderMap) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    st.db.with(|c| c.execute("UPDATE devices SET revoked=1 WHERE id=?1", [who.device_id]).ok());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn devices(St(st): St<State>, h: HeaderMap) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let list = st.db.with(|c| {
        let mut q = c.prepare("SELECT id,name,last_seen FROM devices WHERE user_id=?1 AND revoked=0 ORDER BY id").unwrap();
        q.query_map([who.user_id], |r| {
            let id: i64 = r.get(0)?;
            Ok(serde_json::json!({ "id": id, "name": r.get::<_, String>(1)?, "last_seen": r.get::<_, i64>(2)?, "current": id == who.device_id }))
        })
        .unwrap()
        .filter_map(|x| x.ok())
        .collect::<Vec<_>>()
    });
    Ok(Json(serde_json::Value::Array(list)))
}

async fn revoke(St(st): St<State>, h: HeaderMap, Path(id): Path<i64>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let n = st.db.with(|c| c.execute("UPDATE devices SET revoked=1 WHERE id=?1 AND user_id=?2", rusqlite::params![id, who.user_id]).unwrap_or(0));
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no_such_device"));
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn change_password(St(st): St<State>, h: HeaderMap, Json(r): Json<PasswordReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let salt = dec(&r.salt, 16).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_salt"))?;
    let auth = dec(&r.auth_key, 32).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_auth_key"))?;
    if r.wrapped_sync_key.len() > 4096 {
        return Err(err(StatusCode::BAD_REQUEST, "field_too_long"));
    }
    let hash = hash_auth(&auth);
    st.db.with(|c| -> Result<(), ApiErr> {
        let tx = c.transaction().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal"))?;
        tx.execute("UPDATE users SET salt=?1, auth_hash=?2, wrapped_sync_key=?3 WHERE id=?4", rusqlite::params![salt, hash, r.wrapped_sync_key, who.user_id])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal"))?;
        tx.execute("UPDATE devices SET revoked=1 WHERE user_id=?1 AND id<>?2", rusqlite::params![who.user_id, who.device_id])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal"))?;
        tx.commit().map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal"))
    })?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// The blob that only the recovery key opens. Unknown addresses get a stable fake of the same size.
async fn recover_start(St(st): St<State>, Path(email): Path<String>) -> Result<Json<RecoverStartResp>, ApiErr> {
    let email = norm_email(&email).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_email"))?;
    let w = st.db.with(|c| {
        c.query_row("SELECT wrapped_recovery_key FROM users WHERE email=?1", [&email], |r| r.get::<_, String>(0)).unwrap_or_else(|_| {
            let mut m = server_secret(c);
            m.extend_from_slice(b"recover");
            m.extend_from_slice(email.as_bytes());
            let mut fake = Vec::new();
            for i in 0u8..3 {
                let mut x = m.clone();
                x.push(i);
                fake.extend_from_slice(&sha(&x));
            }
            B64.encode(&fake[..24 + 32 + 16])
        })
    });
    Ok(Json(RecoverStartResp { wrapped_recovery_key: w }))
}

async fn recover_finish(St(st): St<State>, ConnectInfo(addr): ConnectInfo<SocketAddr>, hd: axum::http::HeaderMap, Json(r): Json<RecoverFinishReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let now = Instant::now();
    let ipk = ip_key(client_ip(st.trust_proxy, addr.ip(), &hd));
    let email = norm_email(&r.email).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "bad_credentials"))?;
    let acct = format!("acct:{email}");
    if st.limiter.is_blocked(&ipk, now) || st.limiter.is_blocked(&acct, now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "too_many_attempts"));
    }
    let proof = dec(&r.recovery_auth, 32);
    let salt = dec(&r.salt, 16).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_salt"))?;
    let auth = dec(&r.auth_key, 32).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_auth_key"))?;
    if r.wrapped_sync_key.len() > 4096 {
        return Err(err(StatusCode::BAD_REQUEST, "field_too_long"));
    }
    let row: Option<(i64, Vec<u8>)> = st.db.with(|c| c.query_row("SELECT id, recovery_hash FROM users WHERE email=?1", [&email], |x| Ok((x.get(0)?, x.get(1)?))).ok());
    let probe = proof.clone().unwrap_or_else(|| vec![0u8; 32]);
    let ok = match &row {
        Some((_, h)) => verify_auth(&probe, h) && proof.is_some(),
        None => {
            let _ = verify_auth(&probe, dummy_hash());
            false
        }
    };
    let Some((uid, _)) = row.filter(|_| ok) else {
        st.limiter.fail(&ipk, now);
        st.limiter.fail(&acct, now);
        return Err(err(StatusCode::UNAUTHORIZED, "bad_credentials"));
    };
    let hash = hash_auth(&auth);
    st.db.with(|c| -> Result<(), ApiErr> {
        let internal = || err(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        let tx = c.transaction().map_err(|_| internal())?;
        tx.execute("UPDATE users SET salt=?1, auth_hash=?2, wrapped_sync_key=?3 WHERE id=?4", rusqlite::params![salt, hash, r.wrapped_sync_key, uid]).map_err(|_| internal())?;
        tx.execute("UPDATE devices SET revoked=1 WHERE user_id=?1", [uid]).map_err(|_| internal())?;
        tx.commit().map_err(|_| internal())
    })?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

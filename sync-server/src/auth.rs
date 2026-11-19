use crate::db::{now_ms, Db};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub type ApiErr = (StatusCode, Json<serde_json::Value>);
pub fn err(code: StatusCode, msg: &str) -> ApiErr {
    (code, Json(serde_json::json!({ "error": msg })))
}

pub const MAX_FAILS: u32 = 5;
const WINDOW: Duration = Duration::from_secs(600);
const BLOCK: Duration = Duration::from_secs(600);
const MAX_TRACKED: usize = 10_000;

struct Entry {
    fails: u32,
    since: Instant,
    blocked_until: Option<Instant>,
}

/// Counts failed logins by key ("ip:..." or "acct:...") and blocks a key after too many.
#[derive(Default)]
pub struct Limiter(Mutex<HashMap<String, Entry>>);

/// The address a request really comes from. Behind a reverse proxy on this machine every
/// connection arrives from 127.0.0.1, so with `trust_proxy` the proxy's `X-Forwarded-For` is read
/// instead, but only when the connection itself is from this machine (otherwise anyone could
/// name their own address) and only its last entry, the one the proxy added.
pub fn client_ip(trust_proxy: bool, peer: IpAddr, h: &HeaderMap) -> IpAddr {
    if trust_proxy && peer.is_loopback() {
        if let Some(ip) = h.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit(',').next()).and_then(|v| v.trim().parse::<IpAddr>().ok()) {
            return ip;
        }
    }
    peer
}

pub fn ip_key(ip: IpAddr) -> String {
    let ip = match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut o = v6.octets();
                o[8..].fill(0);
                IpAddr::V6(o.into())
            }
        },
        v4 => v4,
    };
    format!("ip:{ip}")
}

impl Limiter {
    pub fn is_blocked(&self, key: &str, now: Instant) -> bool {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match m.get(key).and_then(|e| e.blocked_until) {
            Some(u) if u > now => true,
            Some(_) => {
                m.remove(key);
                false
            }
            None => false,
        }
    }
    pub fn fail(&self, key: &str, now: Instant) {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= MAX_TRACKED {
            m.retain(|_, e| e.blocked_until.map(|u| u > now).unwrap_or(now.duration_since(e.since) < WINDOW));
        }
        let e = m.entry(key.to_string()).or_insert(Entry { fails: 0, since: now, blocked_until: None });
        if e.blocked_until.is_some() {
            return;
        }
        if now.duration_since(e.since) > WINDOW {
            e.fails = 0;
            e.since = now;
        }
        e.fails += 1;
        if e.fails >= MAX_FAILS {
            e.blocked_until = Some(now + BLOCK);
        }
    }
}

pub fn sha(b: &[u8]) -> Vec<u8> {
    Sha256::digest(b).to_vec()
}

pub fn same(a: &[u8], b: &[u8]) -> bool {
    let mut d = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        d |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    d == 0
}

/// authKey is already 256 random-looking bits, so the server-side hash can be cheap.
fn argon() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(8 * 1024, 1, 1, Some(32)).unwrap())
}

/// salt(16) | hash(32)
pub fn hash_auth(auth_key: &[u8]) -> Vec<u8> {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("system random");
    let mut out = [0u8; 32];
    argon().hash_password_into(auth_key, &salt, &mut out).expect("argon2");
    [salt.as_slice(), &out].concat()
}

pub fn verify_auth(auth_key: &[u8], stored: &[u8]) -> bool {
    if stored.len() != 48 {
        return false;
    }
    let mut out = [0u8; 32];
    if argon().hash_password_into(auth_key, &stored[..16], &mut out).is_err() {
        return false;
    }
    same(&out, &stored[16..])
}

pub fn new_token() -> String {
    let mut t = [0u8; 32];
    getrandom::fill(&mut t).expect("system random");
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &t)
}

pub struct Caller {
    pub user_id: i64,
    pub device_id: i64,
}

/// Resolves `Authorization: Bearer <token>` to a live (not revoked) device.
pub fn caller(db: &Db, h: &HeaderMap) -> Result<Caller, ApiErr> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let hash = sha(token.as_bytes());
    db.with(|c| {
        let row: Option<(i64, i64)> = c
            .query_row("SELECT id, user_id FROM devices WHERE token_hash=?1 AND revoked=0", [&hash], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        match row {
            Some((device_id, user_id)) => {
                let _ = c.execute("UPDATE devices SET last_seen=?1 WHERE id=?2", rusqlite::params![now_ms() as i64, device_id]);
                Ok(Caller { user_id, device_id })
            }
            None => Err(err(StatusCode::UNAUTHORIZED, "unauthorized")),
        }
    })
}

#[cfg(test)]
mod proxy_tests {
    use super::*;
    use axum::http::HeaderValue;

    fn hdr(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn the_forwarded_address_is_used_only_when_trusted_and_from_the_local_proxy() {
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let far: IpAddr = "198.51.100.4".parse().unwrap();
        let user: IpAddr = "203.0.113.7".parse().unwrap();
        // behind the proxy on this machine
        assert_eq!(client_ip(true, lo, &hdr("203.0.113.7")), user);
        // not switched on: the header is ignored
        assert_eq!(client_ip(false, lo, &hdr("203.0.113.7")), lo);
        // a direct connection cannot pick its own address by sending the header
        assert_eq!(client_ip(true, far, &hdr("203.0.113.7")), far);
        // only the last entry counts: it is the one the proxy itself added
        assert_eq!(client_ip(true, lo, &hdr("1.2.3.4, 203.0.113.7")), user);
        // junk or a missing header falls back to the peer
        assert_eq!(client_ip(true, lo, &hdr("not an ip")), lo);
        assert_eq!(client_ip(true, lo, &HeaderMap::new()), lo);
        assert_eq!(client_ip(true, "::1".parse().unwrap(), &hdr("2001:db8::5")), "2001:db8::5".parse::<IpAddr>().unwrap());
    }
}

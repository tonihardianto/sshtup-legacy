use crate::auth::*;
use crate::db::now_ms;
use crate::State;
use axum::extract::{DefaultBodyLimit, Query, State as St};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::Connection;
use sync_core::merge::{newer, Stamp};
use sync_core::wire::{PullResp, PushReq, PushResp, Rejected, WireItem};

pub const MAX_ITEMS: usize = 500;
pub const MAX_BLOB: usize = 256 * 1024;
const FUTURE_SLACK_MS: u64 = 5 * 60_000;

pub fn routes() -> Router<State> {
    Router::new().route("/v1/push", post(push)).route("/v1/pull", get(pull)).layer(DefaultBodyLimit::max(16 * 1024 * 1024))
}

/// Maps the scope a client asks for to the key used in the database, after checking the caller may use it.
/// Returns (db scope, team epoch if it is a team scope).
pub fn resolve_scope(c: &Connection, who: &Caller, scope: &str) -> Result<(String, Option<u32>), ApiErr> {
    if scope == "u" {
        return Ok((format!("u:{}", who.user_id), None));
    }
    let Some(id) = scope.strip_prefix("t:").and_then(|s| s.parse::<i64>().ok()) else {
        return Err(err(StatusCode::BAD_REQUEST, "bad_scope"));
    };
    let epoch: Option<u32> = c
        .query_row("SELECT t.epoch FROM teams t JOIN members m ON m.team_id=t.id WHERE t.id=?1 AND m.user_id=?2", rusqlite::params![id, who.user_id], |r| r.get(0))
        .ok();
    match epoch {
        Some(e) => Ok((format!("t:{id}"), Some(e))),
        None => Err(err(StatusCode::FORBIDDEN, "not_a_member")),
    }
}

/// Size the blob has once base64 is undone (without decoding it).
fn decoded_len(b64: &str) -> usize {
    let pad = b64.bytes().rev().take_while(|c| *c == b'=').count().min(2);
    (b64.len() / 4 * 3).saturating_sub(pad)
}

pub fn blob_ok(b64: &str) -> bool {
    decoded_len(b64) <= MAX_BLOB
}

fn reject(id: &str, reason: &str) -> Rejected {
    Rejected { id: id.to_string(), reason: reason.to_string() }
}

async fn push(St(st): St<State>, h: HeaderMap, Json(req): Json<PushReq>) -> Result<Json<PushResp>, ApiErr> {
    let who = caller(&st.db, &h)?;
    if req.items.len() > MAX_ITEMS {
        return Err(err(StatusCode::BAD_REQUEST, "too_many_items"));
    }
    st.db.with(|c| -> Result<Json<PushResp>, ApiErr> {
        let (scope, team_epoch) = resolve_scope(c, &who, &req.scope)?;
        if let Some(team) = scope.strip_prefix("t:").and_then(|s| s.parse::<i64>().ok()) {
            if !crate::routes_teams::role_of(c, team, who.user_id).map(|r| sync_core::wire::roles::can_write(&r)).unwrap_or(false) {
                return Err(err(StatusCode::FORBIDDEN, "read_only"));
            }
        }
        let now = now_ms();
        let internal = || err(StatusCode::INTERNAL_SERVER_ERROR, "internal");
        let tx = c.transaction().map_err(|_| internal())?;
        let mut resp = PushResp::default();
        for it in &req.items {
            if it.id.is_empty() || it.id.len() > 128 || it.device.len() > 128 {
                resp.rejected.push(reject(&it.id, "bad_item"));
                continue;
            }
            if decoded_len(&it.blob) > MAX_BLOB {
                resp.rejected.push(reject(&it.id, "too_big"));
                continue;
            }
            if it.updated_at > now + FUTURE_SLACK_MS {
                resp.rejected.push(reject(&it.id, "future"));
                continue;
            }
            if team_epoch.map(|e| e != it.epoch).unwrap_or(false) {
                resp.rejected.push(reject(&it.id, "stale_epoch"));
                continue;
            }
            let old: Option<(i64, bool, String)> =
                tx.query_row("SELECT updated_at,deleted,device FROM records WHERE scope=?1 AND id=?2", rusqlite::params![scope, it.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok();
            if let Some((t, d, dev)) = old {
                let a = Stamp { updated_at: it.updated_at, deleted: it.deleted, device: it.device.clone() };
                let b = Stamp { updated_at: t as u64, deleted: d, device: dev };
                if !newer(&a, &b) {
                    resp.rejected.push(reject(&it.id, "not_newer"));
                    continue;
                }
            }
            tx.execute("INSERT INTO scope_seq(scope,seq) VALUES(?1,1) ON CONFLICT(scope) DO UPDATE SET seq=seq+1", [&scope]).map_err(|_| internal())?;
            let seq: i64 = tx.query_row("SELECT seq FROM scope_seq WHERE scope=?1", [&scope], |r| r.get(0)).map_err(|_| internal())?;
            tx.execute(
                "INSERT OR REPLACE INTO records(scope,id,seq,updated_at,deleted,epoch,device,blob) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![scope, it.id, seq, it.updated_at as i64, it.deleted, it.epoch, it.device, it.blob],
            )
            .map_err(|_| internal())?;
            resp.accepted += 1;
        }
        tx.commit().map_err(|_| internal())?;
        Ok(Json(resp))
    })
}

#[derive(serde::Deserialize)]
struct PullQ {
    scope: String,
    since: Option<String>,
    limit: Option<String>,
}

async fn pull(St(st): St<State>, h: HeaderMap, Query(q): Query<PullQ>) -> Result<Json<PullResp>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let since: u64 = match &q.since {
        Some(s) => s.parse::<u64>().ok().filter(|v| *v < i64::MAX as u64).ok_or_else(|| err(StatusCode::BAD_REQUEST, "bad_since"))?,
        None => 0,
    };
    let limit: usize = match &q.limit {
        Some(s) => s.parse::<usize>().map_err(|_| err(StatusCode::BAD_REQUEST, "bad_limit"))?.clamp(1, MAX_ITEMS),
        None => 200,
    };
    st.db.with(|c| {
        let (scope, _) = resolve_scope(c, &who, &q.scope)?;
        let mut stmt = c
            .prepare("SELECT seq,id,updated_at,deleted,epoch,device,blob FROM records WHERE scope=?1 AND seq>?2 ORDER BY seq LIMIT ?3")
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal"))?;
        let mut rows: Vec<(u64, WireItem)> = stmt
            .query_map(rusqlite::params![scope, since as i64, (limit + 1) as i64], |r| {
                Ok((r.get::<_, i64>(0)? as u64, WireItem { id: r.get(1)?, updated_at: r.get::<_, i64>(2)? as u64, deleted: r.get(3)?, epoch: r.get(4)?, device: r.get(5)?, blob: r.get(6)? }))
            })
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal"))?
            .filter_map(|x| x.ok())
            .collect();
        let more = rows.len() > limit;
        rows.truncate(limit);
        let next_seq = rows.last().map(|r| r.0).unwrap_or(since);
        Ok(Json(PullResp { items: rows, next_seq, more }))
    })
}

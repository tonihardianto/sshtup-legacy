use crate::auth::*;
use crate::db::now_ms;
use crate::State;
use axum::extract::{Path, State as St};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, post};
use axum::{Json, Router};
use rusqlite::{params, Connection};
use sync_core::wire::*;

const MAX_NAME: usize = 100;
const MAX_SEALED: usize = 4096;

pub fn routes() -> Router<State> {
    Router::new()
        .route("/v1/teams", post(create).get(list))
        .route("/v1/teams/{id}", delete(delete_team))
        .route("/v1/teams/{id}/lookup", post(lookup))
        .route("/v1/teams/{id}/members", post(add_member))
        .route("/v1/teams/{id}/members/{uid}", delete(remove_member))
        .route("/v1/teams/{id}/members/{uid}/role", post(set_role))
        .route("/v1/teams/{id}/rotate", post(rotate))
        .route("/v1/teams/{id}/rotate/items", post(rotate_items))
}

fn internal() -> ApiErr {
    err(StatusCode::INTERNAL_SERVER_ERROR, "internal")
}

/// The role this user has in the team, if they are in it.
pub fn role_of(c: &Connection, team: i64, user: i64) -> Option<String> {
    c.query_row("SELECT role FROM members WHERE team_id=?1 AND user_id=?2", params![team, user], |r| r.get::<_, String>(0)).ok().map(|r| roles::normalize(&r).to_string())
}

fn manager_role(c: &Connection, team: i64, user: i64) -> Result<String, ApiErr> {
    match role_of(c, team, user) {
        Some(r) if roles::can_manage(&r) => Ok(r),
        Some(_) => Err(err(StatusCode::FORBIDDEN, "not_allowed")),
        None => Err(err(StatusCode::FORBIDDEN, "not_a_member")),
    }
}

fn is_owner(c: &Connection, team: i64, user: i64) -> bool {
    c.query_row("SELECT 1 FROM teams WHERE id=?1 AND owner_id=?2", params![team, user], |_| Ok(())).is_ok()
}

async fn create(St(st): St<State>, h: HeaderMap, Json(r): Json<TeamCreateReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let name = r.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME || r.sealed_team_key.is_empty() || r.sealed_team_key.len() > MAX_SEALED {
        return Err(err(StatusCode::BAD_REQUEST, "bad_request"));
    }
    st.db.with(|c| {
        let tx = c.transaction().map_err(|_| internal())?;
        tx.execute("INSERT INTO teams(name,owner_id,epoch) VALUES(?1,?2,1)", params![name, who.user_id]).map_err(|_| internal())?;
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO members(team_id,user_id,role,sealed_team_key,epoch) VALUES(?1,?2,'owner',?3,1)", params![id, who.user_id, r.sealed_team_key]).map_err(|_| internal())?;
        tx.commit().map_err(|_| internal())?;
        Ok(Json(serde_json::json!({ "id": id, "epoch": 1 })))
    })
}

/// Finds a person's public key so the owner can compare its fingerprint. Changes nothing.
async fn lookup(St(st): St<State>, h: HeaderMap, Path(id): Path<i64>, Json(r): Json<TeamLookupReq>) -> Result<Json<TeamLookupResp>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let email = r.email.trim().to_lowercase();
    st.db.with(|c| {
        manager_role(c, id, who.user_id)?;
        let row: Option<(i64, String)> = c.query_row("SELECT id,pubkey FROM users WHERE email=?1", [&email], |r| Ok((r.get(0)?, r.get(1)?))).ok();
        let Some((uid, pubkey)) = row else { return Err(err(StatusCode::NOT_FOUND, "no_such_user")) };
        let already: bool = c.query_row("SELECT 1 FROM members WHERE team_id=?1 AND user_id=?2", params![id, uid], |_| Ok(())).is_ok();
        if already {
            return Err(err(StatusCode::CONFLICT, "already_member"));
        }
        Ok(Json(TeamLookupResp { pubkey }))
    })
}

/// The owner or an admin adds a person: the team key, sealed to that person's public key, makes them a member at once.
async fn add_member(St(st): St<State>, h: HeaderMap, Path(id): Path<i64>, Json(r): Json<TeamAddMemberReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    let email = r.email.trim().to_lowercase();
    if r.sealed_team_key.is_empty() || r.sealed_team_key.len() > MAX_SEALED {
        return Err(err(StatusCode::BAD_REQUEST, "bad_request"));
    }
    let role = r.role.as_deref().map(|x| if x == "member" { "editor" } else { x }).unwrap_or("editor").to_string();
    st.db.with(|c| {
        let mine = manager_role(c, id, who.user_id)?;
        if !roles::can_grant(&mine, &role) {
            return Err(err(StatusCode::FORBIDDEN, "not_allowed"));
        }
        let uid: i64 = c.query_row("SELECT id FROM users WHERE email=?1", [&email], |r| r.get(0)).map_err(|_| err(StatusCode::NOT_FOUND, "no_such_user"))?;
        let epoch: u32 = c.query_row("SELECT epoch FROM teams WHERE id=?1", [id], |r| r.get(0)).map_err(|_| internal())?;
        // after a removal the owner holds the old key until the renewal is done; a key sealed now would be the wrong one
        let owner_epoch: u32 = c.query_row("SELECT epoch FROM members WHERE team_id=?1 AND user_id=?2", params![id, who.user_id], |r| r.get(0)).map_err(|_| internal())?;
        if owner_epoch != epoch {
            return Err(err(StatusCode::CONFLICT, "rotation_pending"));
        }
        let n = c.execute("INSERT OR IGNORE INTO members(team_id,user_id,role,sealed_team_key,epoch) VALUES(?1,?2,?3,?4,?5)", params![id, uid, role, r.sealed_team_key, epoch]).map_err(|_| internal())?;
        if n == 0 {
            return Err(err(StatusCode::CONFLICT, "already_member"));
        }
        Ok(Json(serde_json::json!({ "ok": true })))
    })
}

async fn list(St(st): St<State>, h: HeaderMap) -> Result<Json<TeamsResp>, ApiErr> {
    let who = caller(&st.db, &h)?;
    st.db.with(|c| {
        let mut out = TeamsResp::default();
        let mut q = c
            .prepare("SELECT t.id,t.name,t.owner_id,t.epoch,m.sealed_team_key FROM teams t JOIN members m ON m.team_id=t.id WHERE m.user_id=?1 ORDER BY t.id")
            .map_err(|_| internal())?;
        let rows: Vec<(i64, String, i64, u32, String)> = q.query_map([who.user_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map_err(|_| internal())?.flatten().collect();
        for (id, name, owner, epoch, sealed) in rows {
            let (key_epoch, prev): (u32, Option<String>) = c.query_row("SELECT epoch,prev_sealed_team_key FROM members WHERE team_id=?1 AND user_id=?2", params![id, who.user_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| internal())?;
            let stale_items = if owner == who.user_id { stale_count(c, id) } else { 0 };
            let mut mq = c.prepare("SELECT u.id,u.email,m.role,u.pubkey FROM members m JOIN users u ON u.id=m.user_id WHERE m.team_id=?1 ORDER BY u.id").map_err(|_| internal())?;
            let members = mq.query_map([id], |r| Ok(MemberInfo { user_id: r.get(0)?, email: r.get(1)?, role: roles::normalize(&r.get::<_, String>(2)?).to_string(), pubkey: r.get(3)? })).map_err(|_| internal())?.flatten().collect();
            out.teams.push(TeamInfo { id, name, owner: owner == who.user_id, my_role: role_of(c, id, who.user_id).unwrap_or_default(), epoch, key_epoch, sealed_team_key: sealed, prev_sealed_team_key: if owner == who.user_id { prev } else { None }, stale_items, members });
        }
        Ok(Json(out))
    })
}

/// The owner closes the team: members, shared records and the team itself go. The owner's client moves the items back to
/// Personal first; whatever is still in the team's scope is gone with it.
async fn delete_team(St(st): St<State>, h: HeaderMap, Path(id): Path<i64>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    st.db.with(|c| {
        if !is_owner(c, id, who.user_id) {
            return Err(err(StatusCode::FORBIDDEN, "not_owner"));
        }
        let scope = format!("t:{id}");
        let tx = c.transaction().map_err(|_| internal())?;
        tx.execute("DELETE FROM records WHERE scope=?1", [&scope]).map_err(|_| internal())?;
        tx.execute("DELETE FROM scope_seq WHERE scope=?1", [&scope]).map_err(|_| internal())?;
        tx.execute("DELETE FROM members WHERE team_id=?1", [id]).map_err(|_| internal())?;
        tx.execute("DELETE FROM teams WHERE id=?1", [id]).map_err(|_| internal())?;
        tx.commit().map_err(|_| internal())?;
        Ok(Json(serde_json::json!({ "ok": true })))
    })
}

async fn remove_member(St(st): St<State>, h: HeaderMap, Path((id, uid)): Path<(i64, i64)>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    st.db.with(|c| {
        if !is_owner(c, id, who.user_id) {
            return Err(err(StatusCode::FORBIDDEN, "not_owner"));
        }
        if uid == who.user_id {
            return Err(err(StatusCode::BAD_REQUEST, "cannot_remove_owner"));
        }
        if stale_count(c, id) > 0 {
            return Err(err(StatusCode::CONFLICT, "rotation_pending"));
        }
        let tx = c.transaction().map_err(|_| internal())?;
        let n = tx.execute("DELETE FROM members WHERE team_id=?1 AND user_id=?2", params![id, uid]).map_err(|_| internal())?;
        if n > 0 {
            // from now on only items sealed for the new epoch are accepted
            tx.execute("UPDATE teams SET epoch=epoch+1 WHERE id=?1", [id]).map_err(|_| internal())?;
        }
        tx.commit().map_err(|_| internal())?;
        Ok(Json(serde_json::json!({ "ok": true })))
    })
}

/// Changes what a member may do. The owner can set any role below owner; an admin only moves editors and viewers
/// between editor and viewer. Nobody changes the owner's role.
async fn set_role(St(st): St<State>, h: HeaderMap, Path((id, uid)): Path<(i64, i64)>, Json(r): Json<TeamSetRoleReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    st.db.with(|c| {
        let mine = manager_role(c, id, who.user_id)?;
        if uid == who.user_id {
            return Err(err(StatusCode::BAD_REQUEST, "cannot_change_own_role"));
        }
        let Some(theirs) = role_of(c, id, uid) else { return Err(err(StatusCode::NOT_FOUND, "no_such_member")) };
        if theirs == roles::OWNER || !roles::can_grant(&mine, &r.role) || (roles::normalize(&mine) == roles::ADMIN && theirs == roles::ADMIN) {
            return Err(err(StatusCode::FORBIDDEN, "not_allowed"));
        }
        c.execute("UPDATE members SET role=?3 WHERE team_id=?1 AND user_id=?2", params![id, uid, r.role]).map_err(|_| internal())?;
        Ok(Json(serde_json::json!({ "ok": true })))
    })
}

/// Items of this team that are still sealed for an older epoch.
fn stale_count(c: &Connection, team: i64) -> u32 {
    c.query_row("SELECT COUNT(*) FROM records r JOIN teams t ON ?1=t.id WHERE r.scope=?2 AND r.epoch<t.epoch", params![team, format!("t:{team}")], |r| r.get(0)).unwrap_or(0)
}

/// Step 1 of a rotation: the owner hands every remaining member the new key (sealed to them). Atomic.
async fn rotate(St(st): St<State>, h: HeaderMap, Path(id): Path<i64>, Json(r): Json<RotateReq>) -> Result<Json<serde_json::Value>, ApiErr> {
    let who = caller(&st.db, &h)?;
    if r.members.iter().any(|m| m.sealed_team_key.is_empty() || m.sealed_team_key.len() > MAX_SEALED) {
        return Err(err(StatusCode::BAD_REQUEST, "bad_request"));
    }
    st.db.with(|c| {
        if !is_owner(c, id, who.user_id) {
            return Err(err(StatusCode::FORBIDDEN, "not_owner"));
        }
        let epoch: u32 = c.query_row("SELECT epoch FROM teams WHERE id=?1", [id], |r| r.get(0)).map_err(|_| internal())?;
        if r.epoch != epoch {
            return Err(err(StatusCode::CONFLICT, "wrong_epoch"));
        }
        let mut current: Vec<i64> = c.prepare("SELECT user_id FROM members WHERE team_id=?1").and_then(|mut q| q.query_map([id], |r| r.get(0))?.collect()).map_err(|_| internal())?;
        let mut given: Vec<i64> = r.members.iter().map(|m| m.user_id).collect();
        current.sort();
        given.sort();
        if current != given {
            return Err(err(StatusCode::BAD_REQUEST, "members_mismatch"));
        }
        let owner_epoch: u32 = c.query_row("SELECT epoch FROM members WHERE team_id=?1 AND user_id=?2", params![id, who.user_id], |r| r.get(0)).map_err(|_| internal())?;
        if owner_epoch == epoch {
            return Err(err(StatusCode::CONFLICT, "already_rotated"));
        }
        let tx = c.transaction().map_err(|_| internal())?;
        // the owner keeps the previous key until every item has been re-sealed
        tx.execute("UPDATE members SET prev_sealed_team_key=sealed_team_key WHERE team_id=?1 AND user_id=?2", params![id, who.user_id]).map_err(|_| internal())?;
        for m in &r.members {
            tx.execute("UPDATE members SET sealed_team_key=?1, epoch=?2 WHERE team_id=?3 AND user_id=?4", params![m.sealed_team_key, epoch, id, m.user_id]).map_err(|_| internal())?;
        }
        tx.commit().map_err(|_| internal())?;
        Ok(Json(serde_json::json!({ "ok": true })))
    })
}

/// Step 2: the owner replaces old-epoch blobs with ones sealed for the new epoch. Can be called many times.
async fn rotate_items(St(st): St<State>, h: HeaderMap, Path(id): Path<i64>, Json(r): Json<RotateItemsReq>) -> Result<Json<RotateItemsResp>, ApiErr> {
    let who = caller(&st.db, &h)?;
    if r.items.len() > crate::routes_records::MAX_ITEMS {
        return Err(err(StatusCode::BAD_REQUEST, "too_many_items"));
    }
    st.db.with(|c| {
        if !is_owner(c, id, who.user_id) {
            return Err(err(StatusCode::FORBIDDEN, "not_owner"));
        }
        let epoch: u32 = c.query_row("SELECT epoch FROM teams WHERE id=?1", [id], |r| r.get(0)).map_err(|_| internal())?;
        let owner_epoch: u32 = c.query_row("SELECT epoch FROM members WHERE team_id=?1 AND user_id=?2", params![id, who.user_id], |r| r.get(0)).map_err(|_| internal())?;
        if r.epoch != epoch || owner_epoch != epoch {
            return Err(err(StatusCode::CONFLICT, "wrong_epoch"));
        }
        let scope = format!("t:{id}");
        let tx = c.transaction().map_err(|_| internal())?;
        let mut out = RotateItemsResp::default();
        for it in &r.items {
            let old: Option<(i64, bool, u32)> = tx.query_row("SELECT updated_at,deleted,epoch FROM records WHERE scope=?1 AND id=?2", params![scope, it.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok();
            let ok = matches!(old, Some((t, d, e)) if t as u64 == it.updated_at && d == it.deleted && e < epoch && it.epoch == epoch) && crate::routes_records::blob_ok(&it.blob);
            if !ok {
                out.skipped += 1;
                continue;
            }
            tx.execute("INSERT INTO scope_seq(scope,seq) VALUES(?1,1) ON CONFLICT(scope) DO UPDATE SET seq=seq+1", [&scope]).map_err(|_| internal())?;
            let seq: i64 = tx.query_row("SELECT seq FROM scope_seq WHERE scope=?1", [&scope], |r| r.get(0)).map_err(|_| internal())?;
            tx.execute("UPDATE records SET blob=?1, epoch=?2, seq=?3 WHERE scope=?4 AND id=?5", params![it.blob, epoch, seq, scope, it.id]).map_err(|_| internal())?;
            out.updated += 1;
        }
        if stale_count(&tx, id) == 0 {
            tx.execute("UPDATE members SET prev_sealed_team_key=NULL WHERE team_id=?1", [id]).map_err(|_| internal())?;
        }
        tx.commit().map_err(|_| internal())?;
        Ok(Json(out))
    })
}

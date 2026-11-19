//! Teams from the client's side: the team key is random, and each member holds a copy sealed to their own
//! public key. The server stores the sealed copies and never sees the team key.
use crate::api::Api;
use crate::SyncError;
use base64::Engine as _;
use sync_core::boxes::{fingerprint, keypair_from_secret, open_sealed, seal_to};
use crate::api::Remote;
use sync_core::seal::{open_item, seal_item, ItemMeta};
use sync_core::wire::{roles, RotateItemsReq, RotateMember, RotateReq, TeamAddMemberReq, TeamCreateReq, WireItem};
use zeroize::Zeroizing;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}
fn bad(e: &str) -> SyncError {
    SyncError::Bad(e.to_string())
}

/// A team this account belongs to, with its key opened.
#[derive(Clone, Debug)]
pub struct TeamView {
    pub id: i64,
    pub name: String,
    pub owner: bool,
    /// this account's role: owner, admin, editor or viewer
    pub role: String,
    pub epoch: u32,
    /// the epoch `key` belongs to; behind `epoch` until the owner has rotated the key after a removal
    pub key_epoch: u32,
    pub key: Option<Zeroizing<[u8; 32]>>,
    /// owner only, while a rotation is unfinished
    pub prev_key: Option<Zeroizing<[u8; 32]>>,
    pub stale_items: u32,
    pub members: Vec<MemberView>,
}

impl TeamView {
    /// May add, change and delete the team's items.
    pub fn can_write(&self) -> bool {
        roles::can_write(&self.role)
    }
    /// May add people and change roles.
    pub fn can_manage(&self) -> bool {
        roles::can_manage(&self.role)
    }
    /// The key may be used for the team's items right now.
    pub fn ready(&self) -> bool {
        self.key.is_some() && self.key_epoch == self.epoch
    }
}

#[derive(Clone, Debug)]
pub struct MemberView {
    pub user_id: i64,
    pub email: String,
    pub role: String,
    pub fingerprint: String,
    pub pubkey: [u8; 32],
}

/// Who is about to get the team key. Show `fingerprint` to the owner and let them compare it
/// with what that person sees on their own screen before the key is sent.
#[derive(Clone, Debug)]
pub struct MemberLookup {
    pub email: String,
    pub fingerprint: String,
    pub(crate) pubkey: [u8; 32],
}

fn pubkey_of(text: &str) -> Result<[u8; 32], SyncError> {
    b64().decode(text).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).ok_or_else(|| bad("public key"))
}

pub fn own_fingerprint(x_public: &[u8; 32]) -> String {
    fingerprint(x_public)
}

/// Makes a team. Returns its id; the key is read back with `my_teams`.
pub fn create_team(api: &Api, x_public: &[u8; 32], name: &str) -> Result<i64, SyncError> {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *key).map_err(|_| bad("no randomness"))?;
    api.create_team(&TeamCreateReq { name: name.trim().to_string(), sealed_team_key: b64().encode(seal_to(x_public, &*key)) })
}

/// Step 1 of adding someone: finds their public key and returns its fingerprint. Nothing secret is sent yet.
pub fn lookup(api: &Api, team: i64, email: &str) -> Result<MemberLookup, SyncError> {
    let email = email.trim().to_lowercase();
    let r = api.team_lookup(team, &email)?;
    let pubkey = pubkey_of(&r.pubkey)?;
    Ok(MemberLookup { email, fingerprint: fingerprint(&pubkey), pubkey })
}

/// Step 2: the owner or an admin seals the team key to that person's public key. They are a member from this moment.
pub fn add_member(api: &Api, team: i64, who: &MemberLookup, team_key: &[u8; 32], role: &str) -> Result<(), SyncError> {
    api.team_add_member(team, &TeamAddMemberReq { email: who.email.clone(), sealed_team_key: b64().encode(seal_to(&who.pubkey, team_key)), role: Some(role.to_string()) })
}

/// Changes what a member may do (`admin`, `editor` or `viewer`).
pub fn set_role(api: &Api, team: i64, user_id: i64, role: &str) -> Result<(), SyncError> {
    api.team_set_role(team, user_id, role)
}

/// The teams this account is in, with keys opened using the account's X25519 secret.
pub fn my_teams(api: &Api, x_secret: &[u8; 32]) -> Result<Vec<TeamView>, SyncError> {
    let kp = keypair_from_secret(x_secret);
    let r = api.teams()?;
    let teams = r
        .teams
        .into_iter()
        .map(|t| {
            let open = |s: &str| b64().decode(s).ok().and_then(|b| open_sealed(&kp, &b).ok()).and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok()).map(Zeroizing::new);
            let key = open(&t.sealed_team_key);
            let prev_key = t.prev_sealed_team_key.as_deref().and_then(open);
            let members = t
                .members
                .into_iter()
                .map(|m| {
                    let pubkey = pubkey_of(&m.pubkey).unwrap_or([0; 32]);
                    MemberView { fingerprint: fingerprint(&pubkey), pubkey, user_id: m.user_id, email: m.email, role: m.role }
                })
                .collect();
            TeamView { id: t.id, name: t.name, owner: t.owner, role: if t.my_role.is_empty() { if t.owner { roles::OWNER.into() } else { roles::EDITOR.into() } } else { roles::normalize(&t.my_role).into() }, epoch: t.epoch, key_epoch: t.key_epoch, key, prev_key, stale_items: t.stale_items, members }
        })
        .collect();
    Ok(teams)
}

const ROTATE_BATCH: usize = 100;

/// Re-seals every item that is still under an older key. Safe to run again after an interruption:
/// it only touches items whose epoch is behind. `batch` and `max_batches` exist for tests.
/// Returns how many items were re-sealed in this call.
pub fn finish_rotation(api: &Api, team: i64, old_key: &[u8; 32], new_key: &[u8; 32], epoch: u32, batch: usize, max_batches: Option<usize>) -> Result<u32, SyncError> {
    let scope = format!("t:{team}");
    let mut remote = api.clone();
    let mut todo: Vec<WireItem> = Vec::new();
    let mut cursor = 0;
    loop {
        let page = remote.pull(&scope, cursor, 200)?;
        for (_, w) in &page.items {
            if w.epoch >= epoch {
                continue;
            }
            let old = ItemMeta { scope: scope.clone(), id: w.id.clone(), updated_at: w.updated_at, deleted: w.deleted, epoch: w.epoch };
            let Some(plain) = b64().decode(&w.blob).ok().and_then(|b| open_item(old_key, &old, &b).ok()) else { continue };
            let new = ItemMeta { epoch, ..old };
            todo.push(WireItem { epoch, blob: b64().encode(seal_item(new_key, &new, &plain)), ..w.clone() });
        }
        if !page.more {
            break;
        }
        cursor = page.next_seq;
    }
    let mut done = 0;
    for (n, chunk) in todo.chunks(batch.max(1)).enumerate() {
        if max_batches.map(|m| n >= m).unwrap_or(false) {
            break;
        }
        done += api.rotate_items(team, &RotateItemsReq { epoch, items: chunk.to_vec() })?.updated;
    }
    Ok(done)
}

/// Owner: takes someone out of the team and moves everything to a fresh key they never had.
pub fn remove_member_and_rotate(api: &Api, team: &TeamView, user_id: i64, x_secret: &[u8; 32]) -> Result<(), SyncError> {
    let old_key = team.key.clone().filter(|_| team.ready()).ok_or_else(|| bad("team key"))?;
    api.remove_member(team.id, user_id)?;
    resume_rotation(api, team.id, x_secret, Some(&old_key))
}

/// Owner: continues a rotation that was started but not finished (step 1 and/or step 2). `old_key` is only needed
/// right after a removal; later the server hands the owner the previous key back.
pub fn resume_rotation(api: &Api, team_id: i64, x_secret: &[u8; 32], old_key: Option<&[u8; 32]>) -> Result<(), SyncError> {
    let teams = my_teams(api, x_secret)?;
    let t = teams.into_iter().find(|t| t.id == team_id).ok_or_else(|| bad("team"))?;
    if !t.owner {
        return Err(bad("only the owner can rotate"));
    }
    let mut new_key = Zeroizing::new([0u8; 32]);
    if t.key_epoch < t.epoch {
        getrandom::fill(&mut *new_key).map_err(|_| bad("no randomness"))?;
        let members = t.members.iter().map(|m| RotateMember { user_id: m.user_id, sealed_team_key: b64().encode(seal_to(&m.pubkey, &*new_key)) }).collect();
        api.rotate(team_id, &RotateReq { epoch: t.epoch, members })?;
    }
    // read it back, so that a resumed run uses the key that is on the server and not a fresh one
    let teams = my_teams(api, x_secret)?;
    let t = teams.into_iter().find(|t| t.id == team_id).ok_or_else(|| bad("team"))?;
    let (Some(new_key), true) = (t.key.clone(), t.ready()) else { return Err(bad("team key")) };
    let prev = t.prev_key.clone().or_else(|| old_key.map(|k| Zeroizing::new(*k)));
    if t.stale_items > 0 {
        let prev = prev.ok_or_else(|| bad("previous team key"))?;
        finish_rotation(api, team_id, &prev, &new_key, t.epoch, ROTATE_BATCH, None)?;
    }
    Ok(())
}

/// Whether the server knows how to delete a team. Asks to delete team 0, which never exists: a server that can
/// answers "not the owner" (403), an older one has no such route (404 or 405).
pub fn can_delete_teams(api: &Api) -> Result<bool, SyncError> {
    match api.delete_team(0) {
        Ok(()) => Ok(true),
        Err(SyncError::Rejected(e)) if e.starts_with("403") => Ok(true),
        Err(SyncError::Rejected(e)) if e.starts_with("404") || e.starts_with("405") => Ok(false),
        Err(e) => Err(e),
    }
}

//! JSON shapes that travel between the app and the sync server.
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WireItem {
    pub id: String,
    pub updated_at: u64,
    pub deleted: bool,
    pub epoch: u32,
    pub device: String,
    /// base64 of the sealed blob
    pub blob: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PushReq {
    pub scope: String,
    pub items: Vec<WireItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub id: String,
    /// future | too_big | stale_epoch | not_newer
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PushResp {
    pub accepted: u32,
    pub rejected: Vec<Rejected>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PullResp {
    pub items: Vec<(u64, WireItem)>,
    pub next_seq: u64,
    pub more: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RegisterReq {
    pub email: String,
    pub invite_code: String,
    /// base64 of the 16-byte account salt
    pub salt: String,
    /// base64 of authKey
    pub auth_key: String,
    pub wrapped_sync_key: String,
    pub wrapped_recovery_key: String,
    /// base64 of a value derived from the recovery key; proves possession of it when resetting the password
    pub recovery_auth: String,
    pub pubkey: String,
    pub wrapped_privkey: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecoverStartResp {
    pub wrapped_recovery_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecoverFinishReq {
    pub email: String,
    pub recovery_auth: String,
    pub salt: String,
    pub auth_key: String,
    pub wrapped_sync_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PasswordReq {
    pub salt: String,
    pub auth_key: String,
    pub wrapped_sync_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LoginReq {
    pub email: String,
    pub auth_key: String,
    pub device_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LoginResp {
    pub token: String,
    pub device_id: String,
    pub wrapped_sync_key: String,
    pub wrapped_recovery_key: String,
    pub pubkey: String,
    pub wrapped_privkey: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SaltResp {
    pub salt: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pull_resp_round_trips_json() {
        let it = WireItem { id: "host:1".into(), updated_at: 3, deleted: false, epoch: 1, device: "d".into(), blob: "AA==".into() };
        let r = PullResp { items: vec![(7, it.clone())], next_seq: 7, more: false };
        let back: PullResp = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.items[0], (7, it));
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TeamCreateReq {
    pub name: String,
    pub sealed_team_key: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TeamLookupReq {
    pub email: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TeamLookupResp {
    pub pubkey: String,
}
/// The owner seals the team key to a person's public key; that person is a member from then on.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TeamAddMemberReq {
    pub email: String,
    pub sealed_team_key: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MemberInfo {
    pub user_id: i64,
    pub email: String,
    pub role: String,
    pub pubkey: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TeamInfo {
    pub id: i64,
    pub name: String,
    pub owner: bool,
    /// the team's current epoch (raised when someone is removed)
    pub epoch: u32,
    /// the epoch the sealed key below belongs to; behind `epoch` until the owner has rotated the key
    pub key_epoch: u32,
    pub sealed_team_key: String,
    /// owner only, while a rotation is unfinished: the previous key, needed to re-encrypt old items
    #[serde(default)]
    pub prev_sealed_team_key: Option<String>,
    /// owner only: items still sealed with an older key
    #[serde(default)]
    pub stale_items: u32,
    pub members: Vec<MemberInfo>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RotateMember {
    pub user_id: i64,
    pub sealed_team_key: String,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RotateReq {
    pub epoch: u32,
    pub members: Vec<RotateMember>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RotateItemsReq {
    pub epoch: u32,
    pub items: Vec<WireItem>,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RotateItemsResp {
    pub updated: u32,
    pub skipped: u32,
}
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TeamsResp {
    pub teams: Vec<TeamInfo>,
}

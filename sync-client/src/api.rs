use crate::SyncError;
use sync_core::wire::{RotateItemsReq, RotateItemsResp, RotateReq, TeamAddMemberReq, TeamCreateReq, TeamLookupReq, TeamLookupResp, TeamSetRoleReq, TeamsResp, LoginReq, LoginResp, PasswordReq, PullResp, PushReq, PushResp, RecoverFinishReq, RecoverStartResp, RegisterReq, SaltResp, WireItem};

pub struct RelayPass {
    pub ticket: String,
    pub addr: String,
}

/// The host part of `https://sync.example.com:8443/x` (IPv6 stays in brackets).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|x| x.1).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?;
    let host = if auth.starts_with('[') { auth.split_once(']').map(|(h, _)| format!("{h}]"))? } else { auth.split(':').next()?.to_string() };
    if host.is_empty() { None } else { Some(host) }
}

pub trait Remote {
    fn push(&mut self, scope: &str, items: Vec<WireItem>) -> Result<PushResp, SyncError>;
    fn pull(&mut self, scope: &str, since: u64, limit: u32) -> Result<PullResp, SyncError>;
}

#[derive(Clone)]
pub struct Api {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

/// What came back: the status code and the body (read in full, whatever the status).
struct Reply {
    status: u16,
    body: Vec<u8>,
}

/// An agent that hands back 4xx/5xx answers as ordinary replies, so their bodies can be read.
fn agent(timeout: std::time::Duration) -> ureq::Agent {
    ureq::Agent::new_with_config(ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build())
}

fn network(e: ureq::Error) -> SyncError {
    SyncError::Network(e.to_string())
}

impl Reply {
    /// The body of a successful reply; a refusal becomes the matching error.
    fn ok(self) -> Result<Vec<u8>, SyncError> {
        match self.status {
            200..=299 => Ok(self.body),
            401 => Err(SyncError::Unauthorized),
            c => Err(SyncError::Rejected(format!("{c} {}", String::from_utf8_lossy(&self.body).chars().take(120).collect::<String>()))),
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: Vec<u8>) -> Result<T, SyncError> {
    serde_json::from_slice(&body).map_err(|e| SyncError::Bad(e.to_string()))
}

impl Api {
    pub fn new(base: &str, token: Option<String>) -> Api {
        Api { base: base.trim_end_matches('/').to_string(), token, agent: agent(std::time::Duration::from_secs(20)) }
    }
    fn send(&self, agent: &ureq::Agent, method: &str, path: &str, body: Option<Vec<u8>>) -> Result<Reply, SyncError> {
        let mut b = ureq::http::Request::builder().method(method).uri(format!("{}{}", self.base, path));
        if let Some(t) = &self.token {
            b = b.header("Authorization", format!("Bearer {t}"));
        }
        let res = match body {
            Some(bytes) => agent.run(b.header("Content-Type", "application/json").body(bytes).map_err(|e| SyncError::Bad(e.to_string()))?),
            None => agent.run(b.body(()).map_err(|e| SyncError::Bad(e.to_string()))?),
        };
        let resp = res.map_err(network)?;
        let status = resp.status().as_u16();
        let body = resp.into_body().with_config().limit(64 * 1024 * 1024).read_to_vec().map_err(network)?;
        Ok(Reply { status, body })
    }
    fn get(&self, path: &str) -> Result<Vec<u8>, SyncError> {
        self.send(&self.agent, "GET", path, None)?.ok()
    }
    fn delete(&self, path: &str) -> Result<Vec<u8>, SyncError> {
        self.send(&self.agent, "DELETE", path, None)?.ok()
    }
    fn post<B: serde::Serialize>(&self, path: &str, body: &B) -> Result<Vec<u8>, SyncError> {
        let bytes = serde_json::to_vec(body).map_err(|e| SyncError::Bad(e.to_string()))?;
        self.send(&self.agent, "POST", path, Some(bytes))?.ok()
    }
    pub fn salt(&self, email: &str) -> Result<SaltResp, SyncError> {
        parse(self.get(&format!("/v1/account/{email}/salt"))?)
    }
    pub fn register(&self, r: &RegisterReq) -> Result<(), SyncError> {
        self.post("/v1/register", r).map(|_| ())
    }
    pub fn login(&self, r: &LoginReq) -> Result<LoginResp, SyncError> {
        parse(self.post("/v1/login", r)?)
    }
    pub fn logout(&self) -> Result<(), SyncError> {
        self.post("/v1/logout", &serde_json::json!({})).map(|_| ())
    }
    pub fn recover_start(&self, email: &str) -> Result<RecoverStartResp, SyncError> {
        parse(self.get(&format!("/v1/recover/{email}"))?)
    }
    pub fn recover_finish(&self, r: &RecoverFinishReq) -> Result<(), SyncError> {
        self.post("/v1/recover", r).map(|_| ())
    }
    pub fn change_password(&self, r: &PasswordReq) -> Result<(), SyncError> {
        self.post("/v1/account/password", r).map(|_| ())
    }
    pub fn devices(&self) -> Result<Vec<serde_json::Value>, SyncError> {
        parse(self.get("/v1/devices")?)
    }
    pub fn create_team(&self, r: &TeamCreateReq) -> Result<i64, SyncError> {
        let v: serde_json::Value = parse(self.post("/v1/teams", r)?)?;
        v["id"].as_i64().ok_or_else(|| SyncError::Bad("team id".into()))
    }
    pub fn team_lookup(&self, team: i64, email: &str) -> Result<TeamLookupResp, SyncError> {
        parse(self.post(&format!("/v1/teams/{team}/lookup"), &TeamLookupReq { email: email.to_string() })?)
    }
    pub fn team_add_member(&self, team: i64, r: &TeamAddMemberReq) -> Result<(), SyncError> {
        self.post(&format!("/v1/teams/{team}/members"), r).map(|_| ())
    }
    pub fn team_set_role(&self, team: i64, user: i64, role: &str) -> Result<(), SyncError> {
        self.post(&format!("/v1/teams/{team}/members/{user}/role"), &TeamSetRoleReq { role: role.to_string() }).map(|_| ())
    }
    pub fn teams(&self) -> Result<TeamsResp, SyncError> {
        parse(self.get("/v1/teams")?)
    }
    pub fn delete_team(&self, team: i64) -> Result<(), SyncError> {
        self.delete(&format!("/v1/teams/{team}")).map(|_| ())
    }
    pub fn remove_member(&self, team: i64, user: i64) -> Result<(), SyncError> {
        self.delete(&format!("/v1/teams/{team}/members/{user}")).map(|_| ())
    }
    pub fn rotate(&self, team: i64, r: &RotateReq) -> Result<(), SyncError> {
        self.post(&format!("/v1/teams/{team}/rotate"), r).map(|_| ())
    }
    pub fn rotate_items(&self, team: i64, r: &RotateItemsReq) -> Result<RotateItemsResp, SyncError> {
        parse(self.post(&format!("/v1/teams/{team}/rotate/items"), r)?)
    }
    /// A one-time ticket for the shared-terminal relay and the `host:port` to connect to.
    /// The relay runs on its own port of the same machine as the sync server.
    pub fn relay_pass(&self) -> Result<RelayPass, SyncError> {
        let quick = agent(std::time::Duration::from_secs(6));
        let bytes = serde_json::to_vec(&serde_json::json!({})).map_err(|e| SyncError::Bad(e.to_string()))?;
        let reply = self.send(&quick, "POST", "/v1/relay/ticket", Some(bytes))?;
        if reply.status == 404 {
            return Err(SyncError::Rejected("this server has no shared-terminal relay switched on (start it with --relay-listen)".into()));
        }
        let v: serde_json::Value = parse(reply.ok()?)?;
        let ticket = v["ticket"].as_str().ok_or_else(|| SyncError::Bad("ticket".into()))?.to_string();
        let port = v["port"].as_u64().filter(|p| *p > 0 && *p < 65536).ok_or_else(|| SyncError::Bad("port".into()))?;
        let host = host_of(&self.base).ok_or_else(|| SyncError::Bad("server address".into()))?;
        Ok(RelayPass { ticket, addr: format!("{host}:{port}") })
    }
    pub fn revoke_device(&self, id: i64) -> Result<(), SyncError> {
        self.delete(&format!("/v1/devices/{id}")).map(|_| ())
    }
}

impl Remote for Api {
    fn push(&mut self, scope: &str, items: Vec<WireItem>) -> Result<PushResp, SyncError> {
        parse(self.post("/v1/push", &PushReq { scope: scope.to_string(), items })?)
    }
    fn pull(&mut self, scope: &str, since: u64, limit: u32) -> Result<PullResp, SyncError> {
        let q = format!("/v1/pull?scope={}&since={since}&limit={limit}", scope.replace(':', "%3A"));
        parse(self.get(&q)?)
    }
}

#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn the_relay_host_comes_from_the_server_address() {
        assert_eq!(host_of("https://sync.example.com").as_deref(), Some("sync.example.com"));
        assert_eq!(host_of("http://127.0.0.1:8787/").as_deref(), Some("127.0.0.1"));
        assert_eq!(host_of("https://me:pw@sync.example.com:8443/x?y").as_deref(), Some("sync.example.com"));
        assert_eq!(host_of("http://[::1]:8787").as_deref(), Some("[::1]"));
        assert_eq!(host_of("sync.example.com:8787").as_deref(), Some("sync.example.com"));
        assert_eq!(host_of("http://"), None);
    }
}

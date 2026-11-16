use crate::SyncError;
use sync_core::wire::{RotateItemsReq, RotateItemsResp, RotateReq, TeamAddMemberReq, TeamCreateReq, TeamLookupReq, TeamLookupResp, TeamsResp, LoginReq, LoginResp, PasswordReq, PullResp, PushReq, PushResp, RecoverFinishReq, RecoverStartResp, RegisterReq, SaltResp, WireItem};

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

fn map_err(e: ureq::Error) -> SyncError {
    match e {
        ureq::Error::Status(401, _) => SyncError::Unauthorized,
        ureq::Error::Status(c, r) => SyncError::Rejected(format!("{c} {}", r.into_string().unwrap_or_default().chars().take(120).collect::<String>())),
        ureq::Error::Transport(t) => SyncError::Network(t.to_string()),
    }
}

impl Api {
    pub fn new(base: &str, token: Option<String>) -> Api {
        let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(20)).build();
        Api { base: base.trim_end_matches('/').to_string(), token, agent }
    }
    fn req(&self, method: &str, path: &str) -> ureq::Request {
        let r = self.agent.request(method, &format!("{}{}", self.base, path));
        match &self.token {
            Some(t) => r.set("Authorization", &format!("Bearer {t}")),
            None => r,
        }
    }
    fn json<T: serde::de::DeserializeOwned>(r: Result<ureq::Response, ureq::Error>) -> Result<T, SyncError> {
        r.map_err(map_err)?.into_json::<T>().map_err(|e| SyncError::Bad(e.to_string()))
    }
    pub fn salt(&self, email: &str) -> Result<SaltResp, SyncError> {
        Self::json(self.req("GET", &format!("/v1/account/{email}/salt")).call())
    }
    pub fn register(&self, r: &RegisterReq) -> Result<(), SyncError> {
        self.req("POST", "/v1/register").send_json(r).map(|_| ()).map_err(map_err)
    }
    pub fn login(&self, r: &LoginReq) -> Result<LoginResp, SyncError> {
        Self::json(self.req("POST", "/v1/login").send_json(r))
    }
    pub fn logout(&self) -> Result<(), SyncError> {
        self.req("POST", "/v1/logout").send_json(serde_json::json!({})).map(|_| ()).map_err(map_err)
    }
    pub fn recover_start(&self, email: &str) -> Result<RecoverStartResp, SyncError> {
        Self::json(self.req("GET", &format!("/v1/recover/{email}")).call())
    }
    pub fn recover_finish(&self, r: &RecoverFinishReq) -> Result<(), SyncError> {
        self.req("POST", "/v1/recover").send_json(r).map(|_| ()).map_err(map_err)
    }
    pub fn change_password(&self, r: &PasswordReq) -> Result<(), SyncError> {
        self.req("POST", "/v1/account/password").send_json(r).map(|_| ()).map_err(map_err)
    }
    pub fn devices(&self) -> Result<Vec<serde_json::Value>, SyncError> {
        Self::json(self.req("GET", "/v1/devices").call())
    }
    pub fn create_team(&self, r: &TeamCreateReq) -> Result<i64, SyncError> {
        let v: serde_json::Value = Self::json(self.req("POST", "/v1/teams").send_json(r))?;
        v["id"].as_i64().ok_or_else(|| SyncError::Bad("team id".into()))
    }
    pub fn team_lookup(&self, team: i64, email: &str) -> Result<TeamLookupResp, SyncError> {
        Self::json(self.req("POST", &format!("/v1/teams/{team}/lookup")).send_json(TeamLookupReq { email: email.to_string() }))
    }
    pub fn team_add_member(&self, team: i64, r: &TeamAddMemberReq) -> Result<(), SyncError> {
        self.req("POST", &format!("/v1/teams/{team}/members")).send_json(r).map(|_| ()).map_err(map_err)
    }
    pub fn teams(&self) -> Result<TeamsResp, SyncError> {
        Self::json(self.req("GET", "/v1/teams").call())
    }
    pub fn remove_member(&self, team: i64, user: i64) -> Result<(), SyncError> {
        self.req("DELETE", &format!("/v1/teams/{team}/members/{user}")).call().map(|_| ()).map_err(map_err)
    }
    pub fn rotate(&self, team: i64, r: &RotateReq) -> Result<(), SyncError> {
        self.req("POST", &format!("/v1/teams/{team}/rotate")).send_json(r).map(|_| ()).map_err(map_err)
    }
    pub fn rotate_items(&self, team: i64, r: &RotateItemsReq) -> Result<RotateItemsResp, SyncError> {
        Self::json(self.req("POST", &format!("/v1/teams/{team}/rotate/items")).send_json(r))
    }
    /// A one-time ticket for the shared-terminal relay and the `host:port` to connect to.
    /// The relay runs on its own port of the same machine as the sync server.
    pub fn relay_pass(&self) -> Result<RelayPass, SyncError> {
        let r = self.req("POST", "/v1/relay/ticket").timeout(std::time::Duration::from_secs(6)).send_json(serde_json::json!({}));
        let v: serde_json::Value = match r {
            Err(ureq::Error::Status(404, _)) => return Err(SyncError::Rejected("this server has no shared-terminal relay switched on (start it with --relay-listen)".into())),
            other => Self::json(other)?,
        };
        let ticket = v["ticket"].as_str().ok_or_else(|| SyncError::Bad("ticket".into()))?.to_string();
        let port = v["port"].as_u64().filter(|p| *p > 0 && *p < 65536).ok_or_else(|| SyncError::Bad("port".into()))?;
        let host = host_of(&self.base).ok_or_else(|| SyncError::Bad("server address".into()))?;
        Ok(RelayPass { ticket, addr: format!("{host}:{port}") })
    }
    pub fn revoke_device(&self, id: i64) -> Result<(), SyncError> {
        self.req("DELETE", &format!("/v1/devices/{id}")).call().map(|_| ()).map_err(map_err)
    }
}

impl Remote for Api {
    fn push(&mut self, scope: &str, items: Vec<WireItem>) -> Result<PushResp, SyncError> {
        Self::json(self.req("POST", "/v1/push").send_json(PushReq { scope: scope.to_string(), items }))
    }
    fn pull(&mut self, scope: &str, since: u64, limit: u32) -> Result<PullResp, SyncError> {
        let q = format!("/v1/pull?scope={}&since={since}&limit={limit}", scope.replace(':', "%3A"));
        Self::json(self.req("GET", &q).call())
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

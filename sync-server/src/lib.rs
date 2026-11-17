//! Sync server: stores encrypted blobs and who may read them. It never sees plaintext.
pub mod auth;
pub mod db;
pub mod relay;
pub mod routes_account;
pub mod routes_records;
pub mod routes_teams;

use axum::Router;
use db::Db;
use std::net::SocketAddr;
use std::sync::Arc;

/// The shared-terminal relay that runs inside this server, on its own TCP port.
#[derive(Clone)]
pub struct RelayCfg {
    pub port: u16,
    pub tickets: Arc<relay::Tickets>,
}

pub struct AppState {
    pub db: Db,
    pub limiter: auth::Limiter,
    pub relay: Option<RelayCfg>,
    /// read the client address from the reverse proxy's X-Forwarded-For (see `auth::client_ip`)
    pub trust_proxy: bool,
}
pub type State = Arc<AppState>;

pub fn app(db: Db) -> Router {
    app_with(db, None, false)
}

pub fn app_with(db: Db, relay: Option<RelayCfg>, trust_proxy: bool) -> Router {
    let st: State = Arc::new(AppState { db, limiter: auth::Limiter::default(), relay, trust_proxy });
    Router::new()
        .merge(routes_account::routes())
        .merge(routes_records::routes())
        .merge(routes_teams::routes())
        .route("/v1/relay/ticket", axum::routing::post(relay_ticket))
        .with_state(st)
}

/// A signed-in account asks to use the relay: it gets a one-time ticket and the relay's port.
async fn relay_ticket(axum::extract::State(st): axum::extract::State<State>, h: axum::http::HeaderMap) -> Result<axum::Json<serde_json::Value>, auth::ApiErr> {
    auth::caller(&st.db, &h)?;
    match &st.relay {
        Some(r) => Ok(axum::Json(serde_json::json!({"ticket": r.tickets.issue(), "port": r.port}))),
        None => Err(auth::err(axum::http::StatusCode::NOT_FOUND, "relay_not_enabled")),
    }
}

/// Makes a one-time registration code and returns it (only its hash is stored).
pub fn create_invite(db: &Db) -> String {
    let mut b = [0u8; 10];
    getrandom::fill(&mut b).expect("system random");
    let code = base32::encode(base32::Alphabet::Rfc4648 { padding: false }, &b);
    db.with(|c| c.execute("INSERT INTO server_invites(code_hash) VALUES(?1)", [auth::sha(code.as_bytes())])).expect("insert invite");
    code
}

pub async fn serve(db: Db, listener: tokio::net::TcpListener) {
    serve_with(db, listener, None, false).await
}

pub async fn serve_with(db: Db, listener: tokio::net::TcpListener, relay: Option<RelayCfg>, trust_proxy: bool) {
    axum::serve(listener, app_with(db, relay, trust_proxy).into_make_service_with_connect_info::<SocketAddr>()).await.expect("server");
}

/// Starts the relay on `listener` (its own threads) and returns what the HTTP API needs to hand out tickets.
pub fn start_relay(listener: std::net::TcpListener) -> RelayCfg {
    let port = listener.local_addr().expect("relay address").port();
    let tickets = Arc::new(relay::Tickets::default());
    let t2 = tickets.clone();
    std::thread::spawn(move || relay::serve(listener, t2));
    RelayCfg { port, tickets }
}

/// For tests: a server on a random local port, running on its own thread.
pub fn spawn_test_server() -> (String, Db) {
    let db = Db::memory().expect("db");
    (spawn_server_with(db.clone()), db)
}

/// For tests: serve an existing database on a random local port.
pub fn spawn_server_with(db: Db) -> String {
    spawn_server_relay(db, false)
}

/// For tests: like `spawn_server_with`, with the relay running on a random local port when `relay` is set.
pub fn spawn_server_relay(db: Db, relay: bool) -> String {
    spawn_server_inner(db, relay, false)
}

/// For tests: a server that sits behind a (pretend) reverse proxy, or not.
pub fn spawn_server_proxy(db: Db, trust_proxy: bool) -> String {
    spawn_server_inner(db, false, trust_proxy)
}

fn spawn_server_inner(db: Db, relay: bool, trust_proxy: bool) -> String {
    let db2 = db;
    let cfg = relay.then(|| start_relay(std::net::TcpListener::bind("127.0.0.1:0").expect("bind relay")));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            tx.send(l.local_addr().unwrap()).unwrap();
            serve_with(db2, l, cfg, trust_proxy).await;
        });
    });
    let addr = rx.recv().unwrap();
    format!("http://{addr}")
}

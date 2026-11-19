use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS server_invites(code_hash BLOB PRIMARY KEY, used INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS users(
  id INTEGER PRIMARY KEY, email TEXT NOT NULL UNIQUE, salt BLOB NOT NULL, auth_hash BLOB NOT NULL,
  wrapped_sync_key TEXT NOT NULL, wrapped_recovery_key TEXT NOT NULL, recovery_hash BLOB NOT NULL, pubkey TEXT NOT NULL, wrapped_privkey TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS devices(
  id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, name TEXT NOT NULL, token_hash BLOB NOT NULL UNIQUE,
  last_seen INTEGER NOT NULL, revoked INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS records(
  scope TEXT NOT NULL, id TEXT NOT NULL, seq INTEGER NOT NULL, updated_at INTEGER NOT NULL, deleted INTEGER NOT NULL,
  epoch INTEGER NOT NULL, device TEXT NOT NULL, blob TEXT NOT NULL, PRIMARY KEY(scope, id));
CREATE INDEX IF NOT EXISTS records_seq ON records(scope, seq);
CREATE TABLE IF NOT EXISTS scope_seq(scope TEXT PRIMARY KEY, seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS teams(id INTEGER PRIMARY KEY, name TEXT NOT NULL, owner_id INTEGER NOT NULL, epoch INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS members(
  team_id INTEGER NOT NULL, user_id INTEGER NOT NULL, role TEXT NOT NULL, sealed_team_key TEXT NOT NULL,
  epoch INTEGER NOT NULL, prev_sealed_team_key TEXT, PRIMARY KEY(team_id, user_id));
";

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        let c = Connection::open(path)?;
        c.pragma_update(None, "journal_mode", "WAL")?;
        private(path);
        Self::init(c)
    }
    pub fn memory() -> rusqlite::Result<Db> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(c: Connection) -> rusqlite::Result<Db> {
        c.execute_batch(SCHEMA)?;
        // before roles every non-owner was a plain "member"; they keep the same rights as editors
        c.execute("UPDATE members SET role='editor' WHERE role='member'", [])?;
        Ok(Db(Arc::new(Mutex::new(c))))
    }
    /// A consistent copy of the whole database in one file (safe while the server runs). Refuses to overwrite.
    pub fn backup_to(&self, out: &Path) -> Result<(), String> {
        if out.exists() {
            return Err(format!("{} already exists; choose a new file name", out.display()));
        }
        let path = out.to_str().ok_or("the path is not valid text")?.to_string();
        self.with(|c| c.execute("VACUUM INTO ?1", [path]).map(|_| ()).map_err(|e| e.to_string()))?;
        private(out);
        Ok(())
    }
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The database holds password hashes and access tokens' hashes: nobody else on the machine needs to read it.
fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

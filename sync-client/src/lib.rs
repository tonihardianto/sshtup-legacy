//! Client side of sync: talking to the server and merging with the local vault.
pub mod account;
pub mod api;
pub mod engine;
pub mod teams;
pub mod url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    Network(String),
    Unauthorized,
    Rejected(String),
    Bad(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Network(e) => write!(f, "Cannot reach the sync server: {e}"),
            SyncError::Unauthorized => write!(f, "The sync server does not accept this device any more. Sign in again."),
            SyncError::Rejected(e) => write!(f, "The sync server refused the request ({e})."),
            SyncError::Bad(e) => write!(f, "Unexpected answer from the sync server ({e})."),
        }
    }
}

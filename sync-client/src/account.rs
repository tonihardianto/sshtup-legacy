//! Creating an account and signing a device in. All key handling happens here, on the device.
use crate::api::Api;
use crate::SyncError;
use base64::Engine as _;
use sync_core::account::{derive_account, unwrap_key, wrap_key};
use sync_core::boxes::{keypair_from_secret, new_keypair};
use sync_core::recovery::{new_recovery_key, parse_recovery_key, recovery_auth};
use sync_core::wire::{LoginReq, PasswordReq, RecoverFinishReq, RegisterReq};
use zeroize::Zeroizing;

pub const MIN_SYNC_PASSWORD: usize = 8;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Keys this device holds after signing in.
pub struct Secrets {
    pub sync_key: Zeroizing<[u8; 32]>,
    pub x_secret: Zeroizing<[u8; 32]>,
    pub x_public: [u8; 32],
}

pub struct Session {
    pub secrets: Secrets,
    pub token: String,
    pub device_id: String,
}

fn bad(e: &str) -> SyncError {
    SyncError::Bad(e.to_string())
}

/// Creates the account. Returns the recovery key text, shown once; the server cannot give it back.
pub fn register(api: &Api, email: &str, password: &str, invite: &str) -> Result<String, SyncError> {
    if password.chars().count() < MIN_SYNC_PASSWORD {
        return Err(SyncError::Rejected(format!("use at least {MIN_SYNC_PASSWORD} characters")));
    }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| bad("no randomness"))?;
    let keys = derive_account(password, &salt).map_err(|_| bad("key derivation"))?;
    let mut sync_key = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *sync_key).map_err(|_| bad("no randomness"))?;
    let kp = new_keypair();
    let (recovery_text, recovery_key) = new_recovery_key();
    let req = RegisterReq {
        email: email.trim().to_string(),
        invite_code: invite.trim().to_string(),
        salt: b64().encode(salt),
        auth_key: b64().encode(keys.auth),
        wrapped_sync_key: b64().encode(wrap_key(&keys.enc, "sync-key", &sync_key)),
        wrapped_recovery_key: b64().encode(wrap_key(&recovery_key, "sync-key-recovery", &sync_key)),
        recovery_auth: b64().encode(recovery_auth(&recovery_key)),
        pubkey: b64().encode(kp.public),
        wrapped_privkey: b64().encode(wrap_key(&sync_key, "x25519", &kp.secret)),
    };
    api.register(&req)?;
    Ok(recovery_text)
}

pub fn login(api: &Api, email: &str, password: &str, device_name: &str) -> Result<Session, SyncError> {
    let salt = b64().decode(api.salt(email)?.salt).ok().and_then(|s| <[u8; 16]>::try_from(s).ok()).ok_or_else(|| bad("salt"))?;
    let keys = derive_account(password, &salt).map_err(|_| bad("key derivation"))?;
    let r = api.login(&LoginReq { email: email.trim().to_string(), auth_key: b64().encode(keys.auth), device_name: device_name.to_string() })?;
    let unwrap = |w: &str, k: &[u8; 32], purpose: &str| -> Result<Zeroizing<[u8; 32]>, SyncError> {
        let blob = b64().decode(w).map_err(|_| bad("wrapped key"))?;
        unwrap_key(k, purpose, &blob).map_err(|_| bad("the server's key material does not open with this password"))
    };
    let sync_key = unwrap(&r.wrapped_sync_key, &keys.enc, "sync-key")?;
    let x_secret = unwrap(&r.wrapped_privkey, &sync_key, "x25519")?;
    let kp = keypair_from_secret(&x_secret);
    Ok(Session { secrets: Secrets { sync_key, x_secret, x_public: kp.public }, token: r.token, device_id: r.device_id })
}

fn new_password_material(password: &str, sync_key: &[u8; 32]) -> Result<(String, String, String), SyncError> {
    if password.chars().count() < MIN_SYNC_PASSWORD {
        return Err(SyncError::Rejected(format!("use at least {MIN_SYNC_PASSWORD} characters")));
    }
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|_| bad("no randomness"))?;
    let keys = derive_account(password, &salt).map_err(|_| bad("key derivation"))?;
    Ok((b64().encode(salt), b64().encode(keys.auth), b64().encode(wrap_key(&keys.enc, "sync-key", sync_key))))
}

/// New sync password for a signed-in device. The data is not re-encrypted: only the key wrapper changes.
/// The server signs every other device out.
pub fn change_password(api: &Api, sync_key: &[u8; 32], new_password: &str) -> Result<(), SyncError> {
    let (salt, auth_key, wrapped_sync_key) = new_password_material(new_password, sync_key)?;
    api.change_password(&PasswordReq { salt, auth_key, wrapped_sync_key })
}

/// Forgot the sync password: the recovery key proves who you are and sets a new password.
/// Nothing is signed in afterwards; call `login` with the new password.
pub fn recover(api: &Api, email: &str, recovery_text: &str, new_password: &str) -> Result<(), SyncError> {
    let rk = parse_recovery_key(recovery_text).ok_or_else(|| SyncError::Rejected("that recovery key is not valid (check for typos)".into()))?;
    let wrapped = b64().decode(api.recover_start(email)?.wrapped_recovery_key).map_err(|_| bad("wrapped key"))?;
    let sync_key = unwrap_key(&rk, "sync-key-recovery", &wrapped).map_err(|_| SyncError::Rejected("that recovery key does not belong to this account".into()))?;
    let (salt, auth_key, wrapped_sync_key) = new_password_material(new_password, &sync_key)?;
    api.recover_finish(&RecoverFinishReq { email: email.trim().to_string(), recovery_auth: b64().encode(recovery_auth(&rk)), salt, auth_key, wrapped_sync_key })
}

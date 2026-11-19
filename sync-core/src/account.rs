use crate::seal::SealError;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

pub struct AccountKeys {
    /// Sent to the server (it stores only a hash of this).
    pub auth: [u8; 32],
    /// Never leaves the device; wraps the sync key.
    pub enc: Zeroizing<[u8; 32]>,
}

/// Same cost as the local vault: 64 MiB, 3 passes.
pub fn derive_account(password: &str, salt: &[u8; 16]) -> Result<AccountKeys, SealError> {
    let params = Params::new(64 * 1024, 3, 1, Some(32)).map_err(|_| SealError::Bad)?;
    let mut root = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut *root)
        .map_err(|_| SealError::Bad)?;
    let hk = Hkdf::<Sha256>::new(Some(b"sshtup-sync-v1"), &*root);
    let mut auth = [0u8; 32];
    let mut enc = Zeroizing::new([0u8; 32]);
    hk.expand(b"auth", &mut auth).map_err(|_| SealError::Bad)?;
    hk.expand(b"enc", &mut *enc).map_err(|_| SealError::Bad)?;
    Ok(AccountKeys { auth, enc })
}

/// Blob: nonce(24) | ciphertext+tag, with `purpose` as AAD.
pub fn wrap_key(wrapping: &[u8; 32], purpose: &str, key: &[u8; 32]) -> Vec<u8> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("system random");
    let ct = XChaCha20Poly1305::new(wrapping.into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: key, aad: purpose.as_bytes() })
        .expect("encrypt");
    [nonce.as_slice(), &ct].concat()
}

pub fn unwrap_key(wrapping: &[u8; 32], purpose: &str, blob: &[u8]) -> Result<Zeroizing<[u8; 32]>, SealError> {
    if blob.len() != 24 + 32 + 16 {
        return Err(SealError::Bad);
    }
    let pt = XChaCha20Poly1305::new(wrapping.into())
        .decrypt(XNonce::from_slice(&blob[..24]), Payload { msg: &blob[24..], aad: purpose.as_bytes() })
        .map_err(|_| SealError::Bad)?;
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&pt);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SALT: [u8; 16] = [1; 16];
    #[test]
    fn same_password_same_keys() {
        let a = derive_account("correct horse", &SALT).unwrap();
        let b = derive_account("correct horse", &SALT).unwrap();
        assert_eq!(a.auth, b.auth);
        assert_eq!(*a.enc, *b.enc);
    }
    #[test]
    fn different_salt_different_keys() {
        let a = derive_account("pw-long-enough", &SALT).unwrap();
        let b = derive_account("pw-long-enough", &[2; 16]).unwrap();
        assert_ne!(a.auth, b.auth);
    }
    #[test]
    fn auth_and_enc_differ() {
        let a = derive_account("pw-long-enough", &SALT).unwrap();
        assert_ne!(a.auth, *a.enc);
    }
    #[test]
    fn wrap_round_trip() {
        let w = [9u8; 32];
        let k = [5u8; 32];
        let b = wrap_key(&w, "sync", &k);
        assert_eq!(*unwrap_key(&w, "sync", &b).unwrap(), k);
    }
    #[test]
    fn wrap_purpose_bound() {
        let w = [9u8; 32];
        let b = wrap_key(&w, "sync", &[5u8; 32]);
        assert!(unwrap_key(&w, "privkey", &b).is_err());
        assert!(unwrap_key(&[8u8; 32], "sync", &b).is_err());
    }
}

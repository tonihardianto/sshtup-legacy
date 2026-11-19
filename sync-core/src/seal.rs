use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

#[derive(Debug, PartialEq, Eq)]
pub enum SealError {
    Bad,
}

/// Everything about an item that the ciphertext is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemMeta {
    pub scope: String,
    pub id: String,
    pub updated_at: u64,
    pub deleted: bool,
    pub epoch: u32,
}

const VERSION: u8 = 1;
const NONCE: usize = 24;

impl ItemMeta {
    /// Length-prefixed so that no two different metas share the same bytes.
    fn aad(&self) -> Vec<u8> {
        let mut v = vec![VERSION];
        for s in [&self.scope, &self.id] {
            v.extend_from_slice(&(s.len() as u32).to_le_bytes());
            v.extend_from_slice(s.as_bytes());
        }
        v.extend_from_slice(&self.updated_at.to_le_bytes());
        v.push(self.deleted as u8);
        v.extend_from_slice(&self.epoch.to_le_bytes());
        v
    }
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

/// Blob layout: version(1) | nonce(24) | ciphertext+tag.
pub fn seal_item(key: &[u8; 32], meta: &ItemMeta, plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; NONCE];
    getrandom::fill(&mut nonce).expect("system random");
    let ct = cipher(key)
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &meta.aad() })
        .expect("encrypt");
    let mut out = Vec::with_capacity(1 + NONCE + ct.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

pub fn open_item(key: &[u8; 32], meta: &ItemMeta, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, SealError> {
    if blob.len() < 1 + NONCE + 16 || blob[0] != VERSION {
        return Err(SealError::Bad);
    }
    cipher(key)
        .decrypt(XNonce::from_slice(&blob[1..1 + NONCE]), Payload { msg: &blob[1 + NONCE..], aad: &meta.aad() })
        .map(Zeroizing::new)
        .map_err(|_| SealError::Bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meta() -> ItemMeta {
        ItemMeta { scope: "u".into(), id: "host:1".into(), updated_at: 100, deleted: false, epoch: 1 }
    }
    const K: [u8; 32] = [7; 32];
    #[test]
    fn round_trip() {
        let b = seal_item(&K, &meta(), b"hello");
        assert_eq!(&**open_item(&K, &meta(), &b).unwrap(), b"hello");
    }
    #[test]
    fn wrong_id_fails() {
        let b = seal_item(&K, &meta(), b"x");
        let mut m = meta();
        m.id = "host:2".into();
        assert_eq!(open_item(&K, &m, &b), Err(SealError::Bad));
    }
    #[test]
    fn wrong_scope_fails() {
        let b = seal_item(&K, &meta(), b"x");
        let mut m = meta();
        m.scope = "t:1".into();
        assert!(open_item(&K, &m, &b).is_err());
    }
    #[test]
    fn wrong_updated_at_fails() {
        let b = seal_item(&K, &meta(), b"x");
        let mut m = meta();
        m.updated_at = 101;
        assert!(open_item(&K, &m, &b).is_err());
    }
    #[test]
    fn flipped_deleted_fails() {
        let b = seal_item(&K, &meta(), b"x");
        let mut m = meta();
        m.deleted = true;
        assert!(open_item(&K, &m, &b).is_err());
    }
    #[test]
    fn wrong_epoch_fails() {
        let b = seal_item(&K, &meta(), b"x");
        let mut m = meta();
        m.epoch = 2;
        assert!(open_item(&K, &m, &b).is_err());
    }
    #[test]
    fn truncated_blob_fails() {
        let b = seal_item(&K, &meta(), b"hello");
        assert!(open_item(&K, &meta(), &b[..b.len() - 1]).is_err());
        assert!(open_item(&K, &meta(), &b[..10]).is_err());
        assert!(open_item(&K, &meta(), &[]).is_err());
    }
    #[test]
    fn nonce_differs_each_call() {
        assert_ne!(seal_item(&K, &meta(), b"x"), seal_item(&K, &meta(), b"x"));
    }
}

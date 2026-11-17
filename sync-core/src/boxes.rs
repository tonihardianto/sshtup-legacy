use crate::seal::SealError;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

pub struct Keypair {
    pub public: [u8; 32],
    pub secret: Zeroizing<[u8; 32]>,
}

pub fn new_keypair() -> Keypair {
    let mut s = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *s).expect("system random");
    keypair_from_secret(&s)
}

pub fn keypair_from_secret(secret: &[u8; 32]) -> Keypair {
    let public = PublicKey::from(&StaticSecret::from(*secret)).to_bytes();
    Keypair { public, secret: Zeroizing::new(*secret) }
}

fn derive(shared: &[u8; 32], eph: &[u8; 32], recip: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(eph);
    salt[32..].copy_from_slice(recip);
    let mut k = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared).expand(b"sshtup-sealed-box-v1", &mut *k).expect("32 bytes");
    k
}

/// Blob: ephemeral public(32) | nonce(24) | ciphertext+tag. Only the holder of the secret can open it.
pub fn seal_to(public: &[u8; 32], msg: &[u8]) -> Vec<u8> {
    let eph = new_keypair();
    let shared = StaticSecret::from(*eph.secret).diffie_hellman(&PublicKey::from(*public));
    let key = derive(shared.as_bytes(), &eph.public, public);
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("system random");
    let ct = XChaCha20Poly1305::new((&*key).into())
        .encrypt(XNonce::from_slice(&nonce), Payload { msg, aad: &eph.public })
        .expect("encrypt");
    [eph.public.as_slice(), &nonce, &ct].concat()
}

pub fn open_sealed(kp: &Keypair, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, SealError> {
    if blob.len() < 32 + 24 + 16 {
        return Err(SealError::Bad);
    }
    let mut eph = [0u8; 32];
    eph.copy_from_slice(&blob[..32]);
    let shared = StaticSecret::from(*kp.secret).diffie_hellman(&PublicKey::from(eph));
    let key = derive(shared.as_bytes(), &eph, &kp.public);
    XChaCha20Poly1305::new((&*key).into())
        .decrypt(XNonce::from_slice(&blob[32..56]), Payload { msg: &blob[56..], aad: &eph })
        .map(Zeroizing::new)
        .map_err(|_| SealError::Bad)
}

/// 8 groups of 5 digits, for people to compare out loud.
pub fn fingerprint(public: &[u8; 32]) -> String {
    let h = Sha256::digest(public);
    (0..8)
        .map(|i| {
            let n = u32::from_be_bytes([h[i * 4], h[i * 4 + 1], h[i * 4 + 2], h[i * 4 + 3]]);
            format!("{:05}", n % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sealed_round_trip() {
        let kp = new_keypair();
        let b = seal_to(&kp.public, b"team key");
        assert_eq!(&**open_sealed(&kp, &b).unwrap(), b"team key");
    }
    #[test]
    fn other_keypair_cannot_open() {
        let b = seal_to(&new_keypair().public, b"x");
        assert!(open_sealed(&new_keypair(), &b).is_err());
    }
    #[test]
    fn tampered_fails() {
        let kp = new_keypair();
        let mut b = seal_to(&kp.public, b"x");
        let n = b.len();
        b[n - 1] ^= 1;
        assert!(open_sealed(&kp, &b).is_err());
        assert!(open_sealed(&kp, &b[..20]).is_err());
    }
    #[test]
    fn fingerprint_stable_and_formatted() {
        let kp = new_keypair();
        let f = fingerprint(&kp.public);
        assert_eq!(f, fingerprint(&kp.public));
        let groups: Vec<&str> = f.split(' ').collect();
        assert_eq!(groups.len(), 8);
        assert!(groups.iter().all(|g| g.len() == 5 && g.chars().all(|c| c.is_ascii_digit())));
    }
    #[test]
    fn different_keys_different_fingerprints() {
        assert_ne!(fingerprint(&new_keypair().public), fingerprint(&new_keypair().public));
    }
}

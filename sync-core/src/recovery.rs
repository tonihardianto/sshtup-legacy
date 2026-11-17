use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const ALPHA: base32::Alphabet = base32::Alphabet::Rfc4648 { padding: false };

/// 32 random bytes plus a 2-byte checksum, as base32 in groups of 5 so a typo is caught.
pub fn new_recovery_key() -> (String, Zeroizing<[u8; 32]>) {
    let mut key = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *key).expect("system random");
    let mut raw = key.to_vec();
    raw.extend_from_slice(&Sha256::digest(&*key)[..2]);
    let text = base32::encode(ALPHA, &raw);
    let grouped: Vec<&str> = text.as_bytes().chunks(5).map(|c| std::str::from_utf8(c).unwrap()).collect();
    (grouped.join("-"), key)
}

/// What the server checks instead of the key itself when a forgotten password is reset.
pub fn recovery_auth(key: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf::Hkdf::<Sha256>::new(Some(b"sshtup-sync-v1"), key).expand(b"recovery-auth", &mut out).expect("32 bytes");
    out
}

pub fn parse_recovery_key(s: &str) -> Option<Zeroizing<[u8; 32]>> {
    let clean: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase();
    let raw = Zeroizing::new(base32::decode(ALPHA, &clean)?);
    if raw.len() != 34 {
        return None;
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&raw[..32]);
    (Sha256::digest(&*key)[..2] == raw[32..]).then_some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_round_trip() {
        let (text, key) = new_recovery_key();
        assert_eq!(*parse_recovery_key(&text).unwrap(), *key);
        let sloppy = text.to_lowercase().replace('-', "  ");
        assert_eq!(*parse_recovery_key(&sloppy).unwrap(), *key);
    }
    #[test]
    fn recovery_rejects_typo() {
        let (text, _) = new_recovery_key();
        let mut chars: Vec<char> = text.chars().collect();
        chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
        let bad: String = chars.into_iter().collect();
        assert!(parse_recovery_key(&bad).is_none());
        assert!(parse_recovery_key("short").is_none());
    }
}

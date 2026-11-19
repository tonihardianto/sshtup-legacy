//! Keeps the vault key in the system's credential store (macOS Keychain, Windows Credential Manager,
//! the Secret Service on Linux) so the app can open without asking for the master password.
//! The key never goes to a file of ours; the credential store guards it with the user's login.

use zeroize::Zeroizing;

const SERVICE: &str = "sshtup";
const ACCOUNT: &str = "vault-key";

fn entry() -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT).ok()
}

fn to_hex(k: &[u8; 32]) -> Zeroizing<String> {
    Zeroizing::new(k.iter().map(|b| format!("{b:02x}")).collect())
}

fn from_hex(s: &str) -> Option<Zeroizing<[u8; 32]>> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = Zeroizing::new([0u8; 32]);
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn save(key: &[u8; 32]) -> bool {
    entry().map(|e| e.set_password(&to_hex(key)).is_ok()).unwrap_or(false)
}

pub fn load() -> Option<Zeroizing<[u8; 32]>> {
    let pw = Zeroizing::new(entry()?.get_password().ok()?);
    from_hex(&pw)
}

pub fn forget() {
    if let Some(e) = entry() {
        let _ = e.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let k = [7u8; 32];
        assert_eq!(*from_hex(&to_hex(&k)).unwrap(), k);
        assert!(from_hex("zz").is_none());
    }
}

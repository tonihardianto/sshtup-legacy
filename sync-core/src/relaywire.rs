//! The shared-terminal relay's wire format and its end-to-end encryption.
//! One JSON message per line over TCP; terminal bytes travel as hex text so any byte sequence
//! fits in a line. The relay never sees terminal bytes in the clear: they are sealed with a
//! random per-session key that only travels inside the share string the host hands out.

use serde_json::Value;
use std::io::{BufRead, Read, Write};

/// Longest single message accepted (a 16 KB terminal chunk is 32 KB as hex).
pub const MAX_LINE: u64 = 262_144;

pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

pub fn unhex(s: &str) -> Vec<u8> {
    let v = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    s.as_bytes().chunks_exact(2).filter_map(|p| Some(v(p[0])? << 4 | v(p[1])?)).collect()
}

pub fn send(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let mut line = v.to_string();
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()
}

/// Reads one message; `None` at end of stream, on an error, on a line that is too long or on bad JSON.
pub fn read_msg<R: BufRead>(r: &mut R) -> Option<Value> {
    let mut s = String::new();
    let n = r.by_ref().take(MAX_LINE).read_line(&mut s).ok()?;
    if n == 0 || !s.ends_with('\n') {
        return None;
    }
    serde_json::from_str(s.trim()).ok()
}

pub fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// A display name with control characters removed and a length limit.
pub fn clean_name(s: &str) -> String {
    let n: String = s.chars().filter(|c| !c.is_control()).take(24).collect();
    if n.trim().is_empty() { "guest".into() } else { n }
}

// ------------------------------------------------------------------ end-to-end --

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

pub const KEY_LEN: usize = 16;

/// Which way a chunk travels. It is authenticated, so a chunk cannot be replayed the other way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    /// host -> guests (terminal output)
    Out,
    /// guest -> host (keystrokes)
    In,
}

pub fn new_key() -> [u8; KEY_LEN] {
    let mut k = [0u8; KEY_LEN];
    getrandom::fill(&mut k).expect("system random");
    k
}

fn cipher(key: &[u8; KEY_LEN]) -> XChaCha20Poly1305 {
    let mut k = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(None, key).expand(b"sshtup relay v1", &mut k).expect("32 bytes");
    XChaCha20Poly1305::new(Key::from_slice(&k))
}

fn aad(d: Dir) -> &'static [u8] {
    match d {
        Dir::Out => b"out",
        Dir::In => b"in",
    }
}

/// nonce(24) | ciphertext, as hex.
pub fn seal(key: &[u8; KEY_LEN], dir: Dir, plain: &[u8]) -> String {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("system random");
    let ct = cipher(key).encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad: aad(dir) }).expect("encrypt");
    hex(&[nonce.as_slice(), &ct].concat())
}

/// `None` when the chunk was not sealed with this key (or was changed on the way).
pub fn open(key: &[u8; KEY_LEN], dir: Dir, h: &str) -> Option<Vec<u8>> {
    let b = unhex(h);
    if b.len() < 24 + 16 {
        return None;
    }
    cipher(key).decrypt(XNonce::from_slice(&b[..24]), Payload { msg: &b[24..], aad: aad(dir) }).ok()
}

/// What the host hands to guests: the relay's session code and the key, `xxxx-xxxx~KEY`.
pub fn share_string(code: &str, key: &[u8; KEY_LEN]) -> String {
    format!("{code}~{}", base32::encode(base32::Alphabet::Rfc4648 { padding: false }, key).to_lowercase())
}

pub fn parse_share(s: &str) -> Option<(String, [u8; KEY_LEN])> {
    let (code, k) = s.trim().split_once('~')?;
    let raw = base32::decode(base32::Alphabet::Rfc4648 { padding: false }, &k.trim().to_uppercase())?;
    let key: [u8; KEY_LEN] = raw.try_into().ok()?;
    Some((code.trim().to_lowercase(), key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_round_trip_and_stay_sealed() {
        let k = new_key();
        let h = seal(&k, Dir::Out, b"hello \x1b[31mred");
        assert!(!h.contains("68656c6c6f"), "plaintext must not appear");
        assert_eq!(open(&k, Dir::Out, &h).unwrap(), b"hello \x1b[31mred");
        assert_ne!(seal(&k, Dir::Out, b"a"), seal(&k, Dir::Out, b"a"), "fresh nonce every time");
    }

    #[test]
    fn wrong_key_wrong_direction_or_tampering_is_refused() {
        let k = new_key();
        let h = seal(&k, Dir::In, b"ls\n");
        assert!(open(&new_key(), Dir::In, &h).is_none());
        assert!(open(&k, Dir::Out, &h).is_none(), "an input chunk cannot be replayed as output");
        let mut t = h.clone().into_bytes();
        let last = t.len() - 1;
        t[last] = if t[last] == b'0' { b'1' } else { b'0' };
        assert!(open(&k, Dir::In, &String::from_utf8(t).unwrap()).is_none());
        assert!(open(&k, Dir::In, "").is_none());
        assert!(open(&k, Dir::In, "abcd").is_none());
    }

    #[test]
    fn share_string_round_trips_and_forgives_case() {
        let k = new_key();
        let s = share_string("abcd-2345", &k);
        assert_eq!(parse_share(&s), Some(("abcd-2345".to_string(), k)));
        assert_eq!(parse_share(&format!("  {}  ", s.to_uppercase())), Some(("abcd-2345".to_string(), k)));
        assert!(parse_share("abcd-2345").is_none(), "a code without its key is useless");
        assert!(parse_share("abcd-2345~xx").is_none());
    }
}

//! What the Keychain shows about an SSH key: its type and size, its fingerprint, and whether the key file is
//! protected by a passphrase. Everything is read from the public key line and the private key file, so no
//! `ssh-keygen` run is needed.

use base64::Engine as _;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KeyInfo {
    /// "ED25519", "RSA", "ECDSA", "DSA" (empty when the public key could not be read).
    pub kind: String,
    pub bits: Option<u32>,
    /// "SHA256:…", the same text `ssh-keygen -l` prints.
    pub fingerprint: String,
    /// The public key line.
    pub public: String,
    /// Whether the private key file is encrypted; None when the file cannot be read.
    pub protected: Option<bool>,
    /// DSA, or RSA shorter than 2048 bits.
    pub weak: bool,
}

impl KeyInfo {
    /// "ED25519", or "RSA 4096".
    pub fn type_label(&self) -> String {
        match (self.kind.as_str(), self.bits) {
            ("", _) => String::new(),
            ("RSA", Some(b)) => format!("RSA {b}"),
            (k, _) => k.to_string(),
        }
    }
}

/// Reads the key at `path` (a private key file). `public_key` is the line the vault already holds; when it is
/// empty the `.pub` file next to the key is used.
pub fn inspect(path: &str, public_key: &str) -> KeyInfo {
    let file = crate::app::expand_home(path);
    let mut public = public_key.trim().to_string();
    if public.is_empty() && !path.trim().is_empty() {
        public = std::fs::read_to_string(format!("{file}.pub")).ok().and_then(|s| s.lines().next().map(|l| l.trim().to_string())).unwrap_or_default();
    }
    let mut info = KeyInfo { public: public.clone(), ..Default::default() };
    if let Some((kind, bits, fp)) = parse_public(&public) {
        info.weak = kind == "DSA" || (kind == "RSA" && bits.is_some_and(|b| b < 2048));
        info.kind = kind;
        info.bits = bits;
        info.fingerprint = fp;
    }
    if !path.trim().is_empty() {
        info.protected = std::fs::read(&file).ok().and_then(|b| protected(&b[..b.len().min(64 * 1024)]));
    }
    info
}

fn take<'a>(b: &mut &'a [u8]) -> Option<&'a [u8]> {
    if b.len() < 4 {
        return None;
    }
    let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let rest = &b[4..];
    if rest.len() < n {
        return None;
    }
    let (s, tail) = rest.split_at(n);
    *b = tail;
    Some(s)
}

/// (type, size in bits, fingerprint) of a public key line such as `ssh-ed25519 AAAA… comment`.
fn parse_public(line: &str) -> Option<(String, Option<u32>, String)> {
    let mut it = line.split_whitespace();
    let algo = it.next()?;
    let blob = base64::engine::general_purpose::STANDARD.decode(it.next()?).ok()?;
    let fp = format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&blob)));
    let (kind, bits) = match algo {
        "ssh-ed25519" => ("ED25519", Some(256)),
        "sk-ssh-ed25519@openssh.com" => ("ED25519-SK", Some(256)),
        "ssh-dss" => ("DSA", Some(1024)),
        "ssh-rsa" => {
            let mut b = &blob[..];
            take(&mut b)?;
            take(&mut b)?; // the exponent
            let n = take(&mut b)?;
            let skip = n.iter().take_while(|x| **x == 0).count();
            let n = &n[skip..];
            let bits = n.first().map(|f| (n.len() as u32) * 8 - f.leading_zeros()).unwrap_or(0);
            ("RSA", Some(bits))
        }
        a if a.starts_with("ecdsa-sha2-nistp") => ("ECDSA", a.strip_prefix("ecdsa-sha2-nistp").and_then(|n| n.parse().ok())),
        a if a.starts_with("sk-ecdsa-sha2-nistp") => ("ECDSA-SK", None),
        _ => return None,
    };
    Some((kind.to_string(), bits, fp))
}

/// Whether the start of a private key file is encrypted: Some(true) / Some(false), None when it is not a key.
fn protected(head: &[u8]) -> Option<bool> {
    let text = String::from_utf8_lossy(head);
    if text.contains("ENCRYPTED") {
        return Some(true);
    }
    if text.contains("BEGIN OPENSSH PRIVATE KEY") {
        let body: String = text.lines().filter(|l| !l.starts_with("-----")).collect::<Vec<_>>().concat();
        let raw = base64::engine::general_purpose::STANDARD.decode(body.trim()).ok().or_else(|| {
            // a file cut at the read limit ends in the middle of a base64 group
            let t = body.trim();
            base64::engine::general_purpose::STANDARD.decode(&t[..t.len() / 4 * 4]).ok()
        })?;
        let rest = raw.strip_prefix(b"openssh-key-v1\0")?;
        let mut b = rest;
        let cipher = take(&mut b)?;
        return Some(cipher != b"none");
    }
    if text.contains("PRIVATE KEY") {
        return Some(false);
    }
    None
}

/// "12 Oct 2026" (UTC).
pub fn date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    // days since 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// `SHA256:kq3VhR8…6yP9xA`, short enough for a list row.
pub fn short_fingerprint(fp: &str) -> String {
    let n = fp.chars().count();
    if n <= 22 {
        return fp.to_string();
    }
    let head: String = fp.chars().take(14).collect();
    let tail: String = fp.chars().skip(n - 6).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED_PUB: &str = include_str!("../testdata/keys/ed_plain.pub");
    const RSA_PUB: &str = include_str!("../testdata/keys/rsa_pem.pub");
    const RSA2048_PUB: &str = include_str!("../testdata/keys/rsa2048.pub");

    #[test]
    fn fingerprints_match_ssh_keygen() {
        let (k, b, fp) = parse_public(ED_PUB).unwrap();
        assert_eq!((k.as_str(), b), ("ED25519", Some(256)));
        assert_eq!(fp, "SHA256:MKH5G4+5p5imvKp0GqaK14zURrN92dUhQ3mHpUrEDQo");
        let (k, b, fp) = parse_public(RSA_PUB).unwrap();
        assert_eq!((k.as_str(), b), ("RSA", Some(1024)));
        assert_eq!(fp, "SHA256:MSucCAbXMqbkYBIrUVjOa6QTUZ35rErdO9RvwrXmv7A");
        let (_, b, fp) = parse_public(RSA2048_PUB).unwrap();
        assert_eq!(b, Some(2048));
        assert_eq!(fp, "SHA256:edGJtMWmKeBFWvbSXZ+7vVNXqYmVDoPQyTUOh6sMRmw");
    }

    #[test]
    fn passphrase_is_read_from_the_key_file() {
        assert_eq!(protected(include_bytes!("../testdata/keys/ed_plain")), Some(false));
        assert_eq!(protected(include_bytes!("../testdata/keys/ed_enc")), Some(true));
        assert_eq!(protected(include_bytes!("../testdata/keys/rsa_pem")), Some(false));
        assert_eq!(protected(include_bytes!("../testdata/keys/rsa4096_enc")), Some(true));
        assert_eq!(protected(b"hello"), None);
    }

    #[test]
    fn weak_keys_are_flagged() {
        let dir = std::env::temp_dir().join(format!("tc_keyinfo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("k");
        std::fs::write(&p, include_bytes!("../testdata/keys/rsa_pem")).unwrap();
        let i = inspect(&p.to_string_lossy(), RSA_PUB);
        assert!(i.weak);
        assert_eq!(i.type_label(), "RSA 1024");
        assert_eq!(i.protected, Some(false));
        let i = inspect("/nonexistent/key", ED_PUB);
        assert!(!i.weak);
        assert_eq!(i.type_label(), "ED25519");
        assert_eq!(i.protected, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1 Jan 1970");
        assert_eq!(date(1_789_000_000), "10 Sep 2026");
        assert_eq!(short_fingerprint("SHA256:MKH5G4+5p5imvKp0GqaK14zURrN92dUhQ3mHpUrEDQo"), "SHA256:MKH5G4+…UrEDQo");
    }
}

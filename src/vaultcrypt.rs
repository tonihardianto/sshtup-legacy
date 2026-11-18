//! Encrypts the vault with a master password.
//!
//! password --Argon2id(salt)--> 32-byte key --XChaCha20-Poly1305(nonce)--> vault bytes
//!
//! File layout (all integers little-endian):
//!   magic "TVAULT" | version u8 | m_kib u32 | t u32 | p u32 | salt 16 | nonce 24 | ciphertext+tag
//! Everything before the ciphertext is authenticated as associated data, so changing the
//! cost parameters or the salt makes decryption fail instead of silently weakening it.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 6] = b"TVAULT";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 6 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN;

/// Cost used for new vaults: 64 MiB, 3 passes, 1 lane (above the OWASP minimum).
pub const DEFAULT_M_KIB: u32 = 64 * 1024;
pub const DEFAULT_T: u32 = 3;
pub const DEFAULT_P: u32 = 1;

// A tampered header must not be able to make us allocate gigabytes or loop for minutes.
const MAX_M_KIB: u32 = 1024 * 1024;
const MAX_T: u32 = 12;
const MAX_P: u32 = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum CryptError {
    /// Not an encrypted vault file.
    NotEncrypted,
    /// Header is damaged or has unsupported values.
    Corrupt,
    /// Wrong password, or the file was modified.
    WrongPassword,
    Internal,
}

/// The derived key plus what is needed to derive it again. Wiped from memory on drop.
pub struct VaultKey {
    key: Zeroizing<[u8; 32]>,
    salt: [u8; SALT_LEN],
    m_kib: u32,
    t: u32,
    p: u32,
}

pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.len() > HEADER_LEN && bytes.starts_with(MAGIC)
}

fn random<const N: usize>() -> Result<[u8; N], CryptError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| CryptError::Internal)?;
    Ok(b)
}

fn derive(password: &str, salt: &[u8; SALT_LEN], m_kib: u32, t: u32, p: u32) -> Result<Zeroizing<[u8; 32]>, CryptError> {
    let params = Params::new(m_kib, t, p, Some(32)).map_err(|_| CryptError::Corrupt)?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, out.as_mut())
        .map_err(|_| CryptError::Internal)?;
    Ok(out)
}

impl VaultKey {
    /// A new key for a new master password: fresh random salt, default cost.
    pub fn new(password: &str) -> Result<Self, CryptError> {
        Self::with_cost(password, DEFAULT_M_KIB, DEFAULT_T, DEFAULT_P)
    }

    pub fn with_cost(password: &str, m_kib: u32, t: u32, p: u32) -> Result<Self, CryptError> {
        let salt = random::<SALT_LEN>()?;
        let key = derive(password, &salt, m_kib, t, p)?;
        Ok(Self { key, salt, m_kib, t, p })
    }

    fn header(&self, nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
        let mut h = Vec::with_capacity(HEADER_LEN);
        h.extend_from_slice(MAGIC);
        h.push(VERSION);
        h.extend_from_slice(&self.m_kib.to_le_bytes());
        h.extend_from_slice(&self.t.to_le_bytes());
        h.extend_from_slice(&self.p.to_le_bytes());
        h.extend_from_slice(&self.salt);
        h.extend_from_slice(nonce);
        h
    }

    /// Encrypts with a fresh random nonce (never reused, even though the key is).
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptError> {
        let nonce = random::<NONCE_LEN>()?;
        let mut out = self.header(&nonce);
        let cipher = XChaCha20Poly1305::new((&*self.key).into());
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &out })
            .map_err(|_| CryptError::Internal)?;
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

/// Opens an encrypted vault. Returns the plaintext and the key to save changes with.
pub fn decrypt(bytes: &[u8], password: &str) -> Result<(Zeroizing<Vec<u8>>, VaultKey), CryptError> {
    if !bytes.starts_with(MAGIC) {
        return Err(CryptError::NotEncrypted);
    }
    if bytes.len() <= HEADER_LEN + 16 || bytes[6] != VERSION {
        return Err(CryptError::Corrupt);
    }
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (m_kib, t, p) = (u32_at(7), u32_at(11), u32_at(15));
    if m_kib < 8 || m_kib > MAX_M_KIB || t < 1 || t > MAX_T || p < 1 || p > MAX_P {
        return Err(CryptError::Corrupt);
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[19..19 + SALT_LEN]);
    let nonce = &bytes[19 + SALT_LEN..HEADER_LEN];
    let key = derive(password, &salt, m_kib, t, p)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let pt = cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: &bytes[HEADER_LEN..], aad: &bytes[..HEADER_LEN] })
        .map_err(|_| CryptError::WrongPassword)?;
    Ok((Zeroizing::new(pt), VaultKey { key, salt, m_kib, t, p }))
}

impl VaultKey {
    /// The derived key itself (for keeping in the system keychain). Anyone who has it can open the vault.
    pub fn raw(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.key)
    }
}

/// Opens an encrypted vault with a key that was derived earlier (no password, no Argon2).
pub fn decrypt_with_key(bytes: &[u8], key: &[u8; 32]) -> Result<(Zeroizing<Vec<u8>>, VaultKey), CryptError> {
    if !bytes.starts_with(MAGIC) {
        return Err(CryptError::NotEncrypted);
    }
    if bytes.len() <= HEADER_LEN + 16 || bytes[6] != VERSION {
        return Err(CryptError::Corrupt);
    }
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let (m_kib, t, p) = (u32_at(7), u32_at(11), u32_at(15));
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&bytes[19..19 + SALT_LEN]);
    let nonce = &bytes[19 + SALT_LEN..HEADER_LEN];
    let cipher = XChaCha20Poly1305::new(key.into());
    let pt = cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: &bytes[HEADER_LEN..], aad: &bytes[..HEADER_LEN] })
        .map_err(|_| CryptError::WrongPassword)?;
    Ok((Zeroizing::new(pt), VaultKey { key: Zeroizing::new(*key), salt, m_kib, t, p }))
}

impl Drop for VaultKey {
    fn drop(&mut self) {
        self.salt.zeroize();
    }
}

// ------------------------------------------------------------------ session logs --
//
// A session log is written as it happens, so it is encrypted in records instead of as one blob:
//   magic "TLOG01" | file id 16 | { record len u32 | nonce 24 | ciphertext+tag } ...
// Every record has its own random nonce. The associated data holds the file id and the record
// number, so records cannot be moved to another file, reordered or dropped from the middle.
// A cut-off final record (crash, power loss) is ignored when reading.
// The key is a random 32-byte log key kept inside the (encrypted) vault, so changing the master
// password never makes old logs unreadable.

const LOG_MAGIC: &[u8; 6] = b"TLOG01";
const LOG_HEADER_LEN: usize = 6 + 16;
const LOG_RECORD_MAX: usize = 64 * 1024;

#[derive(Clone)]
pub struct LogKey(std::sync::Arc<Zeroizing<[u8; 32]>>);

impl std::fmt::Debug for LogKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LogKey(..)")
    }
}

impl LogKey {
    pub fn random() -> Result<Self, CryptError> {
        Ok(Self(std::sync::Arc::new(Zeroizing::new(random::<32>()?))))
    }

    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut k = Zeroizing::new([0u8; 32]);
        for i in 0..32 {
            k[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(std::sync::Arc::new(k)))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&**self.0).into())
    }
}

pub fn is_encrypted_log(bytes: &[u8]) -> bool {
    bytes.starts_with(LOG_MAGIC) && bytes.len() >= LOG_HEADER_LEN
}

fn log_aad(file_id: &[u8], counter: u64) -> Vec<u8> {
    let mut a = Vec::with_capacity(LOG_HEADER_LEN + 8);
    a.extend_from_slice(LOG_MAGIC);
    a.extend_from_slice(file_id);
    a.extend_from_slice(&counter.to_le_bytes());
    a
}

/// Appends encrypted records to any writer (a file, or a buffer when converting).
pub struct LogWriter<W: std::io::Write> {
    out: W,
    cipher: XChaCha20Poly1305,
    file_id: [u8; 16],
    counter: u64,
}

impl<W: std::io::Write> LogWriter<W> {
    pub fn new(mut out: W, key: &LogKey) -> std::io::Result<Self> {
        let file_id = random::<16>().map_err(|_| std::io::Error::other("no randomness"))?;
        out.write_all(LOG_MAGIC)?;
        out.write_all(&file_id)?;
        Ok(Self { out, cipher: key.cipher(), file_id, counter: 0 })
    }

    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        for part in data.chunks(LOG_RECORD_MAX) {
            let nonce = random::<NONCE_LEN>().map_err(|_| std::io::Error::other("no randomness"))?;
            let aad = log_aad(&self.file_id, self.counter);
            let ct = self
                .cipher
                .encrypt(XNonce::from_slice(&nonce), Payload { msg: part, aad: &aad })
                .map_err(|_| std::io::Error::other("encryption failed"))?;
            let mut rec = Vec::with_capacity(4 + NONCE_LEN + ct.len());
            rec.extend_from_slice(&(ct.len() as u32).to_le_bytes());
            rec.extend_from_slice(&nonce);
            rec.extend_from_slice(&ct);
            // one write per record, so a crash leaves at most one cut-off record at the end
            self.out.write_all(&rec)?;
            self.counter += 1;
        }
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

/// Decrypts a whole encrypted log. Fails on a wrong key or any record that was changed.
pub fn read_log(bytes: &[u8], key: &LogKey) -> Result<Vec<u8>, CryptError> {
    if !is_encrypted_log(bytes) {
        return Err(CryptError::NotEncrypted);
    }
    let file_id = &bytes[6..LOG_HEADER_LEN];
    let cipher = key.cipher();
    let mut out = Vec::new();
    let mut pos = LOG_HEADER_LEN;
    let mut counter = 0u64;
    while pos + 4 <= bytes.len() {
        let len = u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let end = pos + 4 + NONCE_LEN + len;
        if len < 16 || len > LOG_RECORD_MAX + 16 {
            return Err(CryptError::Corrupt);
        }
        if end > bytes.len() {
            break; // cut-off last record
        }
        let nonce = &bytes[pos + 4..pos + 4 + NONCE_LEN];
        let pt = cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: &bytes[pos + 4 + NONCE_LEN..end], aad: &log_aad(file_id, counter) })
            .map_err(|_| CryptError::WrongPassword)?;
        out.extend_from_slice(&pt);
        pos = end;
        counter += 1;
    }
    Ok(out)
}

// ------------------------------------------------------------------ small sealed files --
//
// Files that are rewritten as a whole (the command history) use the same log key:
//   magic "TBLOB1" | nonce 24 | ciphertext+tag     (the file's purpose is authenticated too)

const BLOB_MAGIC: &[u8; 6] = b"TBLOB1";

pub fn is_sealed(bytes: &[u8]) -> bool {
    bytes.starts_with(BLOB_MAGIC)
}

fn blob_aad(purpose: &str) -> Vec<u8> {
    let mut a = BLOB_MAGIC.to_vec();
    a.extend_from_slice(purpose.as_bytes());
    a
}

/// Encrypts `plaintext` for one `purpose` ("history"), so a file cannot be swapped for another.
pub fn seal(key: &LogKey, purpose: &str, plaintext: &[u8]) -> Result<Vec<u8>, CryptError> {
    let nonce = random::<NONCE_LEN>()?;
    let ct = key
        .cipher()
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plaintext, aad: &blob_aad(purpose) })
        .map_err(|_| CryptError::Internal)?;
    let mut out = BLOB_MAGIC.to_vec();
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn unseal(key: &LogKey, purpose: &str, bytes: &[u8]) -> Result<Vec<u8>, CryptError> {
    if !is_sealed(bytes) {
        return Err(CryptError::NotEncrypted);
    }
    if bytes.len() < 6 + NONCE_LEN + 16 {
        return Err(CryptError::Corrupt);
    }
    key.cipher()
        .decrypt(XNonce::from_slice(&bytes[6..6 + NONCE_LEN]), Payload { msg: &bytes[6 + NONCE_LEN..], aad: &blob_aad(purpose) })
        .map_err(|_| CryptError::WrongPassword)
}

/// Where a session writes what the program printed.
pub enum LogSink {
    Plain(std::fs::File),
    Encrypted(LogWriter<std::fs::File>),
}

impl LogSink {
    /// Creates a new private log file, encrypted when a key is given.
    pub fn create(path: &std::path::Path, key: Option<&LogKey>) -> Option<Self> {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let f = o.open(path).ok()?;
        match key {
            Some(k) => LogWriter::new(f, k).ok().map(LogSink::Encrypted),
            None => Some(LogSink::Plain(f)),
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        use std::io::Write;
        let _ = match self {
            LogSink::Plain(f) => f.write_all(data),
            LogSink::Encrypted(w) => w.write(data),
        };
    }
}

/// Rewrites the logs in `dir` as encrypted (`to` = Some) or readable (`to` = None, `from` = the key).
/// Files in `skip` (still being written) are left alone. Returns (converted, failed).
pub fn convert_logs(dir: &std::path::Path, to: Option<&LogKey>, from: Option<&LogKey>, skip: &[std::path::PathBuf]) -> (usize, usize) {
    let (mut done, mut failed) = (0, 0);
    let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0) };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() || p.extension().map(|x| x != "log").unwrap_or(true) || skip.contains(&p) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { failed += 1; continue };
        let enc = is_encrypted_log(&bytes);
        let new: Option<Vec<u8>> = match (to, enc) {
            (Some(k), false) => LogWriter::new(Vec::new(), k).and_then(|mut w| w.write(&bytes).map(|_| w.into_inner())).ok(),
            (None, true) => from.and_then(|k| read_log(&bytes, k).ok()),
            _ => continue, // already in the wanted form
        };
        let Some(new) = new else { failed += 1; continue };
        let tmp = p.with_extension("log.tmp");
        let ok = {
            use std::io::Write;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            o.open(&tmp).and_then(|mut f| f.write_all(&new).and_then(|_| f.sync_all())).is_ok() && std::fs::rename(&tmp, &p).is_ok()
        };
        if ok { done += 1 } else { failed += 1; let _ = std::fs::remove_file(&tmp); }
    }
    (done, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // low cost keeps the tests fast; the format and checks are the same
    fn key(pw: &str) -> VaultKey {
        VaultKey::with_cost(pw, 64, 1, 1).unwrap()
    }

    #[test]
    fn round_trip() {
        let k = key("correct horse");
        let blob = k.encrypt(b"{\"hosts\":[]}").unwrap();
        assert!(is_encrypted(&blob));
        assert!(!blob.windows(5).any(|w| w == b"hosts"));
        let (pt, k2) = decrypt(&blob, "correct horse").unwrap();
        assert_eq!(&pt[..], b"{\"hosts\":[]}");
        // the returned key can save again, and reads back
        let again = k2.encrypt(b"second").unwrap();
        assert_eq!(&decrypt(&again, "correct horse").unwrap().0[..], b"second");
    }

    #[test]
    fn wrong_password_is_refused() {
        let blob = key("right").encrypt(b"secret").unwrap();
        assert_eq!(decrypt(&blob, "wrong").err(), Some(CryptError::WrongPassword));
    }

    #[test]
    fn nonce_is_fresh_every_save() {
        let k = key("pw");
        let a = k.encrypt(b"same").unwrap();
        let b = k.encrypt(b"same").unwrap();
        assert_ne!(a[HEADER_LEN - NONCE_LEN..HEADER_LEN], b[HEADER_LEN - NONCE_LEN..HEADER_LEN]);
        assert_ne!(a, b);
    }

    #[test]
    fn a_kept_key_opens_the_vault_without_the_password() {
        let key = VaultKey::with_cost("pw-for-test", 8, 1, 1).unwrap();
        let bytes = key.encrypt(b"secret").unwrap();
        let (pt, again) = decrypt_with_key(&bytes, &key.raw()).unwrap();
        assert_eq!(&pt[..], b"secret");
        // the reopened key saves in a form the password still opens
        let (pt2, _) = decrypt(&again.encrypt(b"more").unwrap(), "pw-for-test").unwrap();
        assert_eq!(&pt2[..], b"more");
        assert_eq!(decrypt_with_key(&bytes, &[0u8; 32]).err(), Some(CryptError::WrongPassword));
    }

    #[test]
    fn tampering_is_detected() {
        let blob = key("pw").encrypt(b"secret").unwrap();
        // flip a ciphertext byte
        let mut c = blob.clone();
        let last = c.len() - 1;
        c[last] ^= 1;
        assert_eq!(decrypt(&c, "pw").err(), Some(CryptError::WrongPassword));
        // change a header byte (the salt): authenticated, so it fails too
        let mut h = blob.clone();
        h[20] ^= 1;
        assert!(decrypt(&h, "pw").is_err());
    }

    #[test]
    fn hostile_header_is_rejected_without_work() {
        let mut blob = key("pw").encrypt(b"secret").unwrap();
        blob[7..11].copy_from_slice(&u32::MAX.to_le_bytes()); // m_kib huge
        assert_eq!(decrypt(&blob, "pw").err(), Some(CryptError::Corrupt));
        assert_eq!(decrypt(b"{\"plain\":true}", "pw").err(), Some(CryptError::NotEncrypted));
        assert!(!is_encrypted(b"{\"plain\":true}"));
    }

    #[test]
    fn log_round_trip_and_tamper() {
        let k = LogKey::random().unwrap();
        let mut w = LogWriter::new(Vec::new(), &k).unwrap();
        w.write(b"first line\n").unwrap();
        w.write(&vec![b'x'; 150_000]).unwrap(); // several records
        let blob = w.into_inner();
        assert!(!blob.windows(10).any(|x| x == b"first line"));
        let back = read_log(&blob, &k).unwrap();
        assert_eq!(&back[..11], b"first line\n");
        assert_eq!(back.len(), 11 + 150_000);
        // wrong key
        assert!(read_log(&blob, &LogKey::random().unwrap()).is_err());
        // a cut-off last record is ignored, earlier data still reads
        let cut = &blob[..blob.len() - 5];
        assert!(read_log(cut, &k).unwrap().starts_with(b"first line"));
        // a flipped byte is detected
        let mut bad = blob.clone();
        bad[LOG_HEADER_LEN + 4 + NONCE_LEN + 2] ^= 1;
        assert!(read_log(&bad, &k).is_err());
        // hex round trip
        assert!(LogKey::from_hex(&k.to_hex()).is_some());
        assert!(LogKey::from_hex("zz").is_none());
    }

    #[test]
    fn converting_logs_both_ways() {
        let dir = std::env::temp_dir().join(format!("tc_logs_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1__a.log"), b"secret output").unwrap();
        std::fs::write(dir.join("2__b.log"), b"in use").unwrap();
        let k = LogKey::random().unwrap();
        let skip = vec![dir.join("2__b.log")];
        assert_eq!(convert_logs(&dir, Some(&k), None, &skip), (1, 0));
        let on_disk = std::fs::read(dir.join("1__a.log")).unwrap();
        assert!(is_encrypted_log(&on_disk) && !String::from_utf8_lossy(&on_disk).contains("secret"));
        assert_eq!(std::fs::read(dir.join("2__b.log")).unwrap(), b"in use");
        assert_eq!(convert_logs(&dir, None, Some(&k), &skip), (1, 0));
        assert_eq!(std::fs::read(dir.join("1__a.log")).unwrap(), b"secret output");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sealed_files_round_trip_and_stay_bound_to_their_purpose() {
        let k = LogKey::random().unwrap();
        let blob = seal(&k, "history", b"{\"0\":[\"ls -la\"]}").unwrap();
        assert!(is_sealed(&blob) && !String::from_utf8_lossy(&blob).contains("ls -la"));
        assert_eq!(unseal(&k, "history", &blob).unwrap(), b"{\"0\":[\"ls -la\"]}");
        assert!(unseal(&k, "other", &blob).is_err());
        assert!(unseal(&LogKey::random().unwrap(), "history", &blob).is_err());
        let mut bad = blob.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(unseal(&k, "history", &bad).is_err());
        assert_eq!(unseal(&k, "history", b"{}").err(), Some(CryptError::NotEncrypted));
    }
}

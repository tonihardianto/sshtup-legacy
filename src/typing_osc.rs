//! How a guest's terminal tab learns who is typing.
//!
//! The guest tab runs `--relay-join` in a pty. That program has no window of its own, so when the host reports
//! "Ann typed", it prints a private escape sequence to its own output and the app, which owns the pty, picks it up,
//! removes it from the screen and shows the badge. The sequence carries a random per-tab secret, so text printed by
//! the host's terminal (which also reaches this output) cannot make a badge appear.
//!
//!     ESC ] 7733 ; <secret> ; <name as hex> ; <rrggbb> BEL

use sync_core::relaywire::{hex, unhex};

const PREFIX: &[u8] = b"\x1b]7733;";
/// A sequence longer than this is not ours (names are short); it is passed through as plain text.
const MAX_LEN: usize = 160;

pub fn emit(secret: &str, name: &str, color: [u8; 3]) -> Vec<u8> {
    format!("\x1b]7733;{secret};{};{:02x}{:02x}{:02x}\x07", hex(name.as_bytes()), color[0], color[1], color[2]).into_bytes()
}

pub fn new_secret() -> String {
    let mut b = [0u8; 12];
    let _ = getrandom::fill(&mut b);
    hex(&b)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Seen {
    pub name: String,
    pub color: [u8; 3],
}

/// Takes the sequences out of the bytes a pty prints. Holds back a sequence that is cut between two reads.
pub struct Scan {
    secret: String,
    pending: Vec<u8>,
}

impl Scan {
    pub fn new(secret: String) -> Self {
        Scan { secret, pending: Vec::new() }
    }

    /// The bytes to show, and the typing reports that were in them.
    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, Vec<Seen>) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(data.len());
        let mut seen = Vec::new();
        let mut i = 0;
        while i < data.len() {
            match find(&data[i..], PREFIX) {
                Some(p) => {
                    out.extend_from_slice(&data[i..i + p]);
                    let start = i + p;
                    let window = &data[start..data.len().min(start + MAX_LEN)];
                    match window.iter().position(|b| *b == 7) {
                        Some(e) => {
                            if let Some(s) = self.parse(&window[PREFIX.len()..e]) {
                                seen.push(s);
                            }
                            // one with a wrong secret is dropped too: it is not text for the screen either
                            i = start + e + 1;
                        }
                        None if data.len() - start < MAX_LEN => {
                            self.pending = data[start..].to_vec();
                            return (out, seen);
                        }
                        None => {
                            out.push(data[start]);
                            i = start + 1;
                        }
                    }
                }
                None => {
                    // the end may be the beginning of a sequence
                    let rest = &data[i..];
                    let keep = (1..PREFIX.len()).rev().find(|k| rest.len() >= *k && rest.ends_with(&PREFIX[..*k])).unwrap_or(0);
                    out.extend_from_slice(&rest[..rest.len() - keep]);
                    self.pending = rest[rest.len() - keep..].to_vec();
                    return (out, seen);
                }
            }
        }
        (out, seen)
    }

    fn parse(&self, body: &[u8]) -> Option<Seen> {
        let body = std::str::from_utf8(body).ok()?;
        let mut it = body.split(';');
        let (secret, name, color) = (it.next()?, it.next()?, it.next()?);
        if it.next().is_some() || secret != self.secret || color.len() != 6 {
            return None;
        }
        let c = unhex(color);
        if c.len() != 3 {
            return None;
        }
        let name = sync_core::relaywire::clean_name(&String::from_utf8_lossy(&unhex(name)));
        Some(Seen { name, color: [c[0], c[1], c[2]] })
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_is_taken_out_of_the_text() {
        let mut s = Scan::new("sec".into());
        let mut input = b"before".to_vec();
        input.extend(emit("sec", "Ann", [1, 2, 255]));
        input.extend(b"after");
        let (out, seen) = s.feed(&input);
        assert_eq!(out, b"beforeafter");
        assert_eq!(seen, vec![Seen { name: "Ann".into(), color: [1, 2, 255] }]);
    }

    #[test]
    fn a_report_cut_between_reads_is_still_found() {
        let whole = emit("sec", "Ann", [9, 8, 7]);
        for cut in 1..whole.len() {
            let mut s = Scan::new("sec".into());
            let (mut out, mut seen) = s.feed(&whole[..cut]);
            let (o2, s2) = s.feed(&whole[cut..]);
            out.extend(o2);
            seen.extend(s2);
            assert!(out.is_empty(), "cut at {cut} leaked {out:?}");
            assert_eq!(seen.len(), 1, "cut at {cut}");
        }
    }

    #[test]
    fn a_report_without_the_secret_is_dropped_and_shows_nothing() {
        let mut s = Scan::new("sec".into());
        let (out, seen) = s.feed(&emit("guess", "Eve", [0, 0, 0]));
        assert!(out.is_empty());
        assert!(seen.is_empty());
    }

    #[test]
    fn ordinary_escape_sequences_pass_untouched() {
        let mut s = Scan::new("sec".into());
        let input = b"\x1b[31mred\x1b[0m \x1b]0;title\x07 \x1b".to_vec();
        let (mut out, seen) = s.feed(&input);
        // the trailing ESC might begin a sequence, so it is held until more arrives
        let (more, _) = s.feed(b"[1mx");
        out.extend(more);
        assert!(seen.is_empty());
        assert_eq!(out, b"\x1b[31mred\x1b[0m \x1b]0;title\x07 \x1b[1mx");
    }

    #[test]
    fn an_unfinished_sequence_never_swallows_the_screen() {
        let mut s = Scan::new("sec".into());
        let mut input = PREFIX.to_vec();
        input.extend(vec![b'x'; 400]);
        let (out, _) = s.feed(&input);
        let (rest, _) = s.feed(b"");
        assert_eq!(out.len() + rest.len() + s.pending.len(), input.len());
        assert!(out.len() > 200, "the text came through");
    }
}

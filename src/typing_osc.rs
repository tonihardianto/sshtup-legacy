//! How a guest's terminal tab learns who is typing.
//!
//! The guest tab runs `--relay-join` in a pty. That program has no window of its own, so when the host reports
//! "Ann typed", it prints a private escape sequence to its own output and the app, which owns the pty, picks it up,
//! removes it from the screen and shows the badge. The sequence carries a random per-tab secret, so text printed by
//! the host's terminal (which also reaches this output) cannot make a badge appear.
//!
//!     ESC ] 7733 ; <secret> ; <kind> ; <json as hex> BEL
//!
//! `t` is "who just typed" (`{"n":name,"c":[r,g,b]}`), `r` the people in the session (a list of the same).

use sync_core::relaywire::{hex, unhex};

const PREFIX: &[u8] = b"\x1b]7733;";
/// A sequence longer than this is not ours (names are short); it is passed through as plain text.
const MAX_LEN: usize = 1500;

pub fn emit(secret: &str, report: &Report) -> Vec<u8> {
    let (kind, json) = match report {
        Report::Typing(p) => ('t', serde_json::to_string(&json_person(p))),
        Report::People(v) => ('r', serde_json::to_string(&v.iter().map(json_person).collect::<Vec<_>>())),
    };
    format!("\x1b]7733;{secret};{kind};{}\x07", hex(json.unwrap_or_default().as_bytes())).into_bytes()
}

fn json_person(p: &Person) -> serde_json::Value {
    serde_json::json!({"n": p.name, "c": p.color})
}

fn person(v: &serde_json::Value) -> Option<Person> {
    let c = v.get("c")?.as_array()?;
    let rgb: Vec<u8> = c.iter().map(|x| x.as_u64().unwrap_or(0).min(255) as u8).collect();
    let [r, g, b] = rgb.as_slice() else { return None };
    Some(Person { name: sync_core::relaywire::clean_name(v.get("n")?.as_str()?), color: [*r, *g, *b] })
}

pub fn new_secret() -> String {
    let mut b = [0u8; 12];
    let _ = getrandom::fill(&mut b);
    hex(&b)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// This person just typed.
    Typing(Person),
    /// Everyone in the session, the host first.
    People(Vec<Person>),
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
    pub fn feed(&mut self, chunk: &[u8]) -> (Vec<u8>, Vec<Report>) {
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

    fn parse(&self, body: &[u8]) -> Option<Report> {
        let body = std::str::from_utf8(body).ok()?;
        let mut it = body.split(';');
        let (secret, kind, data) = (it.next()?, it.next()?, it.next()?);
        if it.next().is_some() || secret != self.secret {
            return None;
        }
        decode(kind, &unhex(data))
    }
}

/// A report from its kind ("t" or "r") and its JSON.
pub fn decode(kind: &str, json: &[u8]) -> Option<Report> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    match kind {
        "t" => person(&v).map(Report::Typing),
        "r" => Some(Report::People(v.as_array()?.iter().filter_map(person).take(8).collect())),
        _ => None,
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(color: [u8; 3]) -> Person {
        Person { name: "Ann".into(), color }
    }

    #[test]
    fn a_report_is_taken_out_of_the_text() {
        let mut s = Scan::new("sec".into());
        let mut input = b"before".to_vec();
        input.extend(emit("sec", &Report::Typing(ann([1, 2, 255]))));
        input.extend(b"after");
        let (out, seen) = s.feed(&input);
        assert_eq!(out, b"beforeafter");
        assert_eq!(seen, vec![Report::Typing(ann([1, 2, 255]))]);
    }

    #[test]
    fn a_report_cut_between_reads_is_still_found() {
        let whole = emit("sec", &Report::People(vec![ann([9, 8, 7]), ann([1, 1, 1])]));
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
    fn the_list_of_people_arrives_whole() {
        let mut s = Scan::new("sec".into());
        let all = Report::People(vec![ann([1, 2, 3]), Person { name: "Bo".into(), color: [4, 5, 6] }]);
        let (out, seen) = s.feed(&emit("sec", &all));
        assert!(out.is_empty());
        assert_eq!(seen, vec![all]);
    }

    #[test]
    fn a_report_without_the_secret_is_dropped_and_shows_nothing() {
        let mut s = Scan::new("sec".into());
        let (out, seen) = s.feed(&emit("guess", &Report::Typing(ann([0, 0, 0]))));
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
        input.extend(vec![b'x'; 3000]);
        let (out, _) = s.feed(&input);
        let (rest, _) = s.feed(b"");
        assert_eq!(out.len() + rest.len() + s.pending.len(), input.len());
        assert!(out.len() > 1000,"the text came through");
    }
}

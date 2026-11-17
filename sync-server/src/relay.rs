//! The shared-terminal relay, part of the sync server: matches a host with its guests by session
//! code and passes terminal traffic between them.
//!
//! Who may use it: only people with an account on this server. The app asks the server's HTTP
//! API (signed in, so over whatever TLS the server sits behind) for a one-time ticket and shows
//! that ticket to the relay, so the account's real token never crosses the relay's plain TCP
//! port. Terminal bytes are sealed end to end by the clients (see `sync_core::relaywire`); the
//! relay only passes opaque text and never holds the key.

use sync_core::relaywire::{clean_name, read_msg, send, text};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::BufReader;
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICKET_TTL: Duration = Duration::from_secs(60);
const MAX_TICKETS: usize = 10_000;

/// One-time passes for the relay, handed out to signed-in accounts by the HTTP API.
#[derive(Default)]
pub struct Tickets {
    inner: Mutex<HashMap<Vec<u8>, Instant>>,
}

impl Tickets {
    pub fn issue(&self) -> String {
        let t = crate::auth::new_token();
        let now = Instant::now();
        let mut m = self.inner.lock().unwrap();
        m.retain(|_, at| now.duration_since(*at) < TICKET_TTL);
        if m.len() >= MAX_TICKETS {
            m.clear();
        }
        m.insert(crate::auth::sha(t.as_bytes()), now);
        t
    }

    /// True once per ticket, and only while it is fresh.
    pub fn redeem(&self, t: &str) -> bool {
        self.redeem_at(t, Instant::now())
    }

    pub fn redeem_at(&self, t: &str, now: Instant) -> bool {
        match self.inner.lock().unwrap().remove(&crate::auth::sha(t.as_bytes())) {
            Some(at) => now.duration_since(at) < TICKET_TTL,
            None => false,
        }
    }
}

pub const MAX_CONNECTIONS: usize = 256;
pub const MAX_GUESTS: usize = 8;
const QUEUE: usize = 2048;

struct Guest {
    id: u64,
    name: String,
    tx: SyncSender<Value>,
    active: bool,
    /// Whether this guest's keystrokes are passed on. New guests start view only.
    can_type: bool,
}

struct Session {
    host: SyncSender<Value>,
    can_type: bool,
    guests: Vec<Guest>,
    next: u64,
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;


// ----------------------------------------------------------------- guessing limit --
//
// A session code is 8 characters (about 40 bits). Failed attempts (a code that does not exist,
// a wrong access token) are counted per address: 5 failures within 10 minutes block that address
// for 10 minutes, and a blocked address gets no answer about any code, right or wrong.
// IPv6 addresses are counted per /64, because one machine usually owns the whole range.
// Behind an SSH tunnel every client looks like 127.0.0.1, so there the limit is shared.

pub const MAX_FAILS: u32 = 5;
const FAIL_WINDOW: Duration = Duration::from_secs(600);
const BLOCK_FOR: Duration = Duration::from_secs(600);
const MAX_TRACKED: usize = 10_000;

struct Entry {
    fails: u32,
    since: Instant,
    blocked_until: Option<Instant>,
}

#[derive(Default)]
pub struct Limiter {
    inner: Mutex<HashMap<IpAddr, Entry>>,
}

fn limit_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            // an IPv4 address in IPv6 clothing counts as the IPv4 address
            if let Some(v4) = v6.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            let mut o = v6.octets();
            o[8..].fill(0);
            IpAddr::V6(o.into())
        }
        v4 => v4,
    }
}

impl Limiter {
    pub fn is_blocked(&self, ip: IpAddr, now: Instant) -> bool {
        let mut m = self.inner.lock().unwrap();
        let key = limit_key(ip);
        match m.get(&key).and_then(|e| e.blocked_until) {
            Some(until) if until > now => true,
            Some(_) => {
                m.remove(&key); // block served: start again from zero
                false
            }
            None => false,
        }
    }

    pub fn record_failure(&self, ip: IpAddr, now: Instant) {
        let mut m = self.inner.lock().unwrap();
        if m.len() >= MAX_TRACKED {
            m.retain(|_, e| e.blocked_until.map(|u| u > now).unwrap_or(now.duration_since(e.since) < FAIL_WINDOW));
            while m.len() >= MAX_TRACKED {
                let Some(k) = m.keys().next().copied() else { break };
                m.remove(&k);
            }
        }
        let e = m.entry(limit_key(ip)).or_insert(Entry { fails: 0, since: now, blocked_until: None });
        if e.blocked_until.is_some() {
            return;
        }
        if now.duration_since(e.since) >= FAIL_WINDOW {
            e.fails = 0;
            e.since = now;
        }
        e.fails += 1;
        if e.fails >= MAX_FAILS {
            e.blocked_until = Some(now + BLOCK_FOR);
        }
    }
}

pub fn serve(listener: TcpListener, tickets: Arc<Tickets>) {
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let limiter = Arc::new(Limiter::default());
    let conns = Arc::new(AtomicUsize::new(0));
    for s in listener.incoming().flatten() {
        if conns.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            continue;
        }
        conns.fetch_add(1, Ordering::SeqCst);
        let (sessions, tickets, conns, limiter) = (sessions.clone(), tickets.clone(), conns.clone(), limiter.clone());
        std::thread::spawn(move || {
            handle(s, &sessions, &tickets, &limiter);
            conns.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

fn random_code() -> String {
    use std::io::Read;
    const AL: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut b = [0u8; 8];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok();
    if !ok {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        b = h.finish().to_le_bytes();
    }
    let mut s = String::new();
    for (i, x) in b.iter().enumerate() {
        if i == 4 {
            s.push('-');
        }
        s.push(AL[*x as usize % AL.len()] as char);
    }
    s
}

fn writer_thread(stream: TcpStream, rx: Receiver<Value>) {
    std::thread::spawn(move || {
        let mut w = stream;
        for v in rx {
            if send(&mut w, &v).is_err() {
                break;
            }
        }
        let _ = w.shutdown(Shutdown::Both);
    });
}

fn handle(stream: TcpStream, sessions: &Sessions, tickets: &Tickets, limiter: &Limiter) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let Ok(peer) = stream.peer_addr().map(|a| a.ip()) else { return };
    let Ok(rs) = stream.try_clone() else { return };
    let mut reader = BufReader::new(rs);
    let (tx, rx) = sync_channel::<Value>(QUEUE);
    writer_thread(stream, rx);
    let Some(first) = read_msg(&mut reader) else { return };
    let kind = text(&first, "t");
    if (kind == "host" || kind == "join") && limiter.is_blocked(peer, Instant::now()) {
        // no delay and no hint about the code or token: a blocked address learns nothing
        let _ = tx.send(json!({"t":"error","msg":"Too many failed attempts. Try again later."}));
        return;
    }
    if kind == "host" || kind == "join" {
        // a bad, used or expired ticket counts as a failed attempt like a wrong code does
        if !tickets.redeem(text(&first, "ticket")) {
            limiter.record_failure(peer, Instant::now());
            std::thread::sleep(Duration::from_millis(400));
            let _ = tx.send(json!({"t":"error","msg":"Not signed in to this server"}));
            return;
        }
    }
    match kind {
        "host" => host(&mut reader, tx, sessions),
        "join" => guest(&mut reader, tx, &first, sessions, (limiter, peer)),
        _ => {
            let _ = tx.send(json!({"t":"error","msg":"Unknown request"}));
        }
    }
}

fn host(reader: &mut BufReader<TcpStream>, tx: SyncSender<Value>, sessions: &Sessions) {
    let code = {
        let mut m = sessions.lock().unwrap();
        let mut code = random_code();
        while m.contains_key(&code) {
            code = random_code();
        }
        m.insert(code.clone(), Session { host: tx.clone(), can_type: false, guests: vec![], next: 1 });
        code
    };
    let _ = tx.send(json!({"t":"hosted","code":code}));
    // a host may stay quiet for a long time (an idle terminal)
    let _ = reader.get_ref().set_read_timeout(None);
    while let Some(m) = read_msg(reader) {
        let mut map = sessions.lock().unwrap();
        let Some(s) = map.get_mut(&code) else { break };
        match text(&m, "t") {
            "out" => {
                let msg = json!({"t":"out","d":text(&m,"d")});
                s.guests.retain(|g| !g.active || g.tx.try_send(msg.clone()).is_ok());
            }
            kind @ ("typing" | "roster") => {
                // sealed by the host: who just typed / who is in the session, for the guests' windows to show
                let msg = json!({"t":kind,"d":text(&m,"d")});
                s.guests.retain(|g| !g.active || g.tx.try_send(msg.clone()).is_ok());
            }
            "snap" => {
                let gid = m.get("gid").and_then(|x| x.as_u64()).unwrap_or(0);
                if let Some(g) = s.guests.iter_mut().find(|g| g.id == gid) {
                    let _ = g.tx.try_send(json!({"t":"out","d":text(&m,"d")}));
                    g.active = true;
                }
            }
            "perm" => {
                let on = m.get("can_type").and_then(|x| x.as_bool()).unwrap_or(false);
                let msg = json!({"t":"perm","can_type":on});
                match m.get("gid").and_then(|x| x.as_u64()) {
                    // one guest
                    Some(gid) => {
                        if let Some(g) = s.guests.iter_mut().find(|g| g.id == gid) {
                            g.can_type = on;
                            let _ = g.tx.try_send(msg);
                        }
                    }
                    // everyone, and whoever joins next
                    None => {
                        s.can_type = on;
                        for g in s.guests.iter_mut() {
                            g.can_type = on;
                            let _ = g.tx.try_send(msg.clone());
                        }
                    }
                }
            }
            "end" => break,
            _ => {}
        }
    }
    if let Some(s) = sessions.lock().unwrap().remove(&code) {
        for g in s.guests {
            let _ = g.tx.try_send(json!({"t":"ended"}));
        }
    }
}

fn guest(reader: &mut BufReader<TcpStream>, tx: SyncSender<Value>, first: &Value, sessions: &Sessions, who: (&Limiter, IpAddr)) {
    let code = text(first, "code").trim().to_lowercase();
    let name = clean_name(text(first, "name"));
    let gid = {
        let mut m = sessions.lock().unwrap();
        let Some(s) = m.get_mut(&code) else {
            drop(m);
            who.0.record_failure(who.1, Instant::now());
            std::thread::sleep(Duration::from_millis(400));
            let _ = tx.send(json!({"t":"error","msg":"No session with that code"}));
            return;
        };
        if s.guests.len() >= MAX_GUESTS {
            let _ = tx.send(json!({"t":"error","msg":"This session is full"}));
            return;
        }
        let id = s.next;
        s.next += 1;
        s.guests.push(Guest { id, name: name.clone(), tx: tx.clone(), active: false, can_type: s.can_type });
        let _ = tx.send(json!({"t":"joined","can_type":s.can_type,"gid":id}));
        let _ = s.host.try_send(json!({"t":"guest","gid":id,"name":name,"count":s.guests.len()}));
        id
    };
    let _ = reader.get_ref().set_read_timeout(None);
    while let Some(m) = read_msg(reader) {
        if text(&m, "t") == "in" {
            let map = sessions.lock().unwrap();
            if let Some(s) = map.get(&code) {
                // the sender's id rides along so the host can tell who is typing
                if s.guests.iter().find(|g| g.id == gid).map(|g| g.can_type).unwrap_or(false) {
                    let _ = s.host.try_send(json!({"t":"in","gid":gid,"d":text(&m,"d")}));
                } else {
                    let _ = tx.try_send(json!({"t":"denied"}));
                }
            }
        }
    }
    if let Some(s) = sessions.lock().unwrap().get_mut(&code) {
        s.guests.retain(|g| g.id != gid);
        let _ = s.host.try_send(json!({"t":"left","gid":gid,"name":name,"count":s.guests.len()}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sync_core::relaywire::{hex, open, seal, unhex, Dir, new_key};

    fn connect(addr: &str) -> (BufReader<TcpStream>, TcpStream) {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        (BufReader::new(s.try_clone().unwrap()), s)
    }

    fn start() -> (String, Arc<Tickets>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let t = Arc::new(Tickets::default());
        let t2 = t.clone();
        std::thread::spawn(move || serve(l, t2));
        (addr, t)
    }

    #[test]
    fn host_and_guest_exchange_traffic() {
        let (addr, tk) = start();

        // a made-up ticket is refused
        let (mut r, mut w) = connect(&addr);
        send(&mut w, &json!({"t":"host","ticket":"nope"})).unwrap();
        assert_eq!(text(&read_msg(&mut r).unwrap(), "t"), "error");

        // no ticket at all is refused too
        let (mut r, mut w) = connect(&addr);
        send(&mut w, &json!({"t":"host"})).unwrap();
        assert_eq!(text(&read_msg(&mut r).unwrap(), "t"), "error");

        // host
        let (mut hr, mut hw) = connect(&addr);
        send(&mut hw, &json!({"t":"host","ticket":tk.issue()})).unwrap();
        let hosted = read_msg(&mut hr).unwrap();
        assert_eq!(text(&hosted, "t"), "hosted");
        let code = text(&hosted, "code").to_string();
        assert_eq!(code.len(), 9);

        // unknown code
        let (mut br, mut bw) = connect(&addr);
        send(&mut bw, &json!({"t":"join","ticket":tk.issue(),"code":"zzzz-zzzz"})).unwrap();
        assert_eq!(text(&read_msg(&mut br).unwrap(), "t"), "error");

        // a guest without a ticket cannot join even with the right code
        let (mut nr, mut nw) = connect(&addr);
        send(&mut nw, &json!({"t":"join","code":code,"name":"eve"})).unwrap();
        assert!(text(&read_msg(&mut nr).unwrap(), "msg").contains("Not signed in"));

        // guest joins; host is told, answers with a snapshot
        let (mut gr, mut gw) = connect(&addr);
        send(&mut gw, &json!({"t":"join","ticket":tk.issue(),"code":code,"name":"ann"})).unwrap();
        assert_eq!(text(&read_msg(&mut gr).unwrap(), "t"), "joined");
        let ev = read_msg(&mut hr).unwrap();
        assert_eq!(text(&ev, "t"), "guest");
        assert_eq!(text(&ev, "name"), "ann");
        let gid = ev["gid"].as_u64().unwrap();
        send(&mut hw, &json!({"t":"snap","gid":gid,"d":hex(b"hello")})).unwrap();
        let m = read_msg(&mut gr).unwrap();
        assert_eq!(unhex(text(&m, "d")), b"hello");

        // live output reaches the guest
        send(&mut hw, &json!({"t":"out","d":hex(b"abc")})).unwrap();
        let m = read_msg(&mut gr).unwrap();
        assert_eq!(unhex(text(&m, "d")), b"abc");

        // typing is refused until the host allows it
        send(&mut gw, &json!({"t":"in","d":hex(b"x")})).unwrap();
        assert_eq!(text(&read_msg(&mut gr).unwrap(), "t"), "denied");
        send(&mut hw, &json!({"t":"perm","gid":gid,"can_type":true})).unwrap();
        assert_eq!(text(&read_msg(&mut gr).unwrap(), "t"), "perm");
        send(&mut gw, &json!({"t":"in","d":hex(b"y")})).unwrap();
        let m = read_msg(&mut hr).unwrap();
        assert_eq!(text(&m, "t"), "in");
        assert_eq!(m["gid"].as_u64(), Some(gid), "the host is told who typed");
        assert_eq!(unhex(text(&m, "d")), b"y");

        // who typed is passed to the guests as it is (sealed by the host)
        send(&mut hw, &json!({"t":"typing","d":hex(b"who")})).unwrap();
        let m = read_msg(&mut gr).unwrap();
        assert_eq!(text(&m, "t"), "typing");
        assert_eq!(unhex(text(&m, "d")), b"who");

        // ending the session tells the guest
        send(&mut hw, &json!({"t":"end"})).unwrap();
        assert_eq!(text(&read_msg(&mut gr).unwrap(), "t"), "ended");
    }

    #[test]
    fn permission_is_per_guest() {
        let (addr, tk) = start();
        let (mut hr, mut hw) = connect(&addr);
        send(&mut hw, &json!({"t":"host","ticket":tk.issue()})).unwrap();
        let code = text(&read_msg(&mut hr).unwrap(), "code").to_string();
        let join = |name: &str| {
            let (mut r, mut w) = connect(&addr);
            send(&mut w, &json!({"t":"join","ticket":tk.issue(),"code":code,"name":name})).unwrap();
            assert_eq!(text(&read_msg(&mut r).unwrap(), "t"), "joined");
            (r, w)
        };
        let (mut ar, mut aw) = join("ann");
        let ga = read_msg(&mut hr).unwrap()["gid"].as_u64().unwrap();
        let (mut br, mut bw) = join("bob");
        let gb = read_msg(&mut hr).unwrap()["gid"].as_u64().unwrap();
        assert_ne!(ga, gb);

        // only ann may type
        send(&mut hw, &json!({"t":"perm","gid":ga,"can_type":true})).unwrap();
        assert_eq!(text(&read_msg(&mut ar).unwrap(), "t"), "perm");
        send(&mut bw, &json!({"t":"in","d":hex(b"b")})).unwrap();
        assert_eq!(text(&read_msg(&mut br).unwrap(), "t"), "denied", "bob is still view only");
        send(&mut aw, &json!({"t":"in","d":hex(b"a")})).unwrap();
        let m = read_msg(&mut hr).unwrap();
        assert_eq!(m["gid"].as_u64(), Some(ga));
        assert_eq!(unhex(text(&m, "d")), b"a");

        // taking it back stops ann at once
        send(&mut hw, &json!({"t":"perm","gid":ga,"can_type":false})).unwrap();
        let m = read_msg(&mut ar).unwrap();
        assert_eq!((text(&m, "t"), m["can_type"].as_bool()), ("perm", Some(false)));
        send(&mut aw, &json!({"t":"in","d":hex(b"a")})).unwrap();
        assert_eq!(text(&read_msg(&mut ar).unwrap(), "t"), "denied");
    }

    #[test]
    fn sealed_traffic_crosses_the_relay_untouched() {
        let (addr, tk) = start();
        let key = new_key();
        let (mut hr, mut hw) = connect(&addr);
        send(&mut hw, &json!({"t":"host","ticket":tk.issue()})).unwrap();
        let code = text(&read_msg(&mut hr).unwrap(), "code").to_string();
        let (mut gr, mut gw) = connect(&addr);
        send(&mut gw, &json!({"t":"join","ticket":tk.issue(),"code":code,"name":"ann"})).unwrap();
        read_msg(&mut gr).unwrap();
        let gid = read_msg(&mut hr).unwrap()["gid"].as_u64().unwrap();
        send(&mut hw, &json!({"t":"snap","gid":gid,"d":seal(&key, Dir::Out, b"")})).unwrap();
        read_msg(&mut gr).unwrap();

        let sealed = seal(&key, Dir::Out, b"secret output");
        send(&mut hw, &json!({"t":"out","d":sealed})).unwrap();
        let m = read_msg(&mut gr).unwrap();
        assert_eq!(text(&m, "d"), sealed, "the relay forwards it byte for byte");
        assert!(!text(&m, "d").contains(&hex(b"secret")));
        assert_eq!(open(&key, Dir::Out, text(&m, "d")).unwrap(), b"secret output");
        assert!(open(&new_key(), Dir::Out, text(&m, "d")).is_none(), "a different key opens nothing");
    }

    #[test]
    fn a_ticket_works_once() {
        let (addr, tk) = start();
        let t = tk.issue();
        let (mut r, mut w) = connect(&addr);
        send(&mut w, &json!({"t":"host","ticket":t})).unwrap();
        assert_eq!(text(&read_msg(&mut r).unwrap(), "t"), "hosted");
        let (mut r2, mut w2) = connect(&addr);
        send(&mut w2, &json!({"t":"host","ticket":t})).unwrap();
        assert_eq!(text(&read_msg(&mut r2).unwrap(), "t"), "error", "a used ticket is dead");
    }

    #[test]
    fn tickets_expire() {
        let tk = Tickets::default();
        let t = tk.issue();
        assert!(!tk.redeem_at(&t, Instant::now() + Duration::from_secs(61)));
        assert!(!tk.redeem(&t), "an expired ticket is gone");
        let t = tk.issue();
        assert!(tk.redeem_at(&t, Instant::now() + Duration::from_secs(30)));
        assert!(!tk.redeem("garbage"));
    }

    #[test]
    fn limiter_blocks_after_repeated_failures_and_forgets_later() {
        let l = Limiter::default();
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let other: IpAddr = "203.0.113.10".parse().unwrap();
        let t0 = Instant::now();
        for i in 0..MAX_FAILS {
            assert!(!l.is_blocked(ip, t0), "not blocked before failure {i}");
            l.record_failure(ip, t0);
        }
        assert!(l.is_blocked(ip, t0));
        assert!(!l.is_blocked(other, t0), "another address is unaffected");
        assert!(l.is_blocked(ip, t0 + Duration::from_secs(599)));
        assert!(!l.is_blocked(ip, t0 + Duration::from_secs(601)), "block ends");
        // failures spread over more than the window never add up
        let l = Limiter::default();
        for i in 0..20u64 {
            l.record_failure(ip, t0 + Duration::from_secs(i * 700));
            assert!(!l.is_blocked(ip, t0 + Duration::from_secs(i * 700)));
        }
        // one /64 is one client; an IPv4-mapped address is the IPv4 address
        let l = Limiter::default();
        for i in 0..MAX_FAILS {
            l.record_failure(format!("2001:db8::{i:x}").parse().unwrap(), t0);
        }
        assert!(l.is_blocked("2001:db8::ffff".parse().unwrap(), t0));
        assert!(!l.is_blocked("2001:db8:0:1::1".parse().unwrap(), t0));
        for _ in 0..MAX_FAILS {
            l.record_failure("::ffff:198.51.100.7".parse().unwrap(), t0);
        }
        assert!(l.is_blocked("198.51.100.7".parse().unwrap(), t0));
    }

    #[test]
    fn guessing_codes_gets_an_address_blocked_even_for_a_right_code() {
        let (addr, tk) = start();
        let (mut hr, mut hw) = connect(&addr);
        send(&mut hw, &json!({"t":"host","ticket":tk.issue()})).unwrap();
        let code = text(&read_msg(&mut hr).unwrap(), "code").to_string();

        for _ in 0..MAX_FAILS {
            let (mut r, mut w) = connect(&addr);
            send(&mut w, &json!({"t":"join","ticket":tk.issue(),"code":"aaaa-aaaa"})).unwrap();
            assert!(text(&read_msg(&mut r).unwrap(), "msg").contains("No session"));
        }
        // now even the real code is refused, and the answer does not depend on the code
        let (mut r, mut w) = connect(&addr);
        send(&mut w, &json!({"t":"join","ticket":tk.issue(),"code":code})).unwrap();
        let m = read_msg(&mut r).unwrap();
        assert_eq!(text(&m, "t"), "error");
        assert!(text(&m, "msg").contains("Too many"), "{m}");
        // hosting is blocked from the same address too
        let (mut r, mut w) = connect(&addr);
        send(&mut w, &json!({"t":"host","ticket":tk.issue()})).unwrap();
        assert!(text(&read_msg(&mut r).unwrap(), "msg").contains("Too many"));
    }
}

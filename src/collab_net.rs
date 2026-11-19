//! Host side of a shared terminal: connects to the relay that runs inside the sync server,
//! forwards what the terminal prints (sealed end to end, the relay never holds the key),
//! tells the guest list, and writes allowed guest keystrokes into the terminal.

use crate::term::Shared;
use crate::wire::{new_key, open, read_msg, seal, send, share_string, text, Dir};
use eframe::egui;
use serde_json::{json, Value};
use std::io::{BufReader, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Connecting,
    Live,
    Failed,
}

/// One guest as the host sees them.
#[derive(Clone, Debug)]
pub struct GuestInfo {
    pub gid: u64,
    pub name: String,
    /// New guests start view only; the host turns typing on for each one.
    pub can_type: bool,
}

/// A colour per guest, stable for the whole session (the host's own avatar is blue).
pub fn guest_color(gid: u64) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 8] = [[255, 196, 61], [240, 98, 146], [77, 208, 140], [186, 140, 255], [255, 138, 76], [64, 210, 220], [180, 220, 70], [255, 120, 120]];
    PALETTE[(gid as usize) % PALETTE.len()]
}

pub struct LinkState {
    pub status: Status,
    pub code: String,
    pub guests: Vec<GuestInfo>,
    pub error: String,
}

/// What the link's own thread needs. It must not own the `Guard`: the guard's `Drop` is what
/// ends the session when the last `HostLink` goes away, and a thread holding it would keep it alive forever.
struct Inner {
    ctl: Mutex<Option<Sender<Value>>>,
    stopped: AtomicBool,
}

struct Guard {
    inner: Arc<Inner>,
    shared: Arc<Shared>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut t) = self.shared.tap.lock() {
            *t = None;
        }
        if let Some(c) = self.inner.ctl.lock().unwrap().as_ref() {
            let _ = c.send(json!({"t":"end"}));
        }
    }
}

#[derive(Clone)]
pub struct HostLink {
    pub state: Arc<Mutex<LinkState>>,
    guard: Arc<Guard>,
}

impl HostLink {
    /// Let one guest type, or take it back. Takes effect on the host's side at once.
    pub fn set_guest_can_type(&self, gid: u64, on: bool) {
        if let Some(g) = self.state.lock().unwrap().guests.iter_mut().find(|g| g.gid == gid) {
            g.can_type = on;
        }
        if let Some(c) = self.guard.inner.ctl.lock().unwrap().as_ref() {
            let _ = c.send(json!({"t":"perm","gid":gid,"can_type":on}));
        }
    }
}

fn fail(state: &Arc<Mutex<LinkState>>, ctx: &egui::Context, msg: &str) {
    let mut s = state.lock().unwrap();
    s.status = Status::Failed;
    s.error = msg.to_string();
    ctx.request_repaint();
}

pub fn start_host(
    server: String,
    token: String,
    parser: Arc<Mutex<vt100::Parser>>,
    pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    shared: Arc<Shared>,
    ctx: egui::Context,
    me: String,
) -> HostLink {
    let state = Arc::new(Mutex::new(LinkState { status: Status::Connecting, code: String::new(), guests: vec![], error: String::new() }));
    let inner = Arc::new(Inner { ctl: Mutex::new(None), stopped: AtomicBool::new(false) });
    let guard = Arc::new(Guard { inner: inner.clone(), shared: shared.clone() });
    let link = HostLink { state: state.clone(), guard };
    std::thread::spawn(move || {
        // sign in to the relay with a one-time ticket from the sync server
        let pass = match sync_client::api::Api::new(&server, Some(token)).relay_pass() {
            Ok(p) => p,
            Err(e) => return fail(&state, &ctx, &crate::sync::runtime::describe(&e)),
        };
        let addr = pass.addr;
        let key = new_key();
        let stream = addr
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(8)).ok()));
        let Some(stream) = stream else {
            return fail(&state, &ctx, "Could not reach the relay");
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let Ok(mut w) = stream.try_clone() else { return fail(&state, &ctx, "Connection failed") };
        let mut r = BufReader::new(stream);
        if send(&mut w, &json!({"t":"host","ticket":pass.ticket})).is_err() {
            return fail(&state, &ctx, "Connection lost");
        }
        let code = match read_msg(&mut r) {
            Some(m) if text(&m, "t") == "hosted" => share_string(text(&m, "code"), &key),
            Some(m) => return fail(&state, &ctx, if text(&m, "msg").is_empty() { "Relay refused" } else { text(&m, "msg") }),
            None => return fail(&state, &ctx, "The relay did not answer"),
        };
        let _ = r.get_ref().set_read_timeout(None);
        if inner.stopped.load(Ordering::SeqCst) {
            let _ = send(&mut w, &json!({"t":"end"}));
            return;
        }
        let (otx, orx) = channel::<Value>();
        let (ttx, trx) = channel::<Vec<u8>>();
        *inner.ctl.lock().unwrap() = Some(otx.clone());
        *shared.tap.lock().unwrap() = Some(ttx);
        {
            let mut s = state.lock().unwrap();
            s.status = Status::Live;
            s.code = code;
        }
        ctx.request_repaint();
        // terminal output -> relay
        let o2 = otx.clone();
        std::thread::spawn(move || {
            for chunk in trx {
                if o2.send(json!({"t":"out","d":seal(&key, Dir::Out, &chunk)})).is_err() {
                    break;
                }
            }
        });
        // everything outgoing -> socket
        std::thread::spawn(move || {
            for v in orx {
                let end = v.get("t").and_then(|x| x.as_str()) == Some("end");
                if send(&mut w, &v).is_err() || end {
                    break;
                }
            }
            let _ = w.shutdown(Shutdown::Both);
        });
        // relay -> us
        let mut last_told: Option<(u64, Instant)> = None;
        while let Some(m) = read_msg(&mut r) {
            match text(&m, "t") {
                "guest" => {
                    let gid = m.get("gid").and_then(|x| x.as_u64()).unwrap_or(0);
                    let snap = parser.lock().map(|p| p.screen().contents_formatted()).unwrap_or_default();
                    let _ = otx.send(json!({"t":"snap","gid":gid,"d":seal(&key, Dir::Out, snap.as_bytes())}));
                    state.lock().unwrap().guests.push(GuestInfo { gid, name: text(&m, "name").to_string(), can_type: false });
                    tell_roster(&state, &otx, &key, &me);
                }
                "left" => {
                    let gid = m.get("gid").and_then(|x| x.as_u64()).unwrap_or(0);
                    state.lock().unwrap().guests.retain(|g| g.gid != gid);
                    tell_roster(&state, &otx, &key, &me);
                }
                "in" => {
                    // checked here as well as at the relay: a guest without permission is dropped,
                    // whatever the relay passed on
                    let gid = m.get("gid").and_then(|x| x.as_u64()).unwrap_or(0);
                    let who = state.lock().unwrap().guests.iter().find(|g| g.gid == gid && g.can_type).map(|g| g.name.clone());
                    if let Some(name) = who {
                        // a chunk that does not open with the session key is not from a guest who has it
                        if let (Some(keys), Ok(mut pw)) = (open(&key, Dir::In, text(&m, "d")), pty_writer.lock()) {
                            let _ = pw.write_all(&keys);
                            let _ = pw.flush();
                            let color = guest_color(gid);
                            // the guests' windows show the same badge; told at most twice a second per person
                            if last_told.map(|(g, at)| g != gid || at.elapsed() > Duration::from_millis(500)).unwrap_or(true) {
                                last_told = Some((gid, Instant::now()));
                                let who = json!({"n": name, "c": color}).to_string();
                                let _ = otx.send(json!({"t":"typing","d":seal(&key, Dir::Out, who.as_bytes())}));
                            }
                            if let Ok(mut t) = shared.typing.lock() {
                                *t = Some(crate::term::Typing { name, color, at: Instant::now() });
                            }
                        }
                    }
                }
                _ => {}
            }
            ctx.request_repaint();
        }
        if !inner.stopped.load(Ordering::SeqCst) {
            fail(&state, &ctx, "The relay closed the connection");
        }
        if let Ok(mut t) = shared.tap.lock() {
            *t = None;
        }
    });
    link
}

/// Tells the guests who is in the session (the host first), for the avatars on their tabs.
fn tell_roster(state: &Mutex<LinkState>, otx: &std::sync::mpsc::Sender<Value>, key: &[u8; crate::wire::KEY_LEN], host: &str) {
    let mut list = vec![json!({"n": host, "c": [0x20, 0x91, 0xF6]})];
    for g in state.lock().unwrap().guests.iter() {
        list.push(json!({"n": g.name, "c": guest_color(g.gid)}));
    }
    let _ = otx.send(json!({"t":"roster","d":seal(key, Dir::Out, Value::Array(list).to_string().as_bytes())}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    fn account(base: &str, db: &sync_server::db::Db, email: &str) -> String {
        sync_client::account::register(&sync_client::api::Api::new(base, None), email, "a long enough password", &sync_server::create_invite(db)).unwrap();
        sync_client::account::login(&sync_client::api::Api::new(base, None), email, "a long enough password", "d").unwrap().token
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let t = Instant::now();
        while !ok() {
            assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The real host code, the real relay inside the sync server and the real guest program, with two accounts.
    #[test]
    fn a_guest_with_an_account_sees_and_types_into_the_hosts_terminal() {
        // the guest is this same program run in join mode; `cargo build --bin sshtup` makes it
        let guest_exe = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("target/debug/sshtup{}", std::env::consts::EXE_SUFFIX));
        assert!(guest_exe.exists(), "build the program first: cargo build --bin sshtup ({} is missing)", guest_exe.display());
        let db = sync_server::db::Db::memory().unwrap();
        let base = sync_server::spawn_server_relay(db.clone(), true);
        let (host_tok, guest_tok) = (account(&base, &db, "host@x.io"), account(&base, &db, "guest@x.io"));
        share_and_join(&base, host_tok, guest_tok);
    }

    /// The same walk-through against a real, deployed server (one account may be host and guest at once).
    /// TC_SERVER=https://… TC_EMAIL=… TC_PASSWORD=… [TC_INVITE=code to register first]
    /// cargo test --bin sshtup against_a_real_server -- --ignored --nocapture
    #[test]
    #[ignore]
    fn against_a_real_server() {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
        let (server, email, pw) = (env("TC_SERVER"), env("TC_EMAIL"), env("TC_PASSWORD"));
        let api = sync_client::api::Api::new(&server, None);
        if let Ok(code) = std::env::var("TC_INVITE") {
            sync_client::account::register(&api, &email, &pw, &code).expect("register");
            println!("registered {email}");
        }
        let tok = sync_client::account::login(&api, &email, &pw, "relay-test").expect("login").token;
        share_and_join(&server, tok.clone(), tok);
        println!("host, relay and guest all worked through {server}");
    }

    fn share_and_join(base: &str, host_tok: String, guest_tok: String) {
        let guest_exe = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("target/debug/sshtup{}", std::env::consts::EXE_SUFFIX));
        let base = base.to_string();
        // the host's terminal: a screen, a place where typed bytes land, and the output tap
        let parser = Arc::new(Mutex::new(vt100::Parser::new(std::num::NonZeroU16::new(24).unwrap(), std::num::NonZeroU16::new(80).unwrap(), 0)));
        parser.lock().unwrap().process(b"already on screen");
        let typed = Arc::new(Mutex::new(Vec::<u8>::new()));
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(Sink(typed.clone()))));
        let shared = Arc::new(Shared {
            prompt: Mutex::new(None),
            answered_at: Mutex::new(None),
            login_seen: Mutex::new(None),
            alive: AtomicBool::new(true),
            bytes: std::sync::atomic::AtomicUsize::new(0),
            bell: std::sync::atomic::AtomicU64::new(0),
            last_output: Mutex::new(Instant::now()),
            exit_code: Mutex::new(None),
            tap: Mutex::new(None),
            typing: Mutex::new(None),
            people: Mutex::new(Vec::new()),
        });
        let link = start_host(base.clone(), host_tok, parser, writer, shared.clone(), egui::Context::default(), "host".into());
        wait_for("the host to go live", || link.state.lock().unwrap().status != Status::Connecting);
        let st = link.state.lock().unwrap();
        assert_eq!(st.status, Status::Live, "{}", st.error);
        let share = st.code.clone();
        drop(st);
        assert!(share.contains('~'), "the code carries the key: {share}");

        // the guest: its own account, its own ticket, the real program
        let pass = sync_client::api::Api::new(&base, Some(guest_tok)).relay_pass().unwrap();
        let mut guest = Command::new(&guest_exe).args(["--relay-join", &pass.addr, &share, "ann", &pass.ticket]).env("SSHTUP_TYPING_SECRET", "s3cret").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
        let (seen2, mut out) = (seen.clone(), guest.stdout.take().unwrap());
        std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            while let Ok(n) = out.read(&mut b) {
                if n == 0 {
                    break;
                }
                seen2.lock().unwrap().extend_from_slice(&b[..n]);
            }
        });
        let shows = |s: &str| String::from_utf8_lossy(&seen.lock().unwrap()).contains(s);

        wait_for("the guest to see the screen that was already there", || shows("already on screen"));
        wait_for("the host to list the guest", || link.state.lock().unwrap().guests.iter().any(|g| g.name == "ann"));
        // the guest's window is told who is in the session: the host first, then the guests
        {
            use crate::typing_osc::{emit, Person, Report};
            let gid = link.state.lock().unwrap().guests.iter().find(|g| g.name == "ann").unwrap().gid;
            let roster = emit("s3cret", &Report::People(vec![Person { name: "host".into(), color: [0x20, 0x91, 0xF6] }, Person { name: "ann".into(), color: guest_color(gid) }]));
            wait_for("the guest window to be told who is here", || seen.lock().unwrap().windows(roster.len()).any(|w| w == roster.as_slice()));
        }
        shared.tap.lock().unwrap().as_ref().unwrap().send(b"LIVE-OUTPUT".to_vec()).unwrap();
        wait_for("live output on the guest", || shows("LIVE-OUTPUT"));

        // view only: keystrokes go nowhere
        let mut keys = guest.stdin.take().unwrap();
        keys.write_all(b"nope").unwrap();
        keys.flush().unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert!(typed.lock().unwrap().is_empty(), "a guest cannot type until the host allows it");
        assert!(shared.typing.lock().unwrap().is_none(), "no badge for keys that went nowhere");
        // allowed (for that guest only): they arrive, and the host sees who typed
        let gid = link.state.lock().unwrap().guests.iter().find(|g| g.name == "ann").unwrap().gid;
        link.set_guest_can_type(gid, true);
        std::thread::sleep(Duration::from_millis(300));
        keys.write_all(b"ls\n").unwrap();
        keys.flush().unwrap();
        wait_for("typed bytes at the host", || typed.lock().unwrap().ends_with(b"ls\n"));
        let badge = shared.typing.lock().unwrap().clone().expect("the host sees who typed");
        assert_eq!((badge.name.as_str(), badge.color), ("ann", guest_color(gid)));
        // and the guest's own window is told, in the private sequence its app looks for
        let told = crate::typing_osc::emit("s3cret", &crate::typing_osc::Report::Typing(crate::typing_osc::Person { name: "ann".into(), color: guest_color(gid) }));
        wait_for("the guest window to be told who typed", || seen.lock().unwrap().windows(told.len()).any(|w| w == told.as_slice()));
        // taking it back stops the keys again
        link.set_guest_can_type(gid, false);
        std::thread::sleep(Duration::from_millis(300));
        let before = typed.lock().unwrap().len();
        keys.write_all(b"rm\n").unwrap();
        keys.flush().unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(typed.lock().unwrap().len(), before, "a guest whose permission was taken back is dropped");

        // the host stops sharing: the guest is told and leaves
        drop(link);
        wait_for("the guest to be told", || shows("The host ended the session"));
        wait_for("the guest program to exit", || guest.try_wait().unwrap().is_some());
    }
}

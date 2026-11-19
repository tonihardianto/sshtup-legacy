//! Guest side of a shared terminal. A guest tab simply runs this program in its terminal:
//! it shows the host's terminal and, when the host allows it, sends keystrokes back.

use crate::wire::{open, parse_share, read_msg, seal, send, text, Dir};
use serde_json::json;
use std::io::{BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn run(addr: &str, share: &str, name: &str, ticket: &str) -> i32 {
    let Some((code, key)) = parse_share(share) else {
        println!("That is not a full session code. It must include the part after the ~ sign.");
        return 1;
    };
    let stream = match addr.to_socket_addrs().ok().and_then(|mut a| a.find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(8)).ok())) {
        Some(s) => s,
        None => {
            println!("Could not reach the relay at {addr}");
            return 1;
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let mut w = stream.try_clone().unwrap();
    let mut r = BufReader::new(stream);
    if send(&mut w, &json!({"t":"join","ticket":ticket,"code":code,"name":name})).is_err() {
        println!("Connection lost");
        return 1;
    }
    match read_msg(&mut r) {
        Some(m) if text(&m, "t") == "joined" => {}
        Some(m) => {
            println!("{}", text(&m, "msg"));
            return 1;
        }
        None => {
            println!("The relay did not answer");
            return 1;
        }
    }
    let _ = r.get_ref().set_read_timeout(None);
    let _ = Command::new("stty").args(["raw", "-echo"]).stdin(Stdio::inherit()).status();
    print!("Joined. You see the host's terminal; typing works only if the host allows it.\r\n");
    let _ = std::io::stdout().flush();
    let mut keys = w.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 256];
        let mut stdin = std::io::stdin();
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 || send(&mut keys, &json!({"t":"in","d":seal(&key, Dir::In, &buf[..n])})).is_err() {
                break;
            }
        }
    });
    let mut out = std::io::stdout();
    // set by the app that owns this terminal tab; reports of who is typing are only printed when it is there
    let secret = std::env::var("SSHTUP_TYPING_SECRET").ok().filter(|s| !s.is_empty());
    while let Some(m) = read_msg(&mut r) {
        match text(&m, "t") {
            "out" => {
                if let Some(bytes) = open(&key, Dir::Out, text(&m, "d")) {
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
            }
            kind @ ("typing" | "roster") => {
                // sealed by the host: who just typed / who is in the session, for the app that owns this tab to draw
                if let (Some(secret), Some(raw)) = (secret.as_deref(), open(&key, Dir::Out, text(&m, "d"))) {
                    if let Some(report) = crate::typing_osc::decode(if kind == "typing" { "t" } else { "r" }, &raw) {
                        let _ = out.write_all(&crate::typing_osc::emit(secret, &report));
                        let _ = out.flush();
                    }
                }
            }
            "ended" => {
                print!("\r\n[The host ended the session]\r\n");
                break;
            }
            _ => {}
        }
    }
    let _ = Command::new("stty").arg("sane").stdin(Stdio::inherit()).status();
    0
}

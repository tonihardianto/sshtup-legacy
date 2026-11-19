use std::path::PathBuf;
use sync_server::{create_invite, db::Db, serve_with, start_relay};

fn usage() -> ! {
    eprintln!("usage:\n  sshtup_sync_server serve --db <file> [--listen 127.0.0.1:8787] [--relay-listen 0.0.0.0:7700] [--trust-proxy]\n  sshtup_sync_server invite --db <file>\n  sshtup_sync_server backup --db <file> --out <new file>");
    std::process::exit(2);
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    let Some(path) = flag(&args, "--db") else { usage() };
    let db = Db::open(&PathBuf::from(path)).unwrap_or_else(|e| {
        eprintln!("cannot open the database: {e}");
        std::process::exit(1);
    });
    match cmd.as_str() {
        "invite" => println!("{}", create_invite(&db)),
        "backup" => {
            let Some(out) = flag(&args, "--out") else { usage() };
            match db.backup_to(&PathBuf::from(&out)) {
                Ok(()) => eprintln!("backup written to {out}"),
                Err(e) => {
                    eprintln!("backup failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        "serve" => {
            let listen = flag(&args, "--listen").unwrap_or_else(|| "127.0.0.1:8787".into());
            let l = tokio::net::TcpListener::bind(&listen).await.unwrap_or_else(|e| {
                eprintln!("cannot listen on {listen}: {e}");
                std::process::exit(1);
            });
            // the shared-terminal relay is optional and lives on its own TCP port
            let relay = flag(&args, "--relay-listen").map(|addr| {
                let rl = std::net::TcpListener::bind(&addr).unwrap_or_else(|e| {
                    eprintln!("cannot listen on {addr} for the relay: {e}");
                    std::process::exit(1);
                });
                eprintln!("shared-terminal relay listening on {addr} (end-to-end encrypted; clients reach it on this port of the server's address)");
                start_relay(rl)
            });
            eprintln!("sync server listening on {listen} (put it behind TLS)");
            let trust_proxy = args.iter().any(|a| a == "--trust-proxy");
            if trust_proxy {
                eprintln!("trusting X-Forwarded-For from a reverse proxy on this machine (only use it behind one)");
            }
            serve_with(db, l, relay, trust_proxy).await;
        }
        _ => usage(),
    }
}

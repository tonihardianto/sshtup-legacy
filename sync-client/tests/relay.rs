use std::io::{BufRead, BufReader, Write};
use sync_client::account::{login, register};
use sync_client::api::Api;
use sync_server::{create_invite, db::Db, spawn_server_relay};

fn signed_in(base: &str, db: &Db) -> Api {
    register(&Api::new(base, None), "me@x.io", "a long enough password", &create_invite(db)).unwrap();
    let s = login(&Api::new(base, None), "me@x.io", "a long enough password", "d").unwrap();
    Api::new(base, Some(s.token))
}

#[test]
fn an_account_gets_a_relay_address_and_a_ticket_that_opens_it() {
    let db = Db::memory().unwrap();
    let base = spawn_server_relay(db.clone(), true);
    let pass = signed_in(&base, &db).relay_pass().unwrap();
    assert!(pass.addr.starts_with("127.0.0.1:"), "{}", pass.addr);
    let mut s = std::net::TcpStream::connect(&pass.addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    writeln!(s, "{}", serde_json::json!({"t":"host","ticket":pass.ticket})).unwrap();
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    assert!(line.contains("hosted"), "{line}");
}

#[test]
fn without_a_relay_the_client_says_so_plainly() {
    let db = Db::memory().unwrap();
    let base = spawn_server_relay(db.clone(), false);
    let e = signed_in(&base, &db).relay_pass().err().expect("no relay");
    assert!(format!("{e:?}").contains("--relay-listen"), "{e:?}");
}

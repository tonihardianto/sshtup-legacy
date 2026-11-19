//! Fixed inputs with outputs recorded from an earlier build. If a dependency update
//! (hkdf, sha2, argon2, ...) changes any of these, data that people already have
//! stops opening, so these must keep passing.
use sync_core::{boxes, recovery, relaywire};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn account_keys_come_out_the_same() {
    let k = sync_core::account::derive_account("correct horse battery staple", &[7u8; 16]).unwrap();
    assert_eq!(k.auth.to_vec(), unhex("599b3e53483aca6a21dd4298c0593cb63694a344c91dece40c7cb9179cffdb13"));
    assert_eq!(k.enc.to_vec(), unhex("b9e78934af1fa8e9407a34dcadee0614720b3d73d9fd923932375b6f4a924e3e"));
}

#[test]
fn an_old_relay_message_still_opens() {
    let key = [9u8; relaywire::KEY_LEN];
    let sealed = "730b44e74830580284c8cf44e42fb1292dfb81cc442cd84226433b39478900263fcc2bb5d1c547c36a389c989428af72d284f2";
    assert_eq!(relaywire::open(&key, relaywire::Dir::Out, sealed).as_deref(), Some(&b"hello relay"[..]));
}

#[test]
fn an_old_sealed_box_still_opens() {
    let kp = boxes::keypair_from_secret(&[4u8; 32]);
    let blob = unhex("56d4b1f6674218c21f735c5aafa78588469af712963ca08c8ae9f5785d156e563e1112eb9638472bf265c4937afd8a791a47a85128ee0d4a1f2b375f4bcab8663b55ec9cc8d4c5269e3da99a187e3ec110");
    assert_eq!(&boxes::open_sealed(&kp, &blob).unwrap()[..], b"hello box");
}

#[test]
fn recovery_auth_and_fingerprints_do_not_change() {
    assert_eq!(recovery::recovery_auth(&[3u8; 32]).to_vec(), unhex("5d5af0225dbe64f75c9c59fc66149ed6985b0e724a111b5d9f5c89f385ce1ebc"));
    assert_eq!(boxes::fingerprint(&[5u8; 32]), "88595 93988 87683 83936 46968 89684 09924 26684");
}

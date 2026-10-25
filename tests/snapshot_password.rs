//! G6 password-protected snapshots (set / change / remove password).
use ferrotap::data_io::{list_snapshots, load_snapshot, save_snapshot};
use ferrotap::model::SessionRecord;
use ferrotap::snapcrypt::*;

#[test]
fn encrypt_round_trip_wrong_password_and_tamper() {
    let c = encrypt("secret session text", "pw1").unwrap();
    assert!(c.starts_with(MAGIC) && !c.contains("secret"));
    assert_ne!(c, encrypt("secret session text", "pw1").unwrap(), "fresh salt and nonce each time");
    assert_eq!(decrypt(&c, "pw1").unwrap(), "secret session text");
    assert!(decrypt(&c, "pw2").unwrap_err().contains("wrong password"));
    let mut bad = c.clone().into_bytes();
    let i = bad.len() - 6;
    bad[i] = if bad[i] == b'A' { b'B' } else { b'A' };
    assert!(decrypt(std::str::from_utf8(&bad).unwrap(), "pw1").is_err(), "tampering is detected");
    assert!(encrypt("x", "").is_err());
    assert!(decrypt("plain text", "pw").is_err());
}

#[test]
fn snapshot_password_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let s = vec![SessionRecord { id: 1, method: "GET".into(), url: "https://secret.example/token=abc".into(), status: 200, ..Default::default() }];
    let p = save_snapshot(dir.path(), "", "mine", &s).unwrap();
    assert!(!list_snapshots(dir.path())[0].locked);

    set_password(&p, None, Some("hunter2")).unwrap();
    let raw = std::fs::read_to_string(&p).unwrap();
    assert!(!raw.contains("secret.example"), "nothing readable on disk");
    assert!(list_snapshots(dir.path())[0].locked);
    assert!(is_encrypted(&p));
    assert!(load_snapshot(&p).is_err(), "plain load refuses");
    assert!(load(&p, None).unwrap_err().contains("password protected"));
    assert!(load(&p, Some("nope")).is_err());
    assert_eq!(load(&p, Some("hunter2")).unwrap()[0].url, s[0].url);

    assert!(set_password(&p, None, Some("x")).is_err(), "changing needs the current password");
    assert!(set_password(&p, Some("wrong"), Some("x")).is_err());
    set_password(&p, Some("hunter2"), Some("correct horse")).unwrap();
    assert_eq!(load(&p, Some("correct horse")).unwrap().len(), 1);

    set_password(&p, Some("correct horse"), None).unwrap();
    assert!(!is_encrypted(&p));
    assert_eq!(load_snapshot(&p).unwrap()[0].url, s[0].url);
}

//! The client-process lookup finds the owner of a loopback connection (this test process).
#[test]
fn finds_our_own_connection() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let c = std::net::TcpStream::connect(l.local_addr().unwrap()).unwrap();
    let (_srv, peer) = l.accept().unwrap();
    assert_eq!(peer, c.local_addr().unwrap());
    let t = std::time::Instant::now();
    let (pid, name) = ferrotap::procinfo::lookup(&peer).expect("owner found");
    assert_eq!(pid, std::process::id());
    assert!(name.to_ascii_lowercase().starts_with("procinfo"), "{name}");
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "native lookup is fast: {:?}", t.elapsed());
    // cached the second time
    let t = std::time::Instant::now();
    assert_eq!(ferrotap::procinfo::lookup(&peer).unwrap().0, pid);
    assert!(t.elapsed() < std::time::Duration::from_millis(5));
    // remote addresses are not looked up
    assert!(ferrotap::procinfo::lookup(&"10.1.2.3:5555".parse().unwrap()).is_none());
}

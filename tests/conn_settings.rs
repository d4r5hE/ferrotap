//! G8 connection settings (pure parts) and root CA validity / regenerate / custom CA.
use std::sync::{Arc, Mutex};

use ferrotap::ca_trust::{parse_cert_basics, CaManager, CertutilRunner};
use ferrotap::conn::*;

#[test]
fn proxy_addresses_and_patterns() {
    assert_eq!(parse_proxy_addr("corp:3128"), Some(ProxyAddr { host: "corp".into(), port: 3128, auth: None }));
    assert_eq!(parse_proxy_addr("http://u:p@10.0.0.1:8080/").unwrap().auth.as_deref(), Some("u:p"));
    assert_eq!(parse_proxy_addr("proxyhost").unwrap().port, 8080);
    assert_eq!(parse_proxy_addr("[::1]:9000").unwrap().host, "::1");
    assert!(parse_proxy_addr("  ").is_none());
    assert_eq!(parse_system_proxy("http=a:1;https=b:2").unwrap().host, "b");
    assert_eq!(parse_system_proxy("c:3").unwrap().port, 3);

    assert!(host_pattern_matches("*.example.com", "api.example.com"));
    assert!(host_pattern_matches("*.example.com", "example.com"));
    assert!(!host_pattern_matches("*.example.com", "badexample.com"));
    assert!(host_pattern_matches("<local>", "intranet"));
    assert!(!host_pattern_matches("<local>", "a.b"));
    assert!(host_pattern_matches("API.test", "api.test"));
}

#[test]
fn host_map_upstream_and_listen() {
    let mut s = ConnSettings::default();
    s.host_map = vec![
        HostMapEntry { enabled: false, host: "off.test".into(), target: "1.1.1.1".into() },
        HostMapEntry { enabled: true, host: "*.dev.test".into(), target: "127.0.0.1".into() },
    ];
    assert_eq!(s.map_host("a.dev.test").as_deref(), Some("127.0.0.1"));
    assert_eq!(s.map_host("off.test"), None);
    s.host_map_enabled = false;
    assert_eq!(s.map_host("a.dev.test"), None);

    assert_eq!(s.upstream_for("x.com"), None);
    s.upstream = UpstreamMode::Manual;
    s.upstream_manual = "gw:8888".into();
    s.upstream_bypass = vec!["*.internal".into(), "<local>".into()];
    assert_eq!(s.upstream_for("x.com").unwrap().host, "gw");
    assert_eq!(s.upstream_for("db.internal"), None);
    assert_eq!(s.upstream_for("printer"), None);

    assert_eq!(s.listen_addr().to_string(), format!("127.0.0.1:{}", s.port));
    s.allow_remote = true;
    assert!(s.listen_addr().ip().is_unspecified());

    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("conn.json");
    s.save(&p).unwrap();
    assert_eq!(ConnSettings::load(&p), s);
    assert_eq!(ConnSettings::load(&dir.path().join("missing.json")), ConnSettings::default());
}

#[test]
fn hosts_file_and_setup_page() {
    let h = parse_hosts_file("# comment\n127.0.0.1   localhost loopback\n::1 localhost # v6\n\nnot-an-ip host\n10.0.0.5 nas.local\n");
    let pairs: Vec<_> = h.iter().map(|e| (e.host.as_str(), e.target.as_str())).collect();
    assert_eq!(pairs, [("localhost", "127.0.0.1"), ("loopback", "127.0.0.1"), ("localhost", "::1"), ("nas.local", "10.0.0.5")]);

    assert!(is_setup_host("ferrotap.ca") && is_setup_host("FERROTAP.CA:80") && !is_setup_host("ferrotap.ca.evil"));
    let (st, ct, body) = setup_page("/", None);
    assert_eq!((st, ct), (200, "text/html; charset=utf-8"));
    assert!(String::from_utf8(body).unwrap().contains("ferrotap-ca.crt"));
    let (st, _, body) = setup_page("/ferrotap-ca.crt", Some("-----BEGIN CERTIFICATE-----"));
    assert_eq!(st, 200);
    assert!(body.starts_with(b"-----BEGIN"));
    assert_eq!(setup_page("/ca", None).0, 404);
    assert_eq!(setup_page("/nope", None).0, 404);
}

/// certutil stand-in: reports "trusted" when told to.
struct FakeCertutil(Mutex<bool>);
impl CertutilRunner for FakeCertutil {
    fn run(&self, args: &[&str]) -> Result<(i32, String), String> {
        if *self.0.lock().unwrap() && args.contains(&"-store") {
            Ok((0, args.last().unwrap().to_string()))
        } else {
            Ok((1, String::new()))
        }
    }
}

#[test]
fn ca_validity_regenerate_and_custom() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Arc::new(FakeCertutil(Mutex::new(false)));
    let ca = CaManager::with_runner(dir.path().join("ca"), fake.clone());
    ca.generate(400).unwrap();
    let info = ca.info().unwrap();
    assert!(info.subject.contains("CN=Ferrotap Local Root CA"), "{}", info.subject);
    assert!((398..=400).contains(&info.days_left), "{}", info.days_left);
    assert!(!info.expired);
    assert_eq!(ca.expiry_warning(30), None);
    assert!(ca.expiry_warning(500).unwrap().contains("expires in"));

    let before = ca.thumbprint_sha1().unwrap();
    *fake.0.lock().unwrap() = true;
    assert!(ca.regenerate(3650).is_err(), "refuses while trusted");
    *fake.0.lock().unwrap() = false;
    let backup = ca.regenerate(3650).unwrap().expect("old files backed up");
    assert!(backup.join("ferrotap-ca.pem").exists() && backup.join("ferrotap-ca.key").exists());
    assert_ne!(ca.thumbprint_sha1().unwrap(), before);
    assert!(ca.info().unwrap().days_left > 3600);

    // custom CA from another manager's files
    let other = CaManager::with_runner(dir.path().join("other"), fake.clone());
    other.generate(100).unwrap();
    assert!(ca.import_custom(&other.cert_path, &dir.path().join("nope.key")).is_err());
    ca.import_custom(&other.cert_path, &other.key_path).unwrap();
    assert_eq!(ca.thumbprint_sha1().unwrap(), other.thumbprint_sha1().unwrap());
    let b = parse_cert_basics(&ca.cert_der().unwrap()).unwrap();
    assert!(b.not_after > b.not_before);
}

#[test]
fn vpn_adapters_from_ipconfig() {
    let text = "Windows IP Configuration\n\n   Host Name . . . . . . . . . . . . : pc\n\nEthernet adapter Ethernet:\n\n   Media State . . . . . . . . . . . : Media disconnected\n   Description . . . . . . . . . . . : Realtek PCIe GbE Family Controller\n\nUnknown adapter Tailscale:\n\n   Connection-specific DNS Suffix  . : \n   Description . . . . . . . . . . . : Tailscale Tunnel\n   Autoconfiguration IPv4 Address. . : 169.254.83.107(Preferred) \n\nUnknown adapter CorpVPN:\n\n   Connection-specific DNS Suffix  . : corp.example.com\n   Description . . . . . . . . . . . : WireGuard Tunnel\n   IPv4 Address. . . . . . . . . . . : 10.8.0.2(Preferred) \n\nPPP adapter Office:\n\n   Description . . . . . . . . . . . : Office\n   IPv4 Address. . . . . . . . . . . : 172.16.5.9(Preferred) \n";
    let v = parse_ipconfig(text);
    let names: Vec<_> = v.iter().map(|a| (a.name.as_str(), a.active)).collect();
    assert_eq!(names, [("Tailscale", false), ("CorpVPN", true), ("Office", true)]);
    assert_eq!(v[1].dns_suffix, "corp.example.com");
    assert_eq!(v[1].description, "WireGuard Tunnel");
    // the real machine: it runs and returns adapters that look like VPNs (maybe none)
    for a in detect_vpns() {
        assert!(!a.name.is_empty());
    }
}

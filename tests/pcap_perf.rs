//! Packets window with a big capture (200k packets): loading is bounded and every tab renders
//! within a frame budget (debug build).
use eframe::egui;
use ferrotap::pcap::*;
use std::time::{Duration, Instant};

fn ipv4_tcp(src: [u8; 4], dst: [u8; 4], sport: u16, dport: u16, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; 12];
    f.extend([0x08, 0x00]);
    let total = 20 + 20 + payload.len();
    f.extend([0x45, 0, (total >> 8) as u8, total as u8, 0, 0, 0, 0, 64, 6, 0, 0]);
    f.extend(src);
    f.extend(dst);
    f.extend(sport.to_be_bytes());
    f.extend(dport.to_be_bytes());
    f.extend(seq.to_be_bytes());
    f.extend(0u32.to_be_bytes());
    f.extend([0x50, 0x18, 0xff, 0xff, 0, 0, 0, 0]);
    f.extend(payload);
    f
}

#[test]
fn big_capture_stays_responsive() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    let n = 200_000;
    let frames: Vec<Frame> = (0..n)
        .map(|i| {
            let c = (i % 500) as u8;
            let data = ipv4_tcp([10, 0, 1, c], [93, 184, 216, (i % 7) as u8], 40_000 + (i % 500) as u16, 443, i as u32, &[0x17; 60]);
            Frame { ts: 1_700_000_000.0 + i as f64 * 0.001, linktype: 1, orig_len: data.len() as u32, data }
        })
        .collect();
    let path = dir.path().join("big.pcap");
    std::fs::write(&path, write_pcap(&frames.iter().collect::<Vec<_>>())).unwrap();
    drop(frames);

    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let t = Instant::now();
    assert_eq!(app.ext.packets.load(&path).unwrap(), n);
    let load = t.elapsed();
    app.ext.packets.show = true;
    let ctx = egui::Context::default();
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))), ..Default::default() };
    let mut report = Vec::new();
    let mut slow = Vec::new();
    for tab in 0..6u8 {
        app.ext.packets.tab = tab;
        let mut worst = Duration::ZERO;
        for i in 0..4 {
            let t = Instant::now();
            let mut out = ctx.run_ui(input(), |ui| app.packets_window(ui.ctx()));
            out.textures_delta.clear();
            if i > 0 {
                worst = worst.max(t.elapsed());
            }
        }
        report.push(format!("tab {tab}: {worst:?}"));
        if worst >= Duration::from_millis(250) {
            slow.push(format!("tab {tab}: {worst:?}"));
        }
    }
    // a filter that matches a tenth of the packets
    app.ext.packets.tab = 0;
    app.ext.packets.filter = "ip.dst == 93.184.216.3".into();
    let t = Instant::now();
    ctx.run_ui(input(), |ui| app.packets_window(ui.ctx())).textures_delta.clear();
    report.push(format!("filter: {:?}", t.elapsed()));
    eprintln!("load {load:?}; {}", report.join(", "));
    assert!(slow.is_empty(), "slow frames: {slow:?}");
}

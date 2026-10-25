//! HTTPS Settings: certutil runs off the UI thread (a slow fake certutil here), so opening the
//! window never stalls a frame, and the trust facts appear once the check finishes.
use eframe::egui;
use ferrotap::ca_trust::{CaManager, CertutilRunner};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SlowCertutil;

impl CertutilRunner for SlowCertutil {
    fn run(&self, _args: &[&str]) -> Result<(i32, String), String> {
        std::thread::sleep(Duration::from_millis(700));
        Ok((1, "not found".into()))
    }
}

fn frame(app: &mut ferrotap::ui::GridApp, ctx: &egui::Context) -> (Duration, String) {
    let t = Instant::now();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.https_window(ui.ctx()));
    let took = t.elapsed();
    out.textures_delta.clear();
    let mut s = String::new();
    fn walk(x: &egui::Shape, s: &mut String) {
        match x {
            egui::Shape::Text(t) => {
                s.push_str(t.galley.text());
                s.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|y| walk(y, s)),
            _ => {}
        }
    }
    out.shapes.iter().for_each(|c| walk(&c.shape, &mut s));
    (took, s)
}

#[test]
fn opening_https_settings_does_not_wait_for_certutil() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let ca = CaManager::with_runner(dir.path().join("ca"), Arc::new(SlowCertutil));
    ca.ensure().unwrap();
    app.ca = ca;
    app.show_https = true;
    let ctx = egui::Context::default();
    let (t1, _) = frame(&mut app, &ctx);
    let (t2, text) = frame(&mut app, &ctx);
    assert!(t1 < Duration::from_millis(500) && t2 < Duration::from_millis(500), "frames waited for certutil: {t1:?} {t2:?}");
    assert!(text.contains("Checking the CA"), "{text}");
    let start = Instant::now();
    let text = loop {
        std::thread::sleep(Duration::from_millis(100));
        let (_, text) = frame(&mut app, &ctx);
        if text.contains("Trusted in CurrentUser store") && !text.contains("Checking the CA") {
            break text;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "facts never arrived: {text}");
    };
    assert!(text.contains("Trusted in CurrentUser store: no"), "{text}");
    assert!(text.contains("Subject:"), "{text}");
}

//! G13 Android over ADB, with a fake adb that records commands (no device is touched).
use std::sync::{Arc, Mutex};

use ferrotap::android::*;

struct Fake(Mutex<Vec<String>>);

impl AdbRunner for Fake {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let line = args.join(" ");
        self.0.lock().unwrap().push(line.clone());
        if line == "devices -l" {
            return Ok("* daemon started successfully\nList of devices attached\nR58M123ABC     device usb:1-1 product:beyond1 model:SM_G973F device:beyond1 transport_id:1\nemulator-5554  unauthorized\n\n".into());
        }
        if line.contains("am start") {
            return Err("activity not found".into());
        }
        Ok(String::new())
    }
}

#[test]
fn adb_flow() {
    let fake = Arc::new(Fake(Mutex::new(Vec::new())));
    let adb = Adb { runner: fake.clone() };
    let d = adb.devices().unwrap();
    assert_eq!(d, vec![
        Device { serial: "R58M123ABC".into(), state: "device".into(), model: "SM G973F".into() },
        Device { serial: "emulator-5554".into(), state: "unauthorized".into(), model: String::new() },
    ]);
    let log = adb.setup("R58M123ABC", 8897).unwrap();
    assert_eq!(log.len(), 2);
    let ca = adb.push_ca("R58M123ABC", std::path::Path::new("C:/ca/ferrotap-ca.pem")).unwrap();
    assert_eq!(ca.len(), 1, "settings could not be opened: only the copy is reported");
    adb.undo("R58M123ABC", 8897).unwrap();
    let cmds = fake.0.lock().unwrap().clone();
    assert_eq!(cmds, vec![
        "devices -l",
        "-s R58M123ABC reverse tcp:8897 tcp:8897",
        "-s R58M123ABC shell settings put global http_proxy 127.0.0.1:8897",
        "-s R58M123ABC push C:/ca/ferrotap-ca.pem /sdcard/Download/ferrotap-ca.crt",
        "-s R58M123ABC shell am start -a android.settings.SECURITY_SETTINGS",
        "-s R58M123ABC shell settings put global http_proxy :0",
        "-s R58M123ABC reverse --remove tcp:8897",
    ]);
    assert!(parse_devices("no header here").is_empty());
    assert!(NETWORK_SECURITY_CONFIG.contains("certificates src=\"user\""));
}

/// The window never runs adb itself: it shows a spinner while a job runs, then takes its result.
#[test]
fn window_runs_adb_off_the_ui_thread() {
    use eframe::egui;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let (jtx, jrx) = std::sync::mpsc::channel();
    app.ext.android.show = true;
    app.ext.android.adb_path = Some(Some("C:/fake/adb.exe".into()));
    app.ext.android.job = Some(jrx);
    let ctx = egui::Context::default();
    let text = |app: &mut ferrotap::ui::GridApp| {
        let mut s = String::new();
        // a new window is invisible on its first (sizing) frame
        ctx.run_ui(egui::RawInput::default(), |ui| app.android_window(ui.ctx())).textures_delta.clear();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.android_window(ui.ctx()));
        out.textures_delta.clear();
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
        s
    };
    let t = text(&mut app);
    assert!(t.contains("adb is working"), "{t}");
    assert!(app.ext.android.devices.is_none());
    jtx.send(AdbJob::Devices(Ok(vec![Device { serial: "abc".into(), state: "device".into(), model: "Pixel".into() }]))).unwrap();
    let t = text(&mut app);
    assert!(app.ext.android.job.is_none());
    assert_eq!(app.ext.android.sel.as_deref(), Some("abc"));
    let t2 = text(&mut app);
    assert!(t2.contains("abc Pixel"), "{t}\n---\n{t2}");
}

//! G13 Android over ADB: list devices, route the device's HTTP proxy to Ferrotap through
//! `adb reverse` (no Wi-Fi setup or remote access needed), push the root CA for installing,
//! undo everything, and guidance for apps that pin certificates.
use std::path::PathBuf;
use std::sync::Arc;

/// Runs adb (a fake in tests).
pub trait AdbRunner: Send + Sync {
    fn run(&self, args: &[String]) -> Result<String, String>;
}

pub struct RealAdb(pub PathBuf);

impl AdbRunner for RealAdb {
    fn run(&self, args: &[String]) -> Result<String, String> {
        let mut c = std::process::Command::new(&self.0);
        c.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        let o = c.output().map_err(|e| format!("cannot run adb: {e}"))?;
        let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&o.stderr));
        if o.status.success() { Ok(s) } else { Err(s.trim().to_string()) }
    }
}

/// adb.exe from PATH, the Android SDK, or C:\platform-tools.
pub fn find_adb() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            if d.join(exe).is_file() {
                return Some(d.join(exe));
            }
        }
    }
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        cands.push(PathBuf::from(l).join("Android").join("Sdk").join("platform-tools").join(exe));
    }
    if let Some(h) = std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT")) {
        cands.push(PathBuf::from(h).join("platform-tools").join(exe));
    }
    cands.push(PathBuf::from("C:\\platform-tools").join(exe));
    cands.into_iter().find(|p| p.is_file())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub serial: String,
    /// "device", "unauthorized", "offline", ...
    pub state: String,
    pub model: String,
}

/// Parse `adb devices -l`.
pub fn parse_devices(out: &str) -> Vec<Device> {
    out.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let serial = it.next()?.to_string();
            let state = it.next()?.to_string();
            let model = it.find_map(|kv| kv.strip_prefix("model:")).unwrap_or("").replace('_', " ");
            Some(Device { serial, state, model })
        })
        .collect()
}

pub const DEVICE_CA_PATH: &str = "/sdcard/Download/ferrotap-ca.crt";

pub struct Adb {
    pub runner: Arc<dyn AdbRunner>,
}

fn a(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

impl Adb {
    pub fn devices(&self) -> Result<Vec<Device>, String> {
        Ok(parse_devices(&self.runner.run(&a(&["devices", "-l"]))?))
    }

    fn on(&self, serial: &str, rest: &[&str]) -> Result<String, String> {
        let mut v = a(&["-s", serial]);
        v.extend(a(rest));
        self.runner.run(&v)
    }

    /// Route the device's proxy to Ferrotap through the USB connection; returns a log.
    pub fn setup(&self, serial: &str, port: u16) -> Result<Vec<String>, String> {
        let p = format!("tcp:{port}");
        let mut log = Vec::new();
        self.on(serial, &["reverse", &p, &p])?;
        log.push(format!("adb reverse {p} {p}: the device's 127.0.0.1:{port} reaches Ferrotap"));
        self.on(serial, &["shell", "settings", "put", "global", "http_proxy", &format!("127.0.0.1:{port}")])?;
        log.push(format!("device HTTP proxy set to 127.0.0.1:{port}"));
        Ok(log)
    }

    /// Copy the root CA to the device's Download folder and open the security settings to install it.
    pub fn push_ca(&self, serial: &str, ca_pem: &std::path::Path) -> Result<Vec<String>, String> {
        let local = ca_pem.display().to_string();
        self.on(serial, &["push", &local, DEVICE_CA_PATH])?;
        let mut log = vec![format!("copied the CA to {DEVICE_CA_PATH}")];
        if self.on(serial, &["shell", "am", "start", "-a", "android.settings.SECURITY_SETTINGS"]).is_ok() {
            log.push("opened Security settings: Encryption & credentials > Install a certificate > CA certificate > Download/ferrotap-ca.crt".into());
        }
        Ok(log)
    }

    /// Undo `setup`: clear the device proxy and remove the reverse port.
    pub fn undo(&self, serial: &str, port: u16) -> Result<Vec<String>, String> {
        self.on(serial, &["shell", "settings", "put", "global", "http_proxy", ":0"])?;
        let _ = self.on(serial, &["reverse", "--remove", &format!("tcp:{port}")]);
        Ok(vec!["device proxy cleared; reverse port removed".into()])
    }
}

/// For the user's own debug builds: trust user-installed CAs (Android 7+ ignores them otherwise).
pub const NETWORK_SECURITY_CONFIG: &str = r#"<!-- res/xml/network_security_config.xml (debug builds only) -->
<network-security-config>
  <debug-overrides>
    <trust-anchors>
      <certificates src="user" />
    </trust-anchors>
  </debug-overrides>
</network-security-config>
<!-- AndroidManifest.xml: <application android:networkSecurityConfig="@xml/network_security_config" ...> -->"#;

/// Result of an adb action run off the UI thread.
pub enum AdbJob {
    Devices(Result<Vec<Device>, String>),
    Log(Result<Vec<String>, String>),
}

#[derive(Default)]
pub struct AndroidUi {
    pub show: bool,
    /// Where adb is (looked up once; again on Refresh).
    pub adb_path: Option<Option<PathBuf>>,
    /// The adb action in progress (adb can take seconds, e.g. starting its server).
    pub job: Option<std::sync::mpsc::Receiver<AdbJob>>,
    pub devices: Option<Result<Vec<Device>, String>>,
    pub sel: Option<String>,
    pub log: Vec<String>,
}

impl crate::ui::GridApp {
    pub fn android_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        if !self.ext.android.show {
            return;
        }
        let st = &mut self.ext.android;
        let adb_path = st.adb_path.get_or_insert_with(find_adb).clone();
        if let Some(rx) = &st.job {
            match rx.try_recv() {
                Ok(AdbJob::Devices(d)) => {
                    if let Ok(l) = &d {
                        if st.sel.is_none() {
                            st.sel = l.iter().find(|d| d.state == "device").map(|d| d.serial.clone());
                        }
                    }
                    st.devices = Some(d);
                    st.job = None;
                }
                Ok(AdbJob::Log(Ok(l))) => {
                    st.log.extend(l);
                    st.job = None;
                }
                Ok(AdbJob::Log(Err(e))) => {
                    st.log.push(format!("error: {e}"));
                    st.job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => st.job = None,
            }
        }
        let busy = st.job.is_some();
        let mut open = true;
        let mut act: Option<&str> = None;
        egui::Window::new("Android (ADB)").open(&mut open).default_width(560.0).show(ctx, |ui| {
            match &adb_path {
                None => {
                    ui.label("adb was not found (PATH, Android SDK platform-tools or C:\\platform-tools). Install Android platform-tools to use this.");
                    return;
                }
                Some(p) => {
                    ui.label(RichText::new(format!("adb: {}", p.display())).small().color(Color32::GRAY));
                }
            }
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Refresh devices")).clicked() {
                    act = Some("refresh");
                }
                if busy {
                    ui.spinner();
                    ui.label(RichText::new("adb is working...").small());
                } else {
                    ui.label(RichText::new("Connect the phone by USB with USB debugging on.").small().color(Color32::GRAY));
                }
            });
            match &st.devices {
                None => {
                    if !busy {
                        act.get_or_insert("refresh");
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(Color32::from_rgb(230, 90, 90), e);
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label("No device connected.");
                }
                Some(Ok(list)) => {
                    for d in list {
                        let label = format!("{} {} ({})", d.serial, d.model, d.state);
                        if ui.selectable_label(st.sel.as_deref() == Some(d.serial.as_str()), label).clicked() {
                            st.sel = Some(d.serial.clone());
                        }
                    }
                }
            }
            ui.add_enabled_ui(st.sel.is_some() && !busy, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Send traffic to Ferrotap").on_hover_text("adb reverse + device HTTP proxy 127.0.0.1:<port>").clicked() {
                        act = Some("setup");
                    }
                    if ui.button("Copy the CA to the phone").clicked() {
                        act = Some("ca");
                    }
                    if ui.button("Undo").on_hover_text("Clear the device proxy").clicked() {
                        act = Some("undo");
                    }
                });
            });
            ui.label(RichText::new("Apps that ignore the system proxy, or pin certificates, will not show up. For your own debug builds, trust user CAs:").small().color(Color32::GRAY));
            ui.collapsing("network_security_config.xml", |ui| {
                ui.label(RichText::new(NETWORK_SECURITY_CONFIG).monospace().small());
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(NETWORK_SECURITY_CONFIG.to_string());
                }
            });
            for l in st.log.iter().rev().take(12) {
                ui.label(RichText::new(l).small());
            }
        });
        if act == Some("refresh") {
            // adb may have been installed since: look again
            st.adb_path = Some(find_adb());
        }
        if let (Some(a), Some(Some(p))) = (act, st.adb_path.clone()) {
            let adb = Adb { runner: Arc::new(RealAdb(p)) };
            let port = crate::conn::active_port();
            let serial = st.sel.clone().unwrap_or_default();
            let ca = crate::launch::ca_pem_path();
            let a = a.to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            st.job = Some(rx);
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let r = match a.as_str() {
                    "refresh" => AdbJob::Devices(adb.devices()),
                    "setup" => AdbJob::Log(adb.setup(&serial, port)),
                    "ca" => AdbJob::Log(adb.push_ca(&serial, &ca)),
                    _ => AdbJob::Log(adb.undo(&serial, port)),
                };
                let _ = tx.send(r);
                repaint.request_repaint();
            });
        }
        st.show = open;
    }
}

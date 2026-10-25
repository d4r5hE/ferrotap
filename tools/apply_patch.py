import re
from pathlib import Path

# 1. Fix src/ca_trust.rs unstable str_as_str and unused warnings
ca_path = Path('D:/Projects/Ferrotap/src/ca_trust.rs')
ca_text = ca_path.read_text(encoding='utf-8')
ca_text = ca_text.replace('use chrono::{DateTime, Utc};', 'use chrono::Utc;')
ca_text = ca_text.replace('BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,', 'BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,')
ca_text = ca_text.replace('match args.get(2).map(|s| s.as_str()) {', 'match args.get(2).copied() {')
ca_path.write_text(ca_text, encoding='utf-8')

# 2. Patch src/ui.rs starting clean from ui.rs.pre-https
ui_base = Path('D:/Projects/Ferrotap/src/ui.rs.pre-https')
ui_text = ui_base.read_text(encoding='utf-8')

# Add fields to GridApp
target_struct = '''    _proxy_guard: SystemProxyGuard,
    autotest: Vec<String>,
    frame_no: u32,
    log_path: Option<String>,
}'''
repl_struct = '''    _proxy_guard: SystemProxyGuard,
    autotest: Vec<String>,
    frame_no: u32,
    log_path: Option<String>,
    pub ca: crate::ca_trust::CaManager,
    pub show_https: bool,
    pub confirm_trust: bool,
    pub export_path: String,
    pub new_pattern: String,
    pub https_msg: String,
}'''
if target_struct not in ui_text:
    target_struct = target_struct.replace('\n', '\r\n')
    repl_struct = repl_struct.replace('\n', '\r\n')
assert target_struct in ui_text
ui_text = ui_text.replace(target_struct, repl_struct, 1)

# Initialize fields in GridApp::new
target_new = '''            launch_msg: String::new(),
            _proxy_guard: SystemProxyGuard::new(system_proxy::global_manager()),
        }'''
repl_new = '''            launch_msg: String::new(),
            _proxy_guard: SystemProxyGuard::new(system_proxy::global_manager()),
            ca: crate::ca_trust::CaManager::new(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ca")),
            show_https: false,
            confirm_trust: false,
            export_path: r"D:\\Projects\\Ferrotap\\data\\export\\ferrotap-ca.cer".into(),
            new_pattern: String::new(),
            https_msg: String::new(),
        }'''
if target_new not in ui_text:
    target_new = target_new.replace('\n', '\r\n')
    repl_new = repl_new.replace('\n', '\r\n')
assert target_new in ui_text
ui_text = ui_text.replace(target_new, repl_new, 1)

# Insert https_window helper method right before /// Run the native window
window_method = '''    /// HTTPS settings window: CA export, explicit-confirmation trust install, do-not-decrypt list editor.
    pub fn https_window(&mut self, ctx: &egui::Context) {
        if !self.show_https { return; }
        let mut open = true;
        let bl = crate::bypass::global();
        egui::Window::new("HTTPS Settings").open(&mut open).default_width(480.0).show(ctx, |ui| {
            ui.label(RichText::new("Root CA").strong());
            if let Ok(tp) = self.ca.thumbprint_sha1() { ui.label(format!("SHA-1: {tp}")); }
            ui.label(format!("Trusted in CurrentUser store: {}", if self.ca.is_trusted() { "yes" } else { "no" }));
            ui.horizontal(|ui| {
                ui.label("Export to:");
                ui.text_edit_singleline(&mut self.export_path);
            });
            ui.label(RichText::new("Format by extension: .pem/.crt = PEM, .p12/.pfx = PKCS#12, else DER").small().color(Color32::GRAY));
            if ui.button("Export root CA").clicked() {
                let p = self.export_path.trim().to_string();
                let l = p.to_lowercase();
                let r = if l.ends_with(".pem") || l.ends_with(".crt") { self.ca.export_pem(&p) }
                    else if l.ends_with(".p12") || l.ends_with(".pfx") { self.ca.export_pkcs12(&p, None) }
                    else { self.ca.export_der(&p) };
                self.https_msg = match r { Ok(f) => format!("Exported: {}", f.display()), Err(e) => format!("Export failed: {e}") };
            }
            ui.separator();
            if !self.confirm_trust {
                if ui.button("Trust root CA (Windows)...").clicked() { self.confirm_trust = true; }
            } else {
                ui.colored_label(Color32::from_rgb(230, 160, 40), "Install Ferrotap Local Root CA into the CurrentUser Trusted Root store? Anyone with the CA key could then impersonate sites for this user.");
                ui.horizontal(|ui| {
                    if ui.button("Yes, trust it").clicked() {
                        self.https_msg = match self.ca.install_trust(|_| true) { Ok(()) => "CA trusted".into(), Err(e) => format!("{e}") };
                        self.confirm_trust = false;
                    }
                    if ui.button("Cancel").clicked() { self.confirm_trust = false; self.https_msg = "Trust install cancelled".into(); }
                });
            }
            ui.separator();
            ui.label(RichText::new("Do not decrypt these hosts (raw TLS tunnel)").strong());
            ui.label(RichText::new("example.com, *.example.com, api*.corp.io").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.new_pattern).hint_text("host or pattern"));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add").clicked() || enter) && !self.new_pattern.trim().is_empty() {
                    self.https_msg = match bl.add(&self.new_pattern) { Ok(()) => format!("Added {}", self.new_pattern.trim()), Err(e) => e };
                    self.new_pattern.clear();
                }
            });
            let mut del: Option<String> = None;
            for p in bl.patterns() {
                ui.horizontal(|ui| {
                    ui.monospace(&p);
                    if ui.small_button("x").clicked() { del = Some(p.clone()); }
                });
            }
            if let Some(d) = del { let _ = bl.remove(&d); }
            ui.label(format!("Tunnelled so far: {}", bl.tunneled.load(std::sync::atomic::Ordering::Relaxed)));
            if !self.https_msg.is_empty() { ui.label(RichText::new(&self.https_msg).color(Color32::GRAY)); }
        });
        if !open { self.show_https = false; }
    }

    /// Run the native window'''
target_run = '    /// Run the native window'
if target_run not in ui_text:
    target_run = target_run.replace('\n', '\r\n')
    window_method = window_method.replace('\n', '\r\n')
assert target_run in ui_text
ui_text = ui_text.replace(target_run, window_method, 1)

# Toolbar button
target_tb = 'if ui.button("Open Terminal").clicked() {'
repl_tb = 'if ui.button("HTTPS Settings").clicked() { self.show_https = !self.show_https; }\n                if ui.button("Open Terminal").clicked() {'
if target_tb not in ui_text:
    target_tb = target_tb.replace('\n', '\r\n')
    repl_tb = repl_tb.replace('\n', '\r\n')
assert target_tb in ui_text
ui_text = ui_text.replace(target_tb, repl_tb, 1)

# Call https_window in ui
target_call = 'let visible = self.model.visible();'
repl_call = 'self.https_window(ui.ctx());\n        let visible = self.model.visible();'
if target_call not in ui_text:
    target_call = target_call.replace('\n', '\r\n')
    repl_call = repl_call.replace('\n', '\r\n')
assert target_call in ui_text
ui_text = ui_text.replace(target_call, repl_call, 1)

Path('D:/Projects/Ferrotap/src/ui.rs').write_text(ui_text, encoding='utf-8')
print('PATCH_SUCCESS')

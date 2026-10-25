import re,io
root=r'D:\Projects\Ferrotap\src'
def rd(p): return open(p,encoding='utf-8',newline='').read().replace('\r\n','\n')
def wr(p,s): open(p,'w',encoding='utf-8',newline='\n').write(s)

# ---- system_proxy.rs
p=root+r'\system_proxy.rs'; s=rd(p)
if 'recover_stale' not in s:
    assert s.count('*orig = Some(current);')==1
    s=s.replace('*orig = Some(current);','*orig = Some(current);\n            Self::write_backup(orig.as_ref().unwrap());')
    assert s.count('        notify_wininet();\n        Ok(true)')==1
    s=s.replace('        notify_wininet();\n        Ok(true)','        Self::clear_backup();\n        notify_wininet();\n        Ok(true)')
    s+='''

// --------------------------------------------------------------------------
// Crash / shutdown safety: persisted backup, stale recovery, console ctrl handler
// --------------------------------------------------------------------------
impl SystemProxyManager {
    /// File holding the original proxy settings while our proxy is on (survives a crash).
    pub fn backup_path() -> PathBuf {
        let base = env::var("FERROTAP_RS_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"D:\\Projects\\Ferrotap\\data"));
        base.join("proxy_backup.txt")
    }

    pub fn write_backup(st: &ProxyState) {
        let p = Self::backup_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, format!("{}\\n{}\\n{}\\n", st.enabled as u8, st.server, st.override_val));
    }

    pub fn clear_backup() {
        let _ = std::fs::remove_file(Self::backup_path());
    }

    /// Undo a stale 127.0.0.1:8897 setting left by a crashed/killed run. Returns true if it changed anything.
    #[cfg(windows)]
    pub fn recover_stale(&self) -> Result<bool, String> {
        if self.original_state.lock().unwrap().is_some() {
            return Ok(false); // we own a live toggle; nothing stale
        }
        let cur = self.get_state()?;
        let stale = cur.enabled && cur.server == DEFAULT_PROXY_SERVER;
        let p = Self::backup_path();
        let saved = std::fs::read_to_string(&p).ok();
        if let Some(txt) = saved {
            Self::clear_backup();
            if !stale {
                return Ok(false);
            }
            let mut l = txt.lines();
            let en = l.next().unwrap_or("0").trim() == "1";
            let server = l.next().unwrap_or("").to_string();
            let ov = l.next().unwrap_or("").to_string();
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let (key, _) = hkcu
                .create_subkey_with_flags(&self.key_path, KEY_SET_VALUE)
                .map_err(|e| format!("open key: {e}"))?;
            key.set_value("ProxyEnable", &(en as u32)).map_err(|e| e.to_string())?;
            if !server.is_empty() {
                key.set_value("ProxyServer", &server).map_err(|e| e.to_string())?;
            }
            if !ov.is_empty() {
                key.set_value("ProxyOverride", &ov).map_err(|e| e.to_string())?;
            }
            notify_wininet();
            return Ok(true);
        }
        if stale {
            self.disable()?;
            return Ok(true);
        }
        Ok(false)
    }

    #[cfg(not(windows))]
    pub fn recover_stale(&self) -> Result<bool, String> {
        Ok(false)
    }
}

/// Restore the system proxy on console Ctrl+C / Ctrl+Break / console close / logoff / shutdown.
/// Uses SetConsoleCtrlHandler (a plain OS thread), so no tokio runtime is needed.
#[cfg(windows)]
pub fn install_console_handler() {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(h: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    unsafe extern "system" fn handler(_ctrl_type: u32) -> i32 {
        let m = global_manager();
        let _ = m.restore();
        let _ = m.recover_stale();
        eprintln!("[Ferrotap] console signal: system proxy restored");
        std::process::exit(0);
    }
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(not(windows))]
pub fn install_console_handler() {}

/// Call once at startup and again after the UI loop returns.
pub fn shutdown_restore() {
    let m = global_manager();
    let _ = m.restore();
    let _ = m.recover_stale();
}
'''
    wr(p,s)

# ---- ui.rs
p=root+r'\ui.rs'; s=rd(p)
if 'toggle_system_proxy' not in s:
    s=s.replace('    _proxy_guard: SystemProxyGuard,\n}','    _proxy_guard: SystemProxyGuard,\n    autotest: Vec<String>,\n    frame_no: u32,\n    log_path: Option<String>,\n}',1)
    s=s.replace('        Self {\n            model: SessionModel::default(),','''        let _ = system_proxy::global_manager().recover_stale();
        let autotest: Vec<String> = std::env::var("FERROTAP_AUTOTEST")
            .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).rev().collect())
            .unwrap_or_default();
        Self {
            autotest,
            frame_no: 0,
            log_path: std::env::var("FERROTAP_SESSION_LOG").ok(),
            model: SessionModel::default(),''',1)
    methods='''    /// Toolbar "System Proxy" button handler.
    pub fn toggle_system_proxy(&mut self) {
        let mgr = self.proxy_mgr.clone();
        let res = if self.proxy_on {
            mgr.restore().and_then(|restored| if restored { Ok(()) } else { mgr.disable() })
        } else {
            mgr.enable(DEFAULT_PROXY_SERVER, DEFAULT_OVERRIDE)
        };
        match res {
            Ok(()) => {
                self.proxy_on = !self.proxy_on;
                self.launch_msg = if self.proxy_on { "System proxy enabled".into() } else { "System proxy restored".into() };
            }
            Err(e) => self.launch_msg = format!("Proxy error: {e}"),
        }
    }

    /// Toolbar "Open Clean Browser" button handler.
    pub fn open_clean_browser(&mut self) {
        self.launch_msg = match system_proxy::launch_clean_browser(DEFAULT_PROXY_SERVER, None) {
            Ok(_) => "Clean browser launched via proxy".to_string(),
            Err(e) => e,
        };
    }

    /// Toolbar "Open Terminal" button handler.
    pub fn open_terminal(&mut self) {
        self.launch_msg = match system_proxy::launch_terminal(DEFAULT_PROXY_SERVER, TerminalType::PowerShell) {
            Ok(_) => "Terminal launched with HTTP(S)_PROXY set".to_string(),
            Err(e) => e,
        };
    }

    /// Run the native window'''
    assert s.count('    /// Run the native window')==1
    s=s.replace('    /// Run the native window',methods,1)
    a=s.index('                let proxy_label =')
    b=s.index('                if !self.launch_msg.is_empty()')
    s=s[:a]+'''                let proxy_label = if self.proxy_on { "System Proxy: ON (8897)" } else { "System Proxy: OFF" };
                if ui.selectable_label(self.proxy_on, proxy_label).clicked() {
                    self.toggle_system_proxy();
                }
                if ui.button("Open Clean Browser").clicked() {
                    self.open_clean_browser();
                }
                if ui.button("Open Terminal").clicked() {
                    self.open_terminal();
                }
'''+s[b:]
    old='''        while let Ok(ev) = self.rx.try_recv() {
            if self.capturing {'''
    assert old in s
    s=s.replace(old,'''        while let Ok(ev) = self.rx.try_recv() {
            if let (SessionEvent::New(s), Some(lp)) = (&ev, &self.log_path) {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(lp) {
                    let _ = writeln!(f, "{} {} {} {}", s.id, s.method, s.url, s.status);
                }
            }
            if self.capturing {''',1)
    old='        ui.ctx().request_repaint_after(Duration::from_millis(100));\n'
    assert old in s
    s=s.replace(old,old+'''        // Test hook (env FERROTAP_AUTOTEST=proxy_on,browser,terminal,proxy_off): runs the same handlers as the buttons.
        self.frame_no += 1;
        if self.frame_no % 40 == 0 {
            if let Some(step) = self.autotest.pop() {
                match step.as_str() {
                    "proxy_on" if !self.proxy_on => self.toggle_system_proxy(),
                    "proxy_off" if self.proxy_on => self.toggle_system_proxy(),
                    "browser" => self.open_clean_browser(),
                    "terminal" => self.open_terminal(),
                    _ => {}
                }
            }
        }
''',1)
    wr(p,s)

# ---- bins
for name in ('ferrotap','ferrotap_grid'):
    p=root+'\\bin\\'+name+'.rs'; s=rd(p)
    if 'install_console_handler' in s: continue
    if name=='ferrotap':
        s=s.replace('fn main() {\n','fn main() {\n    ferrotap::system_proxy::install_console_handler();\n    let _ = ferrotap::system_proxy::global_manager().recover_stale();\n',1)
        s=s.replace('''    if let Err(e) = GridApp::new(rx, label).run("Ferrotap - Live Traffic") {
        eprintln!("ui error: {e}");
    }''','''    if let Err(e) = GridApp::new(rx, label).run("Ferrotap - Live Traffic") {
        eprintln!("ui error: {e}");
    }
    ferrotap::system_proxy::shutdown_restore();''',1)
    else:
        s=s.replace('fn main() -> eframe::Result<()> {\n','fn main() -> eframe::Result<()> {\n    ferrotap::system_proxy::install_console_handler();\n',1)
    wr(p,s)
print('patched')

//! Windows system proxy management and child process launchers for Ferrotap.
//!
//! Features:
//! - Registry-based system proxy toggling (HKCU Internet Settings, default port 8897)
//! - WinINet notification via InternetSetOptionW for immediate effect
//! - RAII guard (SystemProxyGuard) and global restore handler for clean restore on exit
//! - 'Open Clean Browser' launcher with isolated profile, certificate error bypass, and proxy flags
//! - 'Open Terminal' launcher (cmd or PowerShell) with HTTP_PROXY and HTTPS_PROXY configured

use std::collections::HashMap;
use std::env;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
#[cfg(windows)]
use winreg::RegKey;

pub const DEFAULT_PROXY_PORT: u16 = 8897;
pub const DEFAULT_PROXY_SERVER: &str = "127.0.0.1:8897";
pub const DEFAULT_OVERRIDE: &str = "<local>";
pub const INTERNET_SETTINGS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

const INTERNET_OPTION_REFRESH: u32 = 37;
const INTERNET_OPTION_SETTINGS_CHANGED: u32 = 39;

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Dynamic call to WinINet to avoid hard linker dependencies.
#[cfg(windows)]
pub fn notify_wininet() {
    use std::ffi::c_void;
    unsafe {
        let dll_name = CString::new("wininet.dll").unwrap();
        let mod_handle = winapi_load_library(&dll_name);
        if mod_handle.is_null() {
            return;
        }
        let proc_name = CString::new("InternetSetOptionW").unwrap();
        let proc_addr = winapi_get_proc_address(mod_handle, &proc_name);
        if let Some(func) = proc_addr {
            type InternetSetOptionFn = unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> i32;
            let f: InternetSetOptionFn = std::mem::transmute(func);
            f(std::ptr::null_mut(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null_mut(), 0);
            f(std::ptr::null_mut(), INTERNET_OPTION_REFRESH, std::ptr::null_mut(), 0);
        }
    }
}

#[cfg(not(windows))]
pub fn notify_wininet() {}

#[cfg(windows)]
unsafe fn winapi_load_library(name: &CString) -> *mut std::ffi::c_void {
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(lpLibFileName: *const i8) -> *mut std::ffi::c_void;
    }
    LoadLibraryA(name.as_ptr())
}

#[cfg(windows)]
unsafe fn winapi_get_proc_address(h_module: *mut std::ffi::c_void, name: &CString) -> Option<*mut std::ffi::c_void> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetProcAddress(hModule: *mut std::ffi::c_void, lpProcName: *const i8) -> *mut std::ffi::c_void;
    }
    let p = GetProcAddress(h_module, name.as_ptr());
    if p.is_null() { None } else { Some(p) }
}

/// Current registry proxy configuration snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProxyState {
    pub enabled: bool,
    pub server: String,
    pub override_val: String,
}

/// Thread-safe system proxy manager.
#[derive(Debug)]
pub struct SystemProxyManager {
    key_path: String,
    original_state: Mutex<Option<ProxyState>>,
}

impl SystemProxyManager {
    pub fn new() -> Self {
        Self {
            key_path: INTERNET_SETTINGS_KEY.to_string(),
            original_state: Mutex::new(None),
        }
    }

    pub fn with_key_path(key_path: impl Into<String>) -> Self {
        Self {
            key_path: key_path.into(),
            original_state: Mutex::new(None),
        }
    }

    /// Query current registry state.
    #[cfg(windows)]
    pub fn get_state(&self) -> Result<ProxyState, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu
            .open_subkey_with_flags(&self.key_path, KEY_READ)
            .map_err(|e| format!("Failed to open registry key: {e}"))?;

        let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
        let server: String = key.get_value("ProxyServer").unwrap_or_default();
        let override_val: String = key.get_value("ProxyOverride").unwrap_or_default();

        Ok(ProxyState {
            enabled: enabled != 0,
            server,
            override_val,
        })
    }

    #[cfg(not(windows))]
    pub fn get_state(&self) -> Result<ProxyState, String> {
        Ok(ProxyState::default())
    }

    pub fn is_enabled(&self) -> bool {
        self.get_state().map(|s| s.enabled).unwrap_or(false)
    }

    /// Enable Windows Internet Settings proxy, caching previous state for restore.
    #[cfg(windows)]
    pub fn enable(&self, server: &str, override_val: &str) -> Result<(), String> {
        let current = self.get_state()?;
        let mut orig = self.original_state.lock().unwrap();
        if orig.is_none() {
            *orig = Some(current);
            Self::write_backup(orig.as_ref().unwrap());
        }

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey_with_flags(&self.key_path, KEY_SET_VALUE)
            .map_err(|e| format!("Failed to open registry key for writing: {e}"))?;

        key.set_value("ProxyEnable", &1u32)
            .map_err(|e| format!("Failed to set ProxyEnable: {e}"))?;
        key.set_value("ProxyServer", &server)
            .map_err(|e| format!("Failed to set ProxyServer: {e}"))?;
        key.set_value("ProxyOverride", &override_val)
            .map_err(|e| format!("Failed to set ProxyOverride: {e}"))?;

        notify_wininet();
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn enable(&self, _server: &str, _override_val: &str) -> Result<(), String> {
        Ok(())
    }

    /// Disable proxy (sets ProxyEnable = 0).
    #[cfg(windows)]
    pub fn disable(&self) -> Result<(), String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey_with_flags(&self.key_path, KEY_SET_VALUE)
            .map_err(|e| format!("Failed to open registry key for writing: {e}"))?;

        key.set_value("ProxyEnable", &0u32)
            .map_err(|e| format!("Failed to clear ProxyEnable: {e}"))?;

        notify_wininet();
        Ok(())
    }

    #[cfg(not(windows))]
    pub fn disable(&self) -> Result<(), String> {
        Ok(())
    }

    /// Restore original settings saved when enable() was first called.
    #[cfg(windows)]
    pub fn restore(&self) -> Result<bool, String> {
        let mut orig = self.original_state.lock().unwrap();
        let Some(saved) = orig.take() else {
            return Ok(false);
        };

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey_with_flags(&self.key_path, KEY_SET_VALUE)
            .map_err(|e| format!("Failed to open registry key for writing: {e}"))?;

        let en_val = if saved.enabled { 1u32 } else { 0u32 };
        key.set_value("ProxyEnable", &en_val)
            .map_err(|e| format!("Failed to restore ProxyEnable: {e}"))?;
        if !saved.server.is_empty() {
            key.set_value("ProxyServer", &saved.server)
                .map_err(|e| format!("Failed to restore ProxyServer: {e}"))?;
        }
        if !saved.override_val.is_empty() {
            key.set_value("ProxyOverride", &saved.override_val)
                .map_err(|e| format!("Failed to restore ProxyOverride: {e}"))?;
        }

        Self::clear_backup();
        notify_wininet();
        Ok(true)
    }

    #[cfg(not(windows))]
    pub fn restore(&self) -> Result<bool, String> {
        let mut orig = self.original_state.lock().unwrap();
        Ok(orig.take().is_some())
    }
}

impl Default for SystemProxyManager {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard that automatically reverts proxy settings on drop.
pub struct SystemProxyGuard {
    manager: Arc<SystemProxyManager>,
    active: bool,
}

impl SystemProxyGuard {
    pub fn new(manager: Arc<SystemProxyManager>) -> Self {
        Self { manager, active: true }
    }

    pub fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for SystemProxyGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = self.manager.restore();
        }
    }
}

/// Global manager instance for application-wide proxy toggle.
static GLOBAL_MANAGER: OnceLock<Arc<SystemProxyManager>> = OnceLock::new();

pub fn global_manager() -> Arc<SystemProxyManager> {
    GLOBAL_MANAGER.get_or_init(|| Arc::new(SystemProxyManager::new())).clone()
}

/// Install Ctrl+C / SIGINT signal handler to automatically restore proxy before exit.
pub fn register_signal_handler() {
    let mgr = global_manager();
    tokio::spawn(async move {
        if let Ok(()) = tokio::signal::ctrl_c().await {
            eprintln!("\n[Ferrotap] Received termination signal; restoring system proxy...");
            let _ = mgr.restore();
            std::process::exit(0);
        }
    });
}

// --------------------------------------------------------------------------
// Launcher utilities
// --------------------------------------------------------------------------

/// Scan common locations for Chromium browsers (Edge or Chrome).
pub fn find_browser_executable() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(pf86) = env::var("ProgramFiles(x86)") {
        candidates.push(PathBuf::from(&pf86).join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(PathBuf::from(&pf86).join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Ok(pf) = env::var("ProgramFiles") {
        candidates.push(PathBuf::from(&pf).join(r"Microsoft\Edge\Application\msedge.exe"));
        candidates.push(PathBuf::from(&pf).join(r"Google\Chrome\Application\chrome.exe"));
    }
    if let Ok(local) = env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(&local).join(r"Google\Chrome\Application\chrome.exe"));
        candidates.push(PathBuf::from(&local).join(r"Microsoft\Edge\Application\msedge.exe"));
    }

    for path in candidates {
        if path.is_file() {
            return Some(path);
        }
    }

    // Fallback: search PATH for msedge or chrome
    for name in &["msedge.exe", "chrome.exe", "chromium.exe", "google-chrome"] {
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
    }

    None
}

mod which {
    use std::env;
    use std::path::PathBuf;

    pub fn which(name: &str) -> Result<PathBuf, ()> {
        if let Ok(path_var) = env::var("PATH") {
            for dir in env::split_paths(&path_var) {
                let full = dir.join(name);
                if full.is_file() {
                    return Ok(full);
                }
            }
        }
        Err(())
    }
}

/// Build clean browser command line flags with dedicated isolated profile and proxy configuration.
pub fn build_clean_browser_cmd(
    browser_exe: &Path,
    proxy: &str,
    user_data_dir: Option<&Path>,
    url: Option<&str>,
) -> (Vec<String>, PathBuf) {
    let profile_dir = match user_data_dir {
        Some(p) => p.to_path_buf(),
        None => {
            let mut temp = env::temp_dir();
            let rand_suffix: u64 = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(12345);
            temp.push(format!("ferrotap_clean_browser_{rand_suffix}"));
            temp
        }
    };

    let mut args = vec![
        browser_exe.to_string_lossy().to_string(),
        format!("--user-data-dir={}", profile_dir.display()),
        format!("--proxy-server=http://{proxy}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--ignore-certificate-errors".to_string(),
    ];

    if let Some(target) = url {
        if !target.is_empty() {
            args.push(target.to_string());
        }
    }

    (args, profile_dir)
}

/// Launch clean browser instance directed through proxy.
pub fn launch_clean_browser(
    proxy: &str,
    url: Option<&str>,
) -> Result<Child, String> {
    let exe = find_browser_executable()
        .ok_or_else(|| "No Chromium browser (Edge or Chrome) found on this PC".to_string())?;

    let (cmd_args, _dir) = build_clean_browser_cmd(&exe, proxy, None, url);
    let mut cmd = Command::new(&cmd_args[0]);
    for arg in &cmd_args[1..] {
        cmd.arg(arg);
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);

    cmd.spawn()
        .map_err(|e| format!("Failed to launch clean browser: {e}"))
}

/// Build terminal environment variables pointing HTTP/HTTPS to proxy.
pub fn build_terminal_env(proxy: &str, base: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut env_map = match base {
        Some(b) => b.clone(),
        None => env::vars().collect(),
    };

    let proxy_url = if proxy.starts_with("http://") || proxy.starts_with("https://") {
        proxy.to_string()
    } else {
        format!("http://{proxy}")
    };

    env_map.insert("HTTP_PROXY".to_string(), proxy_url.clone());
    env_map.insert("HTTPS_PROXY".to_string(), proxy_url.clone());
    env_map.insert("http_proxy".to_string(), proxy_url.clone());
    env_map.insert("https_proxy".to_string(), proxy_url);

    env_map
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalType {
    Cmd,
    PowerShell,
}

/// Build terminal command line arguments.
pub fn build_terminal_cmd(term_type: TerminalType, proxy: &str) -> Vec<String> {
    match term_type {
        TerminalType::PowerShell => {
            let msg = format!("Ferrotap proxy active at http://{proxy}");
            vec![
                "powershell.exe".to_string(),
                "-NoExit".to_string(),
                "-Command".to_string(),
                format!("Write-Host '{msg}' -ForegroundColor Cyan"),
            ]
        }
        TerminalType::Cmd => {
            let title = format!("Ferrotap Terminal (Proxy: {proxy})");
            vec![
                "cmd.exe".to_string(),
                "/k".to_string(),
                format!("title {title} && echo [Ferrotap] Proxy set to http://{proxy}"),
            ]
        }
    }
}

/// Launch terminal with proxy environment variables configured in a new window.
pub fn launch_terminal(proxy: &str, term_type: TerminalType) -> Result<Child, String> {
    let cmd_args = build_terminal_cmd(term_type, proxy);
    let envs = build_terminal_env(proxy, None);

    let mut cmd = Command::new(&cmd_args[0]);
    for arg in &cmd_args[1..] {
        cmd.arg(arg);
    }
    cmd.envs(envs);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NEW_CONSOLE);

    cmd.spawn()
        .map_err(|e| format!("Failed to launch terminal: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_browser_cmd_builder() {
        let exe = PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe");
        let profile = PathBuf::from(r"D:\Temp\Profile");
        let (args, used_profile) = build_clean_browser_cmd(
            &exe,
            "127.0.0.1:8897",
            Some(&profile),
            Some("http://127.0.0.1:8897/test"),
        );

        assert_eq!(used_profile, profile);
        assert_eq!(args[0], exe.to_string_lossy());
        assert!(args.iter().any(|a| a.contains("--proxy-server=http://127.0.0.1:8897")));
        assert!(args.iter().any(|a| a.contains("--user-data-dir=")));
        assert!(args.iter().any(|a| a == "--ignore-certificate-errors"));
        assert_eq!(args.last().unwrap(), "http://127.0.0.1:8897/test");
    }

    #[test]
    fn test_terminal_env_builder() {
        let mut base = HashMap::new();
        base.insert("USER".to_string(), "tester".to_string());

        let env = build_terminal_env("127.0.0.1:8897", Some(&base));
        assert_eq!(env.get("USER").unwrap(), "tester");
        assert_eq!(env.get("HTTP_PROXY").unwrap(), "http://127.0.0.1:8897");
        assert_eq!(env.get("HTTPS_PROXY").unwrap(), "http://127.0.0.1:8897");
        assert_eq!(env.get("http_proxy").unwrap(), "http://127.0.0.1:8897");
        assert_eq!(env.get("https_proxy").unwrap(), "http://127.0.0.1:8897");
    }

    #[test]
    fn test_terminal_cmd_builder() {
        let cmd_args = build_terminal_cmd(TerminalType::Cmd, "127.0.0.1:8897");
        assert_eq!(cmd_args[0], "cmd.exe");
        assert!(cmd_args.iter().any(|a| a.contains("8897")));

        let ps_args = build_terminal_cmd(TerminalType::PowerShell, "127.0.0.1:8897");
        assert_eq!(ps_args[0], "powershell.exe");
        assert!(ps_args.iter().any(|a| a.contains("8897")));
    }

    #[test]
    fn test_proxy_manager_mock_state() {
        let mgr = SystemProxyManager::new();
        assert_eq!(*mgr.original_state.lock().unwrap(), None);
    }
}


// --------------------------------------------------------------------------
// Crash / shutdown safety: persisted backup, stale recovery, console ctrl handler
// --------------------------------------------------------------------------
impl SystemProxyManager {
    /// File holding the original proxy settings while our proxy is on (survives a crash).
    pub fn backup_path() -> PathBuf {
        let base = env::var("FERROTAP_RS_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"D:\Projects\Ferrotap\data"));
        base.join("proxy_backup.txt")
    }

    pub fn write_backup(st: &ProxyState) {
        let p = Self::backup_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, format!("{}\n{}\n{}\n", st.enabled as u8, st.server, st.override_val));
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
        let stale = cur.enabled && (cur.server == DEFAULT_PROXY_SERVER || cur.server == format!("127.0.0.1:{}", crate::conn::current().port));
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

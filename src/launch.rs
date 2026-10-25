//! G8 / G13 one-click interception: launch an installed browser with a fresh profile, a terminal,
//! or any program with capture forced on (proxy environment + our CA for common runtimes), and
//! list the instances started from here. The Intercept window lives at the bottom.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Chromium,
    Firefox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    pub name: &'static str,
    pub exe: PathBuf,
    pub kind: BrowserKind,
}

/// (name, kind, path relative to Program Files / Program Files (x86) / LocalAppData).
const BROWSERS: &[(&str, BrowserKind, &str)] = &[
    ("Edge", BrowserKind::Chromium, r"Microsoft\Edge\Application\msedge.exe"),
    ("Chrome", BrowserKind::Chromium, r"Google\Chrome\Application\chrome.exe"),
    ("Brave", BrowserKind::Chromium, r"BraveSoftware\Brave-Browser\Application\brave.exe"),
    ("Vivaldi", BrowserKind::Chromium, r"Vivaldi\Application\vivaldi.exe"),
    ("Opera", BrowserKind::Chromium, r"Programs\Opera\opera.exe"),
    ("Chromium", BrowserKind::Chromium, r"Chromium\Application\chrome.exe"),
    ("Firefox", BrowserKind::Firefox, r"Mozilla Firefox\firefox.exe"),
];

fn roots() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].iter().filter_map(|v| std::env::var_os(v).map(PathBuf::from)).collect()
}

/// Browsers installed on this PC (first location found for each).
pub fn detect_browsers() -> Vec<Browser> {
    let roots = roots();
    BROWSERS
        .iter()
        .filter_map(|(name, kind, rel)| roots.iter().map(|r| r.join(rel)).find(|p| p.is_file()).map(|exe| Browser { name, exe, kind: *kind }))
        .collect()
}

/// Kind of a browser exe chosen by path.
pub fn kind_of(exe: &Path) -> BrowserKind {
    let n = exe.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if n.contains("firefox") { BrowserKind::Firefox } else { BrowserKind::Chromium }
}

/// Firefox profile preferences that route everything through `proxy` (host:port).
pub fn firefox_user_js(proxy: &str) -> String {
    let (host, port) = proxy.rsplit_once(':').unwrap_or((proxy, "8897"));
    let mut s = String::new();
    for (k, v) in [
        ("network.proxy.type", "1".to_string()),
        ("network.proxy.http", format!("\"{host}\"")),
        ("network.proxy.http_port", port.to_string()),
        ("network.proxy.ssl", format!("\"{host}\"")),
        ("network.proxy.ssl_port", port.to_string()),
        ("network.proxy.share_proxy_settings", "true".into()),
        ("network.proxy.no_proxies_on", "\"\"".into()),
        ("network.proxy.allow_hijacking_localhost", "true".into()),
        // Trusts the CAs in the Windows store (trust the Ferrotap CA once in HTTPS settings).
        ("security.enterprise_roots.enabled", "true".into()),
        ("browser.shell.checkDefaultBrowser", "false".into()),
        ("browser.aboutwelcome.enabled", "false".into()),
        ("datareporting.policy.dataSubmissionEnabled", "false".into()),
        ("toolkit.telemetry.reportingpolicy.firstRun", "false".into()),
    ] {
        s.push_str(&format!("user_pref(\"{k}\", {v});\n"));
    }
    s
}

fn fresh_dir(prefix: &str) -> PathBuf {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}_{ms}"))
}

/// Command line for a browser through `proxy` with its own profile (`data_dir` or a fresh temp one).
pub fn browser_cmd(exe: &Path, kind: BrowserKind, proxy: &str, data_dir: Option<&Path>, url: Option<&str>) -> Result<(Vec<String>, PathBuf), String> {
    match kind {
        BrowserKind::Chromium => Ok(crate::system_proxy::build_clean_browser_cmd(exe, proxy, data_dir, url)),
        BrowserKind::Firefox => {
            let dir = data_dir.map(Path::to_path_buf).unwrap_or_else(|| fresh_dir("ferrotap_firefox"));
            std::fs::create_dir_all(&dir).map_err(|e| format!("profile folder {}: {e}", dir.display()))?;
            std::fs::write(dir.join("user.js"), firefox_user_js(proxy)).map_err(|e| format!("user.js: {e}"))?;
            let mut v = vec![exe.display().to_string(), "-profile".into(), dir.display().to_string(), "-no-remote".into(), "-new-instance".into()];
            if let Some(u) = url.filter(|u| !u.is_empty()) {
                v.push(u.to_string());
            }
            Ok((v, dir))
        }
    }
}

/// Terminals: (id, label). Git Bash only when installed.
pub fn detect_terminals() -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("powershell", "PowerShell"), ("cmd", "Command Prompt")];
    if git_bash().is_some() {
        v.push(("bash", "Git Bash"));
    }
    v
}

fn git_bash() -> Option<PathBuf> {
    roots().into_iter().map(|r| r.join(r"Git\bin\bash.exe")).find(|p| p.is_file())
}

/// The Ferrotap root CA file (PEM) that runtimes are pointed at.
pub fn ca_pem_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ca").join("ferrotap-ca.pem")
}

/// Environment for a program captured by Ferrotap: proxy variables for every common runtime and,
/// with `trust_ca`, our CA for Node.js, Python (requests / OpenSSL), curl, git, pip, Deno, Cargo, AWS.
pub fn app_env(proxy: &str, trust_ca: bool, ca_pem: &Path) -> HashMap<String, String> {
    let mut e: HashMap<String, String> = crate::system_proxy::build_terminal_env(proxy, Some(&HashMap::new()));
    let (host, port) = proxy.rsplit_once(':').unwrap_or((proxy, "8897"));
    e.insert("ALL_PROXY".into(), format!("http://{proxy}"));
    e.insert("NO_PROXY".into(), String::new());
    e.insert("no_proxy".into(), String::new());
    e.insert("NODE_USE_ENV_PROXY".into(), "1".into());
    e.insert("GLOBAL_AGENT_HTTP_PROXY".into(), format!("http://{proxy}"));
    e.insert(
        "JAVA_TOOL_OPTIONS".into(),
        format!("-Dhttp.proxyHost={host} -Dhttp.proxyPort={port} -Dhttps.proxyHost={host} -Dhttps.proxyPort={port} -Djava.net.useSystemProxies=false"),
    );
    if trust_ca {
        let ca = ca_pem.display().to_string();
        for k in ["NODE_EXTRA_CA_CERTS", "REQUESTS_CA_BUNDLE", "SSL_CERT_FILE", "CURL_CA_BUNDLE", "GIT_SSL_CAINFO", "PIP_CERT", "DENO_CERT", "CARGO_HTTP_CAINFO", "AWS_CA_BUNDLE"] {
            e.insert(k.into(), ca.clone());
        }
    }
    e
}

/// Split a command line into arguments ("quoted parts" stay together).
pub fn split_args(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    v.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        v.push(cur);
    }
    v
}

// ------------------------------------------------------------------------------------------ instances

pub struct Instance {
    pub label: String,
    pub pid: u32,
    pub started: String,
    child: Child,
    pub exit: Option<String>,
}

impl Instance {
    /// Refresh the exit status; true while running.
    pub fn alive(&mut self) -> bool {
        if self.exit.is_some() {
            return false;
        }
        match self.child.try_wait() {
            Ok(Some(st)) => {
                self.exit = Some(st.code().map(|c| format!("exited ({c})")).unwrap_or_else(|| "exited".into()));
                false
            }
            Ok(None) => true,
            Err(e) => {
                self.exit = Some(format!("unknown: {e}"));
                false
            }
        }
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.exit = Some("stopped".into());
    }
}

fn spawn(args: &[String], envs: &HashMap<String, String>, cwd: Option<&Path>, new_console: bool) -> Result<Child, String> {
    let (exe, rest) = args.split_first().ok_or("nothing to start")?;
    let mut c = Command::new(exe);
    c.args(rest).envs(envs);
    if let Some(d) = cwd.filter(|d| !d.as_os_str().is_empty()) {
        c.current_dir(d);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        c.creation_flags(if new_console { CREATE_NEW_CONSOLE } else { CREATE_NEW_PROCESS_GROUP });
    }
    #[cfg(not(windows))]
    let _ = new_console;
    c.spawn().map_err(|e| format!("cannot start {exe}: {e}"))
}

fn instance(label: String, child: Child) -> Instance {
    Instance { label, pid: child.id(), started: chrono::Local::now().format("%H:%M:%S").to_string(), child, exit: None }
}

/// Start a browser through the proxy.
pub fn launch_browser(exe: &Path, proxy: &str, data_dir: Option<&Path>, url: Option<&str>) -> Result<Instance, String> {
    let kind = kind_of(exe);
    let (args, dir) = browser_cmd(exe, kind, proxy, data_dir, url)?;
    let mut env = HashMap::new();
    if let Some(k) = crate::tlsdecrypt::keylog_path() {
        env.insert("SSLKEYLOGFILE".to_string(), k.display().to_string());
    }
    let child = spawn(&args, &env, None, false)?;
    let name = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(instance(format!("{name} (profile {})", dir.display()), child))
}

/// Command line for a terminal id.
pub fn terminal_cmd(id: &str, proxy: &str) -> Vec<String> {
    match id {
        "cmd" => crate::system_proxy::build_terminal_cmd(crate::system_proxy::TerminalType::Cmd, proxy),
        "bash" => match git_bash() {
            Some(b) => vec![b.display().to_string(), "--login".into(), "-i".into()],
            None => crate::system_proxy::build_terminal_cmd(crate::system_proxy::TerminalType::PowerShell, proxy),
        },
        _ => crate::system_proxy::build_terminal_cmd(crate::system_proxy::TerminalType::PowerShell, proxy),
    }
}

/// Start a terminal whose programs use the proxy (and trust our CA when `trust_ca`).
pub fn launch_terminal(id: &str, proxy: &str, trust_ca: bool) -> Result<Instance, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.extend(app_env(proxy, trust_ca, &ca_pem_path()));
    let child = spawn(&terminal_cmd(id, proxy), &env, None, true)?;
    Ok(instance(format!("{id} terminal"), child))
}

/// Start any program with capture forced on.
pub fn launch_program(exe: &str, args: &str, cwd: &str, proxy: &str, trust_ca: bool) -> Result<Instance, String> {
    let exe = exe.trim().trim_matches('"');
    if exe.is_empty() {
        return Err("choose a program first".into());
    }
    let mut argv = vec![exe.to_string()];
    argv.extend(split_args(args));
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.extend(app_env(proxy, trust_ca, &ca_pem_path()));
    let console = exe.to_ascii_lowercase().ends_with(".bat") || exe.to_ascii_lowercase().ends_with(".cmd");
    let child = spawn(&argv, &env, Some(Path::new(cwd.trim())), console)?;
    let name = Path::new(exe).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| exe.to_string());
    Ok(instance(name, child))
}


// ------------------------------------------------------------------------------------------ WSL / Docker

/// wsl.exe when WSL is installed.
pub fn find_wsl() -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    let p = root.join("System32").join("wsl.exe");
    p.is_file().then_some(p)
}

/// Shell script run inside WSL: find Ferrotap (127.0.0.1 with mirrored networking, else the
/// Windows host = the default gateway), export proxy and CA variables, then an interactive bash.
pub fn wsl_script(port: u16, ca_windows_path: &str) -> String {
    let ca = ca_windows_path.replace('\'', "");
    format!(
        "H=127.0.0.1; if ! (exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null; then H=$(ip route show default | awk '{{print $3}}'); fi; \
P=http://$H:{port}; export HTTP_PROXY=$P HTTPS_PROXY=$P http_proxy=$P https_proxy=$P ALL_PROXY=$P NO_PROXY= no_proxy=; \
CA=$(wslpath '{ca}' 2>/dev/null); if [ -n \"$CA\" ]; then export SSL_CERT_FILE=$CA REQUESTS_CA_BUNDLE=$CA NODE_EXTRA_CA_CERTS=$CA CURL_CA_BUNDLE=$CA GIT_SSL_CAINFO=$CA; fi; \
echo \"[Ferrotap] proxy $P (if requests fail, turn on Connections > Allow remote devices)\"; exec bash -i"
    )
}

/// Start a WSL shell whose programs use Ferrotap.
pub fn launch_wsl(port: u16) -> Result<Instance, String> {
    let wsl = find_wsl().ok_or("WSL is not installed")?;
    let args = vec![wsl.display().to_string(), "-e".into(), "bash".into(), "-lc".into(), wsl_script(port, &ca_pem_path().display().to_string())];
    let child = spawn(&args, &std::env::vars().collect(), None, true)?;
    Ok(instance("WSL shell".into(), child))
}

/// A `docker run` command for an image whose traffic goes through Ferrotap (Docker Desktop's
/// host.docker.internal reaches the PC), with the root CA mounted and trusted by common runtimes.
pub fn docker_command(port: u16, ca_windows_path: &str, image: &str) -> String {
    let p = format!("http://host.docker.internal:{port}");
    let ca_in = "/usr/local/share/ca-certificates/ferrotap.crt";
    let image = if image.trim().is_empty() { "IMAGE" } else { image.trim() };
    format!(
        "docker run --rm -it -e HTTP_PROXY={p} -e HTTPS_PROXY={p} -e http_proxy={p} -e https_proxy={p} -e NO_PROXY= \
-v \"{ca_windows_path}:{ca_in}:ro\" -e SSL_CERT_FILE={ca_in} -e REQUESTS_CA_BUNDLE={ca_in} -e NODE_EXTRA_CA_CERTS={ca_in} -e CURL_CA_BUNDLE={ca_in} {image}"
    )
}

// ------------------------------------------------------------------------------------------ window

#[derive(Default)]
pub struct LaunchUi {
    pub show: bool,
    pub browsers: Option<Vec<Browser>>,
    pub program: String,
    pub args: String,
    pub cwd: String,
    pub url: String,
    pub trust_ca: Option<bool>,
    pub instances: Vec<Instance>,
    pub msg: String,
    pub docker_image: String,
}

impl crate::ui::GridApp {
    pub fn launch_menu(&mut self, ui: &mut eframe::egui::Ui) {
        if ui.selectable_label(self.ext.launch.show, "Intercept").on_hover_text("Start a browser, terminal or any program with capture forced on").clicked() {
            self.ext.launch.show = !self.ext.launch.show;
        }
    }

    /// Start the default browser (Connections settings), or the first one found.
    pub fn launch_default_browser(&mut self) -> Result<String, String> {
        let s = crate::conn::current();
        let exe = if s.browser.trim().is_empty() {
            detect_browsers().first().map(|b| b.exe.clone()).ok_or("No browser found on this PC")?
        } else {
            PathBuf::from(s.browser.trim())
        };
        let dir = (!s.browser_data_dir.trim().is_empty()).then(|| PathBuf::from(s.browser_data_dir.trim()));
        let inst = launch_browser(&exe, &crate::ui::our_proxy_server(), dir.as_deref(), None)?;
        let msg = format!("Started {}", inst.label);
        self.ext.launch.instances.push(inst);
        Ok(msg)
    }

    /// Start the default terminal (Connections settings).
    pub fn launch_default_terminal(&mut self) -> Result<String, String> {
        let s = crate::conn::current();
        let inst = launch_terminal(&s.terminal, &crate::ui::our_proxy_server(), true)?;
        let msg = format!("Started {} with the proxy (and the Ferrotap CA) set", inst.label);
        self.ext.launch.instances.push(inst);
        Ok(msg)
    }

    pub fn launch_windows(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        if !self.ext.launch.show {
            return;
        }
        let proxy = crate::ui::our_proxy_server();
        let mut open = true;
        let mut pick_prog = false;
        let mut set_default: Option<(Option<String>, Option<String>)> = None;
        let mut open_android = false;
        let settings = crate::conn::current();
        let st = &mut self.ext.launch;
        let browsers = st.browsers.get_or_insert_with(detect_browsers).clone();
        let mut trust = st.trust_ca.unwrap_or(true);
        egui::Window::new("Intercept").open(&mut open).default_width(560.0).vscroll(true).show(ctx, |ui| {
            ui.label(RichText::new(format!("Everything started here goes through Ferrotap ({proxy}).")).small().color(Color32::GRAY));
            ui.label(RichText::new("Browsers (fresh profile)").strong());
            ui.horizontal(|ui| {
                ui.label("Open URL");
                ui.add(egui::TextEdit::singleline(&mut st.url).hint_text("optional").desired_width(260.0));
            });
            ui.horizontal_wrapped(|ui| {
                if browsers.is_empty() {
                    ui.label("No browser found.");
                }
                for b in &browsers {
                    let def = settings.browser.trim() == b.exe.display().to_string();
                    let r = ui.button(if def { format!("{} (default)", b.name) } else { b.name.to_string() });
                    if r.clicked() {
                        let dir = (!settings.browser_data_dir.trim().is_empty()).then(|| PathBuf::from(settings.browser_data_dir.trim()));
                        let url = st.url.trim().to_string();
                        match launch_browser(&b.exe, &proxy, dir.as_deref(), (!url.is_empty()).then_some(url.as_str())) {
                            Ok(i) => {
                                st.msg = format!("Started {}", i.label);
                                st.instances.push(i);
                            }
                            Err(e) => st.msg = e,
                        }
                    }
                    r.context_menu(|ui| {
                        if ui.button("Use as the default browser").clicked() {
                            set_default = Some((Some(b.exe.display().to_string()), None));
                            ui.close();
                        }
                    });
                }
            });
            if browsers.iter().any(|b| b.kind == BrowserKind::Firefox) {
                ui.label(RichText::new("Firefox uses the CAs trusted in Windows: trust the Ferrotap CA in HTTPS settings first. Right-click a browser to make it the default.").small().color(Color32::GRAY));
            }
            ui.separator();
            ui.label(RichText::new("Terminals").strong());
            ui.horizontal_wrapped(|ui| {
                for (id, label) in detect_terminals() {
                    let def = settings.terminal == id;
                    let r = ui.button(if def { format!("{label} (default)") } else { label.to_string() });
                    if r.clicked() {
                        match launch_terminal(id, &proxy, trust) {
                            Ok(i) => {
                                st.msg = format!("Started {}", i.label);
                                st.instances.push(i);
                            }
                            Err(e) => st.msg = e,
                        }
                    }
                    r.context_menu(|ui| {
                        if ui.button("Use as the default terminal").clicked() {
                            set_default = Some((None, Some(id.to_string())));
                            ui.close();
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Android phone (ADB)...").clicked() {
                    open_android = true;
                }
                if find_wsl().is_some() && ui.button("WSL shell").on_hover_text("bash in WSL with the proxy and the Ferrotap CA set").clicked() {
                    match launch_wsl(crate::conn::active_port()) {
                        Ok(i) => {
                            st.msg = format!("Started {}", i.label);
                            st.instances.push(i);
                        }
                        Err(e) => st.msg = e,
                    }
                }
            });
            ui.collapsing("Docker container", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Image");
                    ui.add(egui::TextEdit::singleline(&mut st.docker_image).hint_text("node:22 / python:3.13 ...").desired_width(220.0));
                });
                let cmd = docker_command(crate::conn::active_port(), &ca_pem_path().display().to_string(), &st.docker_image);
                ui.label(RichText::new(&cmd).monospace().small());
                ui.horizontal(|ui| {
                    if ui.small_button("Copy command").clicked() {
                        ui.ctx().copy_text(cmd.clone());
                    }
                    ui.label(RichText::new("Containers reach the PC through its network address: turn on Connections > Allow remote devices.").small().color(Color32::GRAY));
                });
            });
            ui.separator();
            ui.label(RichText::new("A program").strong());
            ui.horizontal(|ui| {
                ui.label("Program");
                ui.add(egui::TextEdit::singleline(&mut st.program).hint_text("C:\\path\\app.exe, node, python...").desired_width(300.0));
                if ui.small_button("Browse...").clicked() {
                    pick_prog = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Arguments");
                ui.add(egui::TextEdit::singleline(&mut st.args).desired_width(300.0));
            });
            ui.horizontal(|ui| {
                ui.label("Start in");
                ui.add(egui::TextEdit::singleline(&mut st.cwd).hint_text("optional folder").desired_width(300.0));
            });
            ui.checkbox(&mut trust, "Trust the Ferrotap CA in Node.js, Python, curl, git, pip, Deno, Cargo, AWS CLI")
                .on_hover_text("Sets NODE_EXTRA_CA_CERTS, REQUESTS_CA_BUNDLE, SSL_CERT_FILE, ... to the Ferrotap CA; proxy variables are always set (HTTP(S)_PROXY, NODE_USE_ENV_PROXY, JAVA_TOOL_OPTIONS).");
            if ui.button("Start with capture").clicked() {
                match launch_program(&st.program, &st.args, &st.cwd, &proxy, trust) {
                    Ok(i) => {
                        st.msg = format!("Started {} (pid {})", i.label, i.pid);
                        st.instances.push(i);
                    }
                    Err(e) => st.msg = e,
                }
            }
            if !st.msg.is_empty() {
                ui.label(RichText::new(&st.msg).color(Color32::GRAY));
            }
            ui.separator();
            ui.label(RichText::new(format!("Started from Ferrotap ({})", st.instances.len())).strong());
            let mut stop: Option<usize> = None;
            let mut clear = false;
            egui::Grid::new("launch_instances").striped(true).show(ui, |ui| {
                for (i, inst) in st.instances.iter_mut().enumerate() {
                    let alive = inst.alive();
                    ui.label(&inst.label);
                    ui.label(format!("pid {}", inst.pid));
                    ui.label(&inst.started);
                    if alive {
                        ui.colored_label(Color32::LIGHT_GREEN, "running");
                        if ui.small_button("Stop").clicked() {
                            stop = Some(i);
                        }
                    } else {
                        ui.label(inst.exit.clone().unwrap_or_default());
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
            if !st.instances.is_empty() && ui.small_button("Remove finished").clicked() {
                clear = true;
            }
            if let Some(i) = stop {
                st.instances[i].stop();
            }
            if clear {
                st.instances.retain_mut(|i| i.alive());
            }
        });
        st.trust_ca = Some(trust);
        if open_android {
            self.ext.android.show = true;
        }
        if pick_prog {
            let start = self.ext.launch.program.clone();
            self.ext.pick.open_file("launch_prog", "Choose a program", &start, &["exe", "bat", "cmd", "com"]);
        }
        if let Some(p) = self.ext.pick.take("launch_prog") {
            self.ext.launch.program = p.display().to_string();
        }
        if let Some((b, t)) = set_default {
            let mut s = crate::conn::current();
            if let Some(b) = b {
                s.browser = b;
            }
            if let Some(t) = t {
                s.terminal = t;
            }
            crate::conn::set_current(s.clone());
            self.ext.launch.msg = match s.save(&crate::conn::ConnSettings::default_path()) {
                Ok(()) => "Default saved".into(),
                Err(e) => format!("Save failed: {e}"),
            };
        }
        self.ext.launch.show = open;
    }
}

# Ferrotap user guide

Where things are and how to try them. Run the app with `cargo run --bin ferrotap` (the window), or
`cargo run --bin ferrotap_proxy -- --help` for headless capture.

## First steps
1. **HTTPS Settings** (toolbar) > *Trust root CA* once, so HTTPS can be decrypted.
2. **Open Clean Browser** (toolbar) starts a browser with its own profile that goes through Ferrotap,
   or turn **System Proxy** on to capture every program that uses the Windows proxy.
3. Sessions appear in the grid. Click one: the inspector below shows request and response.
   Downloads over 16 MB (with or without a length) pass straight through; their session keeps the first 8 MB.

## The grid
- Right-click a session: copy (URL, cURL, PowerShell, fetch, Python), **Replay**, **Replay advanced...**
  (times, in parallel, delay, results summary), **Edit session...** (change it, replay it, or serve its
  response for that URL), **Open in Composer**, **Save as Composer request**, mark, comment, compare,
  select related.
- **Clear** menu: by status, CONNECTs, static files, marked, cached / non-cached, completed, selected...
  *Keep* limits the list to the most recent 100 / 500 / 1,000 / 10,000 sessions (marked ones stay).
- **Traffic tabs** (above the grid): *+ Tab from selection* makes a tab with just those sessions;
  right-click a tab to add / move / close; Ctrl+PageUp / PageDown switch tabs. *Inspector: below / beside*
  and *Show side panel* change the layout.
- **Filter Builder**: conditions on any field (now also query parameters, comments, HTTP versions);
  values autocomplete from what was captured.

## Inspector tabs
Headers (with *show auto-generated* and *Save...*), Params, Cookies, Forms, Raw, Text / JSON / XML /
JavaScript (decoded from gzip / brotli / zstd), Hex, **Image** (for images), **Preview** (readable HTML
view and *Open in browser*), Trailers, Timings (request sent, DNS, connect, TLS, TTFB, receive),
Sizes, Plugins (from scripts). *Pop out* moves the inspector into its own window.

## Toolbar windows
- **Composer**: build requests; tabs for Params (table / raw), Headers, **Cookies**, Body (raw /
  GraphQL), Tests, Code. Settings: follow redirects, timeout, HTTP/1.0. Collections have **folders**
  (new / rename / delete, *Move to...*).
- **Rules**: Auto Responder and more. Actions: canned response, Map Local file or **folder**, redirect,
  Map Remote, pass through + modify, block, close, **allow list**. In *More*: query / cookie edits,
  status override, user-agent presets, disable caching / cookies, **throttling** presets (3G, 4G, ...).
  Groups nest with `parent/child`. *Rule set file* imports / exports JSON and Fiddler `.farx` files.
- **Breakpoints**: pause requests / responses, edit, *Resume all* / *Abort all*.
- **Connections**: port, *Allow remote devices* (then open http://ferrotap.ca/ on the phone), what is
  recorded (only / except processes or ports), system proxy bypass hosts (VPNs are detected), browser
  profile folder, streaming mode, *Keep clients on the proxy* (removes HTTP/3 offers), TLS key log file,
  ignore certificate errors, host remapping (import the hosts file), upstream gateway.
- **Intercept**: start any installed browser, a terminal, a WSL shell, any program with the proxy and
  CA variables set, copy a `docker run` command, or set up an **Android phone over ADB**.
- **Scripts**: Rhai scripts with `on_request`, `on_response`, `column`, `inspect`; `plugins\*.rhai`
  load automatically. Diagnostics show `log()` lines and errors.
- **Packets**: open `.pcap` / `.pcapng`, display filters (`tcp.port == 443 and not dns`),
  conversations (linked to HTTP sessions), endpoints, protocols, IO graph, DNS log, *Follow TCP stream*
  and *Decrypt TLS* with a key log.
- **Assistant**: ask a local Ollama model about the selected sessions (masked with the privacy rules).
- **Windows** menu: show Composer, Rules, Packets, Scripts, Connections, Intercept or Messages in their
  own window.
- **Insights**: overview (units, timing phases), waterfall, errors, duplicates and caching, sizes,
  header notes, cookies, **Protocols** (HTTP/2 vs HTTP/1.1 per host, fallbacks, HTTP/3 offers).
- **File**: export / import HAR, **SAZ**, `.ftap` (optionally a redacted copy), **session report**
  (HTML / Markdown / PDF), snapshots (with **passwords**), compare, auto-save.
- **Messages**: WebSocket / SSE / gRPC; Protobuf settings take `.proto` files or **server reflection**.
- **Settings**: theme, toolbar buttons, privacy masking, notifications, links to the windows above.

## Automation
- REST API and MCP server (`ferrotap_mcp`): sessions, marks, replay, rules, snapshots, filters,
  auto-save, capture, system proxy, sanitized output.
- Headless: `ferrotap_proxy --out capture.har --rules rules.json --scripts edit.rhai`.

## Big captures
- Tested with 100k sessions, 8 MB bodies and 200k-packet files; windows keep computed numbers until
  the sessions change, and slow work (adb, certutil, auto-save) runs in the background.
- *Clear > Keep* caps the list; downloads over 16 MB keep only their first 8 MB in the session.
- The proxy no longer writes every session to `sessions.json`; set `FERROTAP_SESSIONS_DUMP=1` if you
  want that debugging dump.

## Not done yet
Reverse proxy, SOCKS and extra listeners, transparent capture, live packet capture (needs Npcap),
Android VPN-style capture, importing Fiddler settings / quick filters.

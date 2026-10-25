# Ferrotap — feature checklist

Ferrotap aims for feature parity with mature web-debugging proxies (Fiddler, Charles, mitmproxy,
HTTP Toolkit, Proxyman and similar tools); this checklist tracks that coverage. It describes
features and behaviour in our own words only — all of Ferrotap's code, UI and assets are its own,
and nothing is copied from any other product.

Status when written: ✅ done · ◐ partly · ❌ missing.
**Work rule for each part: pick the next group with ❌/◐ items in order (G1 first), implement them in Rust, prove
each with a test (tools\test_<group>.py or cargo test), then change their marks here to ✅ in the same part.**

## G1. Session grid actions (context menu)
- ✅ Replay a request (part 41); ✅ Replay in parallel; ✅ Advanced Replay (count, delay between, sequential/parallel)
  (context menu > Replay advanced...: times, requests in parallel, delay between starts, results summary)
- ✅ Copy URL; ✅ Copy row (tab separated); ✅ Copy summary (part 41; one session at a time - multi-select later)
- ✅ Copy as cURL; ✅ Copy as PowerShell; ✅ Copy as Fetch (JS); ✅ Copy as Python (requests) (part 41)
- ✅ Mark (red, blue, green, orange, purple, copper, strikeout) and ✅ Unmark
- ✅ Add / edit comment
- ✅ Edit in Composer (reissue)
- ✅ Add rule from selected session(s) (Rules window, prefilled from the focused session)
- ✅ Remove (delete) sessions (one, or every selected session: context menu or the Delete key)
- ✅ Clear menu: done - All, Unselected, CONNECTs, 2xx / 3xx / 4xx / 5xx, errors, static resources (part 41);
  Selected, Visible, Filtered-out, Marked, Non-marked, 1xx; Completed (closed), Cached / Non-cached
- ✅ Add to Compare group / Compare two sessions (diff view with an ignore-headers list)
- ✅ Add selected sessions to a Snapshot
- ✅ Export selected sessions (see G6)
- ✅ Select related sessions: parent request, child requests, duplicate requests; select all; range/toggle selection
- ✅ Edit a captured request / response in place (then replay it); set a session's cached response (context menu > Edit session...)

## G2. Grid columns, filters, search
- ✅ Choose columns (show/hide, width); ✅ reorder columns; ✅ sort
Rule for ✅: the feature works IN THE APP (wired into ui.rs / the proxy and usable) and is tested. Logic + tests that
nothing in the app uses yet is ◐.
- ✅ Custom column (title + source: a request/response header, cookie, or body text)
- ✅ Columns: remote TLS + HTTP version, timings (connect/TLS/TTFB captured live by proxy.rs)
- ✅ Columns: process name + PID, client/remote IP, request/response body size
- ✅ Search (multiple words, case sensitive/insensitive, headers and/or bodies)
- ✅ Quick filters (method, status, URL, host, content type, mark, comment)
- ✅ Advanced filter builder (any/all conditions on any field incl. headers, cookies, sizes, timings, PID/process)
- ✅ Saved filters (name, save, duplicate, delete, enable/disable, revert); "Reset filters"
- ✅ Traffic groups panel (group sessions by host / process / app in a side tree); pin / unpin a group; delete a
  group's sessions
- ✅ Streaming mode toggle (show bodies as they stream; SSE / gRPC always stream, Connections > Streaming mode for all)
- ✅ Autocomplete in filters (Filter Builder: value suggestions for every field, header / cookie / query names) for every field (host, URL/path, query names and values, header names and values,
  cookies, method, status, process, comment, sizes, durations, client/remote HTTP and TLS versions)
- ✅ Several traffic tabs (open, close, move, next/previous); auto-scroll to newest toggle; horizontal / vertical
  layout toggle; show / hide side panel

## G3. Inspectors
- ✅ Headers table (request + response); ✅ "show auto-generated headers"; ✅ copy key / value / both
- ✅ Params (query) view; ✅ Cookies view (name, value, domain, path, expires, max-age, HttpOnly, Secure, SameSite)
- ✅ Body text / JSON pretty; ✅ XML / HTML / JavaScript viewers; ✅ Hex; ✅ Raw
- ✅ Image viewer (PNG, JPEG, GIF, WebP, BMP, ICO); ✅ Preview (readable HTML view + Open in browser); ✅ Web forms view (urlencoded + multipart form fields/files)
- ✅ Auto-decode bodies: gzip, deflate, brotli (exact bytes kept for non-UTF-8 bodies), with "show decoded / original" toggle
- ✅ Decode tool (base64, URL, HTML entities, hex, JWT) for a selection or pasted text
- ✅ Save body to file; ✅ download headers / body; ✅ word wrap toggle; ✅ search inside a body
- ✅ Trailers view (recorded and forwarded by the proxy)
- ✅ Timings tab (request sent, DNS, connect, TLS, TTFB, receive) (DNS, TCP connect, TLS handshake, request sent, server processing/TTFB, response received)
- ✅ Sizes tab (header / body / compressed / decompressed sizes)
- ✅ Overview tab (Insights > Overview, with time / size units and which timing phases to show): statistics of the selected sessions (counts by status, content type, host, process; total bytes
  and time; timings and sizes histograms; a timeline / waterfall of the selected sessions) with copy; timing and
  size preferences (which phases/units to show)

## G4. Streaming protocols
- ✅ WebSocket: frames captured; ✅ message list in the UI (direction, type, size, time) + message viewer,
  ✅ handshake view, ✅ metadata, ✅ copy message/row, ✅ unfragment, ✅ MessagePack decode
- ✅ Server-Sent Events message list + viewer
- ✅ Socket.IO message decoding; ✅ SignalR message decoding (JSON and MessagePack hub protocols)
- ✅ gRPC (frames + protobuf tree; .proto files or server reflection decode with field / enum names, matched by /package.Service/Method): message list, decode via .proto files (Protobuf settings: load descriptors) or server reflection

## G5. Rules (Auto Responder) and breakpoints
- ✅ Rule list (ordered, enable/disable, add/edit/delete/reorder), ✅ match by URL contains/regex + method
- ✅ Match on header, cookie, body, host, process, status (response rules)
- ✅ Action: canned response (status, headers, body)
- ✅ Actions: response from file, redirect / map to another URL or host, modify request/response headers and body
  (set/remove/replace text or regex), delay (latency), close/abort connection, "stop processing more rules"
- ✅ Breakpoints: pause request/response by URL/method/status, edit, resume, abort, timeout (part 39);
  ✅ "resume all paused" (+ abort all)
- ✅ Rule groups (nested with parent/child names, duplicate group, move rule between groups); ✅ duplicate a rule; ✅ per-rule match counter
  + reset; ✅ import / export rule sets (JSON, and from files); ✅ "apply rules and enable capture" one-click
- ✅ Predefined response templates (e.g. 200 OK, 204, 301/302 redirect, 401, 403, 404, 500, 502, 503, CORS
  preflight) to pick from; ✅ choose a response file from disk

## G6. Sessions storage (Snapshots) and import/export
- ✅ Snapshots: save selected / all sessions into named snapshots in folders; open, rename, clone, delete
- ✅ Password-protected snapshots (set / change / remove password; AES-256-GCM, PBKDF2 key)
- ✅ Auto-save (interval, folder, keep N)
- ✅ Compare two snapshots
- ✅ Export sessions as HAR, SAZ and our own format; ✅ import HAR / SAZ / a folder of session files

## G7. Composer extras
- ✅ Method, URL, headers, body, send through the proxy, response view; ✅ params key-value editor
- ✅ Collections with save / load / delete; ✅ history; ✅ folders (create, rename, move, delete)
- ✅ Several Composer tabs open at once; ✅ raw mode for headers / params / cookies; ✅ cookies editor
- ✅ "Save as Composer request" from a session (+ "Open in Composer"); ✅ Composer settings (follow redirects, timeout, HTTP/1.0 or 1.1)

## G8. Capture modes and connections
- ✅ Capture on/off; ✅ system proxy on/off with restore; ✅ clean browser; ✅ terminal with proxy env
- ✅ HTTPS: trust/remove our CA, export PEM/DER/PKCS#12; ✅ do-not-decrypt host list
- ✅ Reset (regenerate) the CA; ✅ use a custom CA certificate; ✅ show CA validity / expiry warning
- ✅ Ignore server certificate errors (unsafe) toggle + certificate error notice per session
- ✅ Listening port setting; ✅ allow remote devices to connect (+ show PC IP/port, CA download page for phones: http://ferrotap.ca/)
- ❌ Reverse proxy (listen on a port, forward to host:port)
- ✅ Upstream gateway: use system proxy / manual proxy (HTTP / HTTPS) / no proxy
- ✅ Host remapping (map host -> IP/host), import from the OS hosts file, enable/disable
- ✅ Network capture by process / port ("capture all" or "capture some" processes)
- ✅ Choose default browser / terminal (and browser data folder)
- ✅ Capture one application: launch a chosen program with capture forced on (proxy env / per-process capture)
  and list the instrumented process instances
- ✅ Remote devices: connectivity check from the device, list of the PC's local networks/IPs
- ✅ Hosts that bypass the proxy (list + automatic VPN host bypass notice)

## G9. Scripting
- ✅ Scripts tab (Rhai: on_request / on_response / column / inspect hooks, live validation, insert snippets, help, diagnostics, sandbox limits): create / rename / duplicate / delete / reorder scripts, enable/disable each, an editor with
  validation, autocomplete and hover help, runtime diagnostics; scripts run on each request/response (a small
  embedded language, e.g. Rhai); a "run scripts safely" sandbox note

## G10. Settings, UI and quality of life
- ✅ Settings screen: connections, HTTPS, gateway, browsers, terminal, appearance, privacy, protobuf, scripting
- ✅ Light / dark / system theme
- ✅ Privacy / sanitization: mask chosen headers, cookies, URL parts and bodies on export (replacement text)
- ✅ Keyboard shortcuts + a shortcuts dialog
- ✅ Notification settings (which events pop a notification); ✅ customizable toolbar (show/hide buttons)
- ✅ Pop-out any panel into its own window (inspector, and Windows menu: Composer, Rules, Packets, Scripts, Connections, Intercept, Messages)
- ✅ Status bar (session counts, capture state, proxy port, rules/breakpoints active counts)
- ✅ Home screen with quick-start cards (capture, composer, rules, remote devices)

## G11. AI and LLM traffic tools (Fiddler 7.x)
- ✅ LLM traffic inspector for sessions to AI APIs: model, messages, tools, tokens (input/output/thinking),
  cost breakdown, latency (TTFT, tokens/s) - computed locally from the captured request/response
- ◐ Local MCP server (status, sessions, bodies, mark / delete / select / replay, clear, capture, start with browser / terminal, system proxy, rules add / clear, export, snapshots, HTTPS dialog, sanitization, filters, autosave done; reverse proxy tools not offered) (API key, client configuration) with tools: status; start capture with browser / terminal /
  an application; enable/disable system proxy; get sessions / count / details / request+response body; select,
  mark, delete, clear, replay sessions; apply filters; create / clear rules; save sessions; snapshots (add, list,
  details, filters, clear); start/stop reverse proxy and add/remove its ports; autosave on/off/options; open the
  trust-CA dialog. Output sanitization on/off.
- ✅ Group AI-agent sessions into "agent calls"; cache LLM responses (replay identical LLM calls from cache)
- ✅ AI assistant panel (optional; local Ollama models, sessions masked with the privacy rules)

# Part 2 - the ultimate network interception app (beyond Fiddler)

The user (2026-10-13): "make one ultimate app that has everything related to network interception, so take forever
if required". Ideas below come from the feature sets of and user feedback on Burp Suite, Wireshark, Charles Proxy,
mitmproxy, HTTP Toolkit, Proxyman, Requestly and Postman (reviews on G2, Capterra, TrustRadius, PeerSpot, Telerik
feedback, Hacker News). Features and behaviour only, in our own words; no code or assets from any of them.
Order: finish G1-G11 (Fiddler parity) first, then G12 onwards.

## Lessons from user feedback (design rules for every part)
- **Fast and light**: users complain Burp and Charles get slow and memory-hungry; Fiddler Everywhere's web-based UI
  feels sluggish. Keep the native Rust/egui UI, stream large bodies, never block the UI thread, test with 100k sessions. (Checked by tests: 100k sessions at ~35-105 ms per debug frame and ~225 MB; 8 MB bodies under 20 ms per inspector frame; downloads over 16 MB stream through; Clear > Keep caps the list.)
- **Not overwhelming**: every tool is called hard for beginners (Wireshark, Burp, Fiddler's crowded start screen).
  Clean defaults, a guided first run, advanced panels hidden until asked for, tooltips, search over all commands.
- **Do two things at once**: Fiddler Everywhere users can't use the Composer while watching live traffic, and miss
  drag-and-drop and good keyboard handling. Every panel non-modal and pop-out-able; keyboard-first; drag & drop.
- **Easy HTTPS / one-click setup**: HTTP Toolkit is loved for zero-setup interception. One click per target
  (browser, terminal, app, phone, container) with automatic certificate trust after a clear confirmation.
- **Works with VPNs**: Charles users say it breaks on VPN. Detect VPN adapters, offer per-host bypass/chaining.
- **Scripting is a must**: losing FiddlerScript is the top complaint about Fiddler Everywhere (G9 + G16).
- **Stable and honest**: Burp users report crashes, HTTP/2 bugs and false positives. Every feature ships with
  tests; scanners report confidence and evidence, never guesses.
- **Good reports and exports**: Burp users want better report formats. HTML/PDF/Markdown reports (G18).
- **Free and local**: price is the main complaint about Burp Pro, Charles and Proxyman. No accounts, no cloud.

## G12. Network conditions and traffic control (Charles, Proxyman, Requestly)
- ✅ Throttling presets (3G, 4G, slow Wi-Fi, offline) and custom bandwidth/latency/packet-loss, per host or global
- ✅ Map Local (serve a local file or folder for matching URLs); Map Remote (send matching requests elsewhere)
- ✅ Rewrite rules: add/remove/replace headers, query params, cookies, body text; change status; user-agent switcher
- ✅ Block list / allow list (return an error or drop), DNS spoofing / override, "disable caching", "disable cookies"
- ❌ Port forwarding and SOCKS4/5 proxy listener; multiple listening ports; chaining to an upstream SOCKS/HTTP proxy
- ✅ Repeat advanced (N times, concurrency, delay) with a results summary (status, time, size per run)

## G13. Universal interception (HTTP Toolkit, mitmproxy)
- ✅ One-click intercept (browsers, terminals, a chosen .exe, Node/Python/Java/.NET env + CA, WSL shell, Docker run command) for: each installed browser (fresh profile), terminal/PowerShell/cmd, a chosen .exe,
  Node.js / Python / Java / .NET apps (env vars + CA trust for that runtime), Docker containers, WSL
- ◐ Android (ADB setup: reverse port, device proxy, push CA, undo, pinning guidance done; VPN-style capture not yet): ADB setup (push our CA, set the device proxy) and a VPN-style capture for apps that ignore proxies;
  certificate-pinning help for the user's own debug apps (network-security-config guidance)
- ✅ iOS: device setup page (profile download, trust steps) via the remote-devices flow
- ❌ Transparent proxy mode and a WireGuard-style / local-redirect mode for apps that ignore proxy settings
- ✅ HTTP/3 (QUIC) support or a clear fallback to HTTP/2 (Alt-Svc h3 offers removed); HTTP/2 stream view with fallback detection (Insights > Protocols)

## G14. Packet-level analysis (Wireshark)
- ❌ Optional raw packet capture (via the installed Npcap driver, only if present) next to HTTP sessions
- ✅ Open / save pcap and pcapng files; link a packet flow to its HTTP session (Packets window)
- ✅ Decrypt TLS in a pcap with a key log file (SSLKEYLOGFILE written by our clean browser and the proxy; TLS 1.3 / 1.2, AES-GCM and ChaCha20)
- ✅ Follow TCP/TLS stream view; conversations and endpoints tables; protocol hierarchy statistics
- ✅ Display-filter language for packets (field comparisons, and/or/not) with syntax help and autocomplete
- ✅ IO graph (packets/bytes per second over time); DNS query log next to the sessions

## G15. API development tools (Postman, Insomnia)
- ✅ Environments and variables ({{baseUrl}}, {{token}}) in Composer requests and collections
- ✅ Tests / assertions on a response (status, header, JSON path, time) with pass/fail results
- ✅ Run a whole collection in order (collection runner) with a results report
- ✅ Generate code from a request: cURL, PowerShell, Python, JavaScript fetch, C#, Go, Rust
- ✅ Import / export OpenAPI (Swagger) and Postman collections (OpenAPI 3 JSON); build a collection from captured traffic
- ✅ GraphQL: show query, variables and operation name as their own view; GraphQL requests in Composer
- ✅ Local mock server: serve chosen collection responses on a port, with simple routing

## G16. Automation and integration (mitmproxy, HTTP Toolkit)
- ✅ Headless / command-line mode (--out, --rules, --scripts): capture to a file, apply rules and scripts, no window (for scripts and CI)
- ✅ Local REST API to control the app (start/stop capture, list/export sessions, add rules)
- ✅ Record a browsing session and turn it into a replayable collection or test (status, content type, JSON keys, time checks)
- ✅ Plugin folder (plugins\*.rhai: request / response hooks, grid columns, inspector tab): load user scripts/extensions that add inspectors, columns or rule actions

## G17. Reports and sharing
- ✅ Session report as HTML, Markdown and PDF (selected sessions, timings, errors, notes)
- ✅ Built-in HAR viewer (open a .har someone sent you without capturing)
- ✅ Share a redacted copy: export with the privacy/sanitization settings applied (G10)

## G18. Performance and quality insights (passive, informational)
- ✅ Waterfall per page with slow-request highlighting and a "slowest hosts" summary
- ✅ Error dashboard: 4xx/5xx counts, failed connections, TLS errors, timeouts
- ✅ Duplicate / redundant request finder; cache analysis (cacheable but not cached, cache headers explained)
- ✅ Compression check (responses sent uncompressed), large-payload finder, image size report
- ✅ Response-header hygiene notes for the user's own sites (missing security/caching headers explained)
- ✅ Cookie overview per domain (size, flags, expiry) and third-party request summary

## Not replicated (cloud-account features) - local equivalents instead
- Login / licensing / subscription / upsell: none (Ferrotap is free and local).
- Sharing snapshots, rule sets and Composer collections with other users via Telerik's cloud: replaced by
  export/import files (G5, G6, G7).
- Import from Fiddler Classic (settings, CA, quick filters, rules): ◐ AutoResponder rules (.farx) via Rules > Import; a CA via HTTPS > use a custom CA; settings / quick filters not yet.

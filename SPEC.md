# Ferrotap - specification

These notes describe the feature set Ferrotap targets — the common ground of mature web-debugging
proxies such as Fiddler, Charles and mitmproxy — so the app can cover it with its own design and code.

Everything here is written in our own words and describes features and behaviour only. All of
Ferrotap's code, UI and assets are its own; nothing is copied from any other product.

## How the original is built (for orientation only)
- Electron desktop shell → a local .NET web server (the proxy + API) → an Angular web UI inside the window.
- UI screens: one main window, a Settings screen, **pop-out windows** for any panel (`popout/:component/:id`),
  a sign-in screen, and AI-assistant pages (Fiddler has an LLM assistant that can call tools such as
  "live traffic", "filters info", "overview analytics", "rules").
- Ferrotap's design: a Rust proxy core (hudsucker) that captures sessions and streams them to a
  native desktop UI (egui), all in a single local app.

## Main window layout
- **Left sidebar**: Home, Live Traffic, Composer, Rules (Auto Responder), Snapshots, Remote Devices, Settings.
- **Live Traffic** = toolbar (Capture on/off, HTTPS on/off, Clear, filters, search) + **session grid** + an
  **Inspectors** pane split into **Request** (top) and **Response** (bottom).
- Every panel can be popped out into its own window.

## 1. Capture (Live Traffic)
- Start/stop capture. Modes: **system proxy** (set/restore the Windows proxy; warn if it was changed by someone
  else), **browser** ("Open Clean Browser" = a fresh browser profile pointed at the proxy), **terminal**
  ("Open Terminal" with HTTP(S)_PROXY set), **Network Capture** (per-process / per-port rules, "Capture all" or
  "Capture some"), **Reverse Proxy** (listen on a port, forward to a host), **remote devices** ("Allow remote
  devices to connect" + show the PC's IP/port + certificate download link).
- **HTTPS**: "Trust CA Certificate and Enable HTTPS" (ask first!), export the CA as PEM / DER / PKCS#12, show
  certificate validity/expiry, "Do not decrypt" list (hosts to tunnel without decrypting), ignore cert errors.
- Protocols shown: HTTP/1.1, HTTP/2, **WebSocket** (message list per session), **gRPC** (messages, decode via
  server reflection), **Server-Sent Events**, Socket.IO, CONNECT tunnels.
- Upstream: gateway / upstream proxy settings, DNS resolution timing.

## 2. Session grid
- One row per request: #, result (status), protocol, host, URL, method, body size, content type, process name +
  PID, time, duration, client/remote TLS + HTTP version, comments. **Choose columns** (show/hide, reorder).
- Row actions (context menu): **Mark** with a colour (red, blue, green, orange, purple, copper) or strikeout,
  unmark; **Add comment**; **Replay** (and "Replay requests in parallel", **Advanced Replay** with count/delay);
  **Edit in Composer** ("Reissue from Composer"); **Add rule from selection**; **Add to compare group** /
  **Compare sessions** (diff two sessions, with "ignore headers" list); add to Snapshot; export; remove.
- Clear: all, selected, closed only, marked / non-marked, filtered-out.
- Search box + **Filters**: quick filters (by process, PID, host, status, method, content type, size, timings)
  and **Advanced Filters** (condition builder, save/duplicate/delete named filters, enable/disable).
- Paused sessions (breakpoints) show as paused; "Resume paused session(s)".

## 3. Inspectors (request + response)
- Tabs: **Headers** (table, copy key/value, show auto-generated headers), **Params / Query**, **Cookies**
  (name, value, domain, path, expires, max-age, HttpOnly, Secure, SameSite), **Body** with viewers: Text, JSON
  (tree, expand/collapse all), XML, HTML, JavaScript, Image, **Hex**, **Raw**, **Preview** (render), plus
  **Timing** and **Size** details.
- Body tools: format/pretty-print, word wrap, search, decode popup (base64/URL/etc.), copy, **save body to file**,
  "view full response" for truncated bodies, auto-decompress (gzip/br/deflate).
- WebSocket / gRPC inspectors: message list (id, direction, type, size, time) + message viewer.

## 4. Composer (build & send requests)
- Method, URL, HTTP version; **Params**, **Headers**, **Body**, cookies — each as a key/value table **or** raw
  (switch Structured/Raw, Table/Bulk); execute; response shown in the same inspectors.
- **Collections**: tree of folders and saved requests (create, rename, move, delete); "Edit in Composer" from any
  session.

## 5. Rules (Auto Responder) and breakpoints
- Rule list (ordered, groups, enable/disable each rule/group, drag to reorder, match count per rule).
- Rule = **match** conditions (URL contains/regex/exact, method, header, body, host, process) + **actions**:
  return a canned response (status, headers, body from text or file), redirect / map remote, modify request or
  response headers/body, delay (latency), close connection, **set request breakpoint**, **set response
  breakpoint** (pause the session so the user can edit it, then Resume).
- Import/export rule sets (JSON); "Apply current rules and enable capture"; "Add rule from selection".

## 6. Snapshots (saved sessions)
- Save selected / all sessions into named snapshots (folders); open, rename, clone, delete; optional password;
  **auto-save all sessions** option; **Compare snapshots**.
- Import/export: **HAR**, **SAZ** (Fiddler Classic archive), our own format; "Export selected sessions"; import a
  whole folder of session files.

## 7. Overview / analytics
- Stats for the selected sessions: counts by status, content type, host, process; total sizes and timings;
  filter the overview; copy the data.

## 8. Settings
- Connections (port, allow remote, system proxy), HTTPS (trust/export cert, do-not-decrypt), gateway/upstream,
  default browser / terminal, theme (light/dark), keyboard shortcuts dialog, privacy (sanitize headers/cookies
  in exports), "Import Fiddler Classic settings" (rules, filters).

## Build order (one feature per project part; each with a self-test)
1. Session recorder addon + session store (done in part 7 if it passes).
2. Desktop UI: Live Traffic window — capture on/off (mitmdump on 8899), session grid (live), basic Inspectors
   (headers + body text/JSON/raw) — test by sending curl requests and seeing rows appear.
3. System proxy on/off (with restore on exit) + "Open Clean Browser" + "Open Terminal".
4. HTTPS: trust/export CA (ask the user before trusting!), do-not-decrypt list.
5. Filters + search + mark/comment/columns.
R1/R1b/R1c (Rust core) DONE: hudsucker MITM on 8897 with HTTPS, HTTP/2, WebSocket parity vs the Python proxy;
   release build ~9x faster and ~60% less RAM (PARITY.md). Rust (D:\Projects\Ferrotap) is the final product;
   cargo via %USERPROFILE%\.cargo\bin\cargo.exe, release builds only.
R2 (port finished Python features to Rust) DONE so far: session grid, inspectors, system proxy + launchers, HTTPS
   settings, filters/search/marks, Composer + collections, Rules; breakpoints in progress.
R3. EVERY FEATURE ("make sure we have every single one"): the complete checklist
   is FEATURES.md (groups
   G1-G18). After breakpoints, each part takes the next group with open (❌/◐) items in order G1, G2, ... G18,
   implements them in Rust with a test each, and marks them ✅ in that file in the same part. ✅ means usable IN THE APP (wired into ui.rs / the proxy); logic nothing uses yet is ◐, and the next part wires it in before starting a new group. A group too big for
   one part is split (e.g. G3a/G3b). The project is finished only when no ❌/◐ remains (cloud-account features
   excepted, as the file says).
(Old Python steps 6-9 - Composer, Rules, Snapshots, inspectors/settings - are covered in Rust by R2/R3 above.)

## 9. HTTP Response Body Decode Pipeline
Pipeline order for incoming HTTP response bodies (foundation for R3 MIME viewers):
1. **Transfer-Encoding Reassembly (HTTP/1.1 only)**:
   - Chunk reassembly parses hex chunk length, chunk extensions, chunk data CRLF delimiters, terminating 0-length chunk, and trailers.
   - Structured trailers (e.g., `Signature`, checksums) are preserved and captured in the decoded representation.
   - HTTP/2 and HTTP/3 skip chunk reassembly; frames arrive pre-reassembled via stream-tracking.
2. **Content-Encoding Decompression**:
   - Decompresses gzip, deflate, br (Brotli), and zstd in sequence.
3. **Growable Buffer Allocation**:
   - Buffers are dynamically grown and must not rely on `Content-Length`, which reflects compressed wire size.
4. **Size Metric Exposure**:
   - Expose both raw wire byte size and decoded byte size to the Inspector UI.
5. **Viewer Layer Handoff**:
   - Emits clean decoded bytes and normalized `Content-Type` to downstream viewers (JSON, XML, image, hex).

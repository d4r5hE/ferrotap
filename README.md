# Ferrotap

**A local HTTP/HTTPS debugging proxy.** Capture and inspect traffic, pause and edit requests with
breakpoints, rewrite traffic with rules, replay and compose requests, and read packet captures — in
one desktop app, with nothing leaving your machine.

Ferrotap is free and open source, written in Rust with an [egui](https://github.com/emilk/egui)
interface. There's no account, no cloud, and no telemetry.

> **Website / docs:** https://d4r5he.github.io/ferrotap/

## Highlights

- **Capture & inspect** — a live request grid and a rich inspector: headers, query & form params,
  cookies, raw view, pretty JSON (JSONP/NDJSON aware), XML/HTML, JavaScript, a hex view, and inline
  image preview for requests and responses.
- **Decode HTTPS** — Ferrotap generates a root CA **locally, unique to your install** (the private
  key never leaves your machine) and uses it to decrypt TLS for inspection.
- **Powerful filtering** — filter the grid with simple keys (`host:`, `status:5xx`, `size:>100000`,
  `-host:ads`) or a full mitmproxy-style expression language (`~m POST & ~c 5xx`). Packet captures
  use a Wireshark-style display filter.
- **Breakpoints** — pause a request or response and edit it before it continues, targeted with the
  same expression language (method, URL, host, status, headers, content-type, body).
- **Auto-responder & rewrite** — map URLs to local files or other hosts, serve canned responses,
  rewrite headers/bodies, and throttle bandwidth. Match by `contains`, `glob` (`*/api/*`) or
  `regex`, plus header/cookie/body/status conditions.
- **Compose & replay** — build and resend requests, organize them into collections with environment
  variables, and assert on responses. Import a `curl` command, or copy any session as cURL, HTTPie,
  wget, PowerShell, Python, fetch, C#, Go or Rust.
- **Packet captures** — open and save `.pcap`/`.pcapng`, decode protocols, follow a TCP stream, and
  browse conversations, endpoints and an I/O graph.
- **WebSocket, SSE & gRPC** — a streams view shows messages frame by frame with timing, byte counts
  and decoded close codes.
- **Import & export** — HAR 1.2, Fiddler SAZ archives, Postman collections, and Ferrotap's own
  `.ftap` format.

## Install

### From a release

Download a build from the [Releases page](https://github.com/d4r5hE/ferrotap/releases), unzip, and
run the executable.

### From source

Needs a recent [Rust toolchain](https://rustup.rs).

```sh
git clone https://github.com/d4r5hE/ferrotap
cd ferrotap
cargo run --release
```

The headless proxy can be run on its own:

```sh
cargo run --release --bin ferrotap_proxy -- --help
```

## Trusting HTTPS traffic

To read HTTPS, Ferrotap acts as a man-in-the-middle using its own root certificate authority. **That
CA is created on your machine and its private key never leaves your device** — there is no shared or
downloadable certificate to install, and you should never trust one. You trust *your own* CA:

1. In Ferrotap's HTTPS / certificate settings, **export your root CA** (or let the app install it).
2. Trust that file in your OS or browser certificate store:
   - **Windows** — *Install Certificate → Local Machine → Trusted Root Certification Authorities*.
   - **macOS** — add it to the System keychain in *Keychain Access* and set it to *Always Trust*.
   - **Firefox** — *Settings → Privacy & Security → Certificates → View Certificates → Authorities →
     Import*.
3. Point your system or browser proxy at Ferrotap's listener (shown in the app), or use its
   system-proxy toggle.

Only trust a root CA you generated yourself, and remove it when you're done if the machine isn't
solely yours.

## Filter quick reference

```text
# Session grid — keys
host:example.com   status:5xx   method:POST   size:>100000   dur:>=2000
path:/api   scheme:https   ip:1.2.3.4   type:json   header:authorization   -host:ads.com

# Session grid — expression language (also used for breakpoints)
~m GET & ~c 2xx          ~d "api\.example\." | ~t json          ~u /v1/ & ~h authorization

# Packet captures (Wireshark-style)
tcp.port == 443   ip.addr == 10.0.0.0/24   tcp.port in {80 443 8080}   http.response.code >= 400
```

More in [GUIDE.md](GUIDE.md).

## MCP server (use it from an AI assistant)

Ferrotap ships an **MCP** (Model Context Protocol) server, `ferrotap_mcp`, so an MCP-capable
assistant (Claude Desktop, Claude Code, …) can inspect and drive your captured traffic: list and
read sessions and bodies, export HAR, manage Auto Responder rules, and control capture. It speaks
JSON-RPC over stdio and bridges to Ferrotap's local REST API.

**Enable it:**

1. In Ferrotap, open the **Automation** menu → **REST API / MCP** → tick **Enable REST API**
   (binds to `127.0.0.1` only, default port **8898**, off by default). Copy the **token** shown there.
2. Add the server to your MCP client's config (point `command` at the `ferrotap_mcp` binary):

   ```json
   {
     "mcpServers": {
       "ferrotap": {
         "command": "ferrotap_mcp",
         "args": ["--api-port", "8898", "--token", "<paste the token>", "--read-only"]
       }
     }
   }
   ```

   (`--api-port`/`--token` can also come from `FERROTAP_API_PORT`/`FERROTAP_API_TOKEN`.)

**Safety.** The server is local-only and token-gated. Captured traffic contains secrets (auth
tokens, cookies), so:

- **`--read-only`** (or `FERROTAP_MCP_READONLY=1`) offers *only* inspection tools and refuses
  anything that changes traffic, rules, capture state or settings — a good default for an assistant.
- Turn on **Settings → Privacy** masking (the `set_sanitize` tool toggles it) to redact headers,
  cookies, query-string tokens and body patterns in what the assistant sees.
- Only connect assistants/clients you trust.

## Building & contributing

```sh
cargo build            # debug build
cargo test             # run the test suite
cargo run              # launch the app
```

Issues and pull requests are welcome. Ferrotap is an independent, original implementation; it isn't
affiliated with or derived from any other product.

## License

Released under the [MIT License](LICENSE).

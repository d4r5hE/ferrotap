$ErrorActionPreference = 'Stop'
$root = 'D:\Projects\Ferrotap'
$enc = New-Object System.Text.UTF8Encoding($false)
function Patch($file, $old, $new) {
    $t = [IO.File]::ReadAllText($file)
    $c = ([regex]::Matches($t, [regex]::Escape($old))).Count
    if ($c -ne 1) { throw "$file : '$old' matched $c times" }
    [IO.File]::WriteAllText($file, $t.Replace($old, $new), $enc)
    "patched $file : $old"
}
$p = "$root\src\proxy.rs"
Patch $p '    start_time: String,' "    start_time: String,`n    sent: Instant,"
Patch $p '            started,' "            started,`n            sent: Instant::now(),"
Patch $p 'let (mut parts, body) = res.into_parts();' "let (mut parts, body) = res.into_parts();`n        let ttfb_total = p.sent.elapsed().as_millis() as u64;`n        let conn_info = crate::timing::take_conn(&p.url);"
Patch $p 'duration_ms: p.started.elapsed().as_millis(), ..Default::default()' "duration_ms: p.started.elapsed().as_millis(),`n            connect_ms: conn_info.connect_ms,`n            tls_ms: conn_info.tls_ms,`n            ttfb_ms: Some(ttfb_total.saturating_sub(conn_info.connect_ms.unwrap_or(0) + conn_info.tls_ms.unwrap_or(0))),`n            tls_version: conn_info.tls_version.clone(),`n            ..Default::default()"
Patch $p '.with_rustls_connector(aws_lc_rs::default_provider())' '.with_http_connector(crate::timing::build_connector())'
Patch "$root\src\lib.rs" 'pub mod proxy;' "pub mod proxy;`npub mod timing;"
$c = "$root\Cargo.toml"
Patch $c '[dependencies]' "[dependencies]`nhyper-rustls = { version = `"0.27`", default-features = false, features = [`"http1`", `"http2`", `"aws-lc-rs`", `"tls12`"] }`nwebpki-roots = `"1`"`ntower-service = `"0.3`""
Patch $c '[dev-dependencies]' "[dev-dependencies]`ntokio-rustls = `"0.26`""

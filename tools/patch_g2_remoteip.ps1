Set-Location D:\Projects\Ferrotap
$p = (Resolve-Path src\g2.rs).Path
$lines = [System.Collections.Generic.List[string]]([IO.File]::ReadAllLines($p))

# 1. add RemoteIp to ColumnSource (first ClientIp inside that enum)
$start = -1
for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match 'pub enum ColumnSource') { $start = $i; break } }
if ($start -ge 0) {
    $end = $start
    for ($i = $start; $i -lt $lines.Count; $i++) { if ($lines[$i] -match '^\}') { $end = $i; break } }
    $has = $false
    for ($i = $start; $i -le $end; $i++) { if ($lines[$i] -match '^\s*RemoteIp,') { $has = $true } }
    if (-not $has) {
        for ($i = $start; $i -le $end; $i++) {
            if ($lines[$i] -match '^\s*ClientIp,') { $lines.Insert($i + 1, '    RemoteIp,'); break }
        }
    }
}

# 2. add FilterField::RemoteIp match arm where ClientIp is evaluated
$txt = ($lines -join "`n") + "`n"
$arm = 'FilterField::ClientIp => session.client_ip.clone().unwrap_or_default(),'
if (-not $txt.Contains('FilterField::RemoteIp =>')) {
    $txt = $txt.Replace($arm, $arm + "`n            FilterField::RemoteIp => session.remote_ip.clone().unwrap_or_default(),")
}
[IO.File]::WriteAllText($p, $txt)

cargo check --all-targets --message-format=short 2>&1 | Select-String 'error' | % { $_.Line }
'--- TEST ---'
cargo test 2>&1 | Select-String 'test result|FAILED|panicked|error|left:|right:' | % { $_.Line.Substring(0, [Math]::Min(220, $_.Line.Length)) }

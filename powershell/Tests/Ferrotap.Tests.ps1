# Dependency-free unit tests (no Pester needed). Run:  powershell -NoProfile -File Ferrotap.Tests.ps1
$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\Ferrotap\Ferrotap.psd1') -Force

$script:pass = 0; $script:fail = 0
function Check([string]$Name, [scriptblock]$Test) {
    try { $r = & $Test; if ($r -eq $true) { $script:pass++ } else { $script:fail++; Write-Host "FAIL: $Name (got '$r')" -ForegroundColor Red } }
    catch { $script:fail++; Write-Host "FAIL: $Name threw: $($_.Exception.Message)" -ForegroundColor Red }
}
function Throws([scriptblock]$Test, [string]$Like) {
    try { & $Test | Out-Null; return $false } catch { return ($_.Exception.Message -like $Like) }
}
$f = { param($flt, $m, $u, $s) if ($null -ne $s) { Test-FerrotapFilter $flt -Method $m -Url $u -Status $s -Quiet } else { Test-FerrotapFilter $flt -Method $m -Url $u -Quiet } }

# ---- filter DSL ----
Check 'all'            { (& $f '~all' 'GET' 'http://x/') -eq $true }
Check 'method ci'      { (& $f '~m get' 'GET' 'http://x/') -eq $true }
Check 'method no'      { (& $f '~m POST' 'GET' 'http://x/') -eq $false }
Check 'url regex'      { (& $f '~u /api/v\d' 'GET' 'http://x/API/v2/y') -eq $true }
Check 'host'           { (& $f '~d ^x\.io$' 'GET' 'https://u:p@x.io:8443/p?q=1') -eq $true }
Check 'host ipv6'      { (& $f '~d ^::1$' 'GET' 'http://[::1]:80/') -eq $true }
Check 'and'            { (& $f '~m GET & ~u api' 'GET' 'http://x/api') -eq $true }
Check 'and false'      { (& $f '~m GET & ~u api' 'POST' 'http://x/api') -eq $false }
Check 'or'             { (& $f '~m POST | ~u api' 'GET' 'http://x/api') -eq $true }
Check 'not'            { (& $f '!~m POST' 'GET' 'http://x/') -eq $true }
Check 'precedence'     { (& $f '~m GET | ~m POST & ~u nomatch' 'GET' 'http://x/') -eq $true }
Check 'parens'         { (& $f '(~m GET | ~m POST) & ~u nomatch' 'GET' 'http://x/') -eq $false }
Check 'quoted arg'     { (& $f '~u "a b"' 'GET' 'http://x/a b') -eq $true }
Check 'status exact'   { (& $f '~c 404' 'GET' 'http://x/' 404) -eq $true }
Check 'status 5xx'     { (& $f '~c 5xx' 'GET' 'http://x/' 503) -eq $true }
Check 'status range'   { (& $f '~c 400-499' 'GET' 'http://x/' 451) -eq $true }
Check 'status list'    { (& $f '~c 401,403,5xx' 'GET' 'http://x/' 403) -eq $true }
Check 'status no'      { (& $f '~c 5xx' 'GET' 'http://x/' 200) -eq $false }
Check 'status at request stage never' { (& $f '~c 200' 'GET' 'http://x/' $null) -eq $false }
Check 'stage q/s'      { ((& $f '~q' 'GET' 'http://x/') -eq $true) -and ((& $f '~s' 'GET' 'http://x/' 200) -eq $true) -and ((& $f '~s' 'GET' 'http://x/') -eq $false) }
Check 'atoms evidence' { $r = Test-FerrotapFilter '~m GET & ~u api' -Url http://x/api; ($r.Atoms -join '|') -eq '~m GET|~u api' }
Check 'syntax: empty'  { Throws { ConvertFrom-FerrotapFilter '' } '*empty filter*' }
Check 'syntax: paren'  { Throws { ConvertFrom-FerrotapFilter '(~all' } "*missing ')'*" }
Check 'syntax: unknown'{ Throws { ConvertFrom-FerrotapFilter '~z' } '*unknown operator*' }
Check 'syntax: no arg' { Throws { ConvertFrom-FerrotapFilter '~u' } '*needs an argument*' }
Check 'syntax: quote'  { Throws { ConvertFrom-FerrotapFilter '~u "abc' } '*unterminated*' }
Check 'syntax: regex'  { Throws { ConvertFrom-FerrotapFilter '~u [' } '*invalid regex*' }
Check 'syntax: status' { Throws { ConvertFrom-FerrotapFilter '~c 9x' } '*bad status*' }
Check 'syntax: trailing' { Throws { ConvertFrom-FerrotapFilter '~all ~all' } '*unexpected*' }

# ---- rules ----
Check 'New rule defaults' { $r = New-FerrotapRule -Pattern '/api/'; $r.enabled -and $r.match_kind -eq 'contains' -and $r.status -eq 200 -and $r.action -eq 'canned' }
Check 'New rule headers'  { $r = New-FerrotapRule -Pattern 'x' -Header @{ 'X-A' = '1' }; $r.headers.Count -eq 1 -and $r.headers[0].key -eq 'X-A' }
Check 'New rule bad regex'{ Throws { New-FerrotapRule -Pattern '[' -MatchKind regex } '*invalid regex*' }
Check 'New rule bad dsl'  { Throws { New-FerrotapRule -Pattern '~m' -MatchKind dsl } '*needs an argument*' }
Check 'New rule body+file'{ Throws { New-FerrotapRule -Pattern 'x' -Body 'a' -File 'b' } '*not both*' }
Check 'contains pipeline' { (New-FerrotapRule -Pattern '/api/' | Test-FerrotapRule -Url 'http://x/api/u' -Quiet) -eq $true }
Check 'contains case-sensitive' { (New-FerrotapRule -Pattern '/API/' | Test-FerrotapRule -Url 'http://x/api/u' -Quiet) -eq $false }
Check 'regex rule'        { (New-FerrotapRule -Pattern 'users/\d+$' -MatchKind regex | Test-FerrotapRule -Url 'http://x/users/42' -Quiet) -eq $true }
Check 'dsl rule'          { (New-FerrotapRule -Pattern '~m POST & ~u login' -MatchKind dsl | Test-FerrotapRule -Method POST -Url 'http://x/login' -Quiet) -eq $true }
Check 'method filter'     { $x = New-FerrotapRule -Pattern 'x' -Method post | Test-FerrotapRule -Url 'http://x/' -Method GET; (-not $x.Matched) -and $x.Reason -like 'method mismatch*' }
Check 'disabled rule'     { $x = New-FerrotapRule -Pattern 'x' -Disabled | Test-FerrotapRule -Url 'http://x/'; (-not $x.Matched) -and $x.Reason -eq 'rule is disabled' }
Check 'result struct'     { $x = New-FerrotapRule -Name n1 -Pattern 'x' | Test-FerrotapRule -Url 'http://x/'; $x.Rule -eq 'n1' -and $x.Matched -and $x.Reason -eq 'URL contains pattern' }

# ---- response ----
Check 'canned response'   { $r = New-FerrotapRule -Pattern 'x' -Status 201 -Body 'hi' | Get-FerrotapRuleResponse; $r.Status -eq 201 -and $r.Body -eq 'hi' -and $r.Headers[0].Value -like 'text/plain*' }
Check 'file response'     {
    $tmp = Join-Path $env:TEMP 'ferrotap_test.json'; Set-Content $tmp '{"a":1}' -Encoding UTF8
    $r = New-FerrotapRule -Pattern 'x' -File $tmp | Get-FerrotapRuleResponse; Remove-Item $tmp
    $r.Status -eq 200 -and $r.Headers[0].Value -eq 'application/json' -and $r.Body -like '*"a":1*' }
Check 'missing file 404'  { (New-FerrotapRule -Pattern 'x' -File 'D:\nope\none.txt' | Get-FerrotapRuleResponse).Status -eq 404 }

# ---- save / load round trip ----
$dir = Join-Path $env:TEMP ('ferrotap_ps_' + [guid]::NewGuid().ToString('N'))
Check 'save+load'  {
    $one = New-FerrotapRule -Name a -Pattern '/a' -Header @{ 'X-T' = 'v' } -Body 'A'
    $one | Save-FerrotapRule -Path $dir | Out-Null
    $l = @(Get-FerrotapRule -Path $dir)
    $l.Count -eq 1 -and $l[0].name -eq 'a' -and $l[0].headers[0].key -eq 'X-T' -and $l[0].body -eq 'A' }
Check 'json is array w/o BOM' {
    $raw = [IO.File]::ReadAllBytes((Join-Path $dir 'rules.json')); $txt = [Text.Encoding]::UTF8.GetString($raw)
    $raw[0] -ne 0xEF -and $txt.TrimStart().StartsWith('[') -and $txt -like '*"match_kind"*' }
Check 'append'     { New-FerrotapRule -Name b -Pattern '/b' | Save-FerrotapRule -Path $dir -Append | Out-Null; @(Get-FerrotapRule -Path $dir).Count -eq 2 }
Check 'name filter'{ @(Get-FerrotapRule -Path $dir -Name 'b').Count -eq 1 }

# ---- tap ----
Check 'tap first match wins' {
    $rules = @((New-FerrotapRule -Name first -Pattern '/api' -Body '1'), (New-FerrotapRule -Name second -Pattern '/api' -Body '2'))
    $o = 'http://x/api' | Invoke-FerrotapTap -Rule $rules
    $o.Matched -and $o.Rule -eq 'first' -and $o.Response.Body -eq '1' }
Check 'tap skips disabled' {
    $rules = @((New-FerrotapRule -Name off -Pattern '/api' -Disabled), (New-FerrotapRule -Name on -Pattern '/api'))
    ('http://x/api' | Invoke-FerrotapTap -Rule $rules).Rule -eq 'on' }
Check 'tap objects + MatchedOnly' {
    $rules = @(New-FerrotapRule -Name p -Pattern '/hit' -Method POST)
    $sess = @([pscustomobject]@{ method = 'POST'; url = 'http://x/hit' }, [pscustomobject]@{ method = 'GET'; url = 'http://x/hit' }, [pscustomobject]@{ method = 'POST'; url = 'http://x/miss' })
    $all = @($sess | Invoke-FerrotapTap -Rule $rules); $m = @($sess | Invoke-FerrotapTap -Rule $rules -MatchedOnly)
    $all.Count -eq 3 -and $m.Count -eq 1 -and $m[0].Url -eq 'http://x/hit' }
Check 'tap rules from file' { ('http://x/a' | Invoke-FerrotapTap -RulesPath $dir).Rule -eq 'a' }
Check 'tap no match' { $o = 'http://x/zzz' | Invoke-FerrotapTap -RulesPath $dir; (-not $o.Matched) -and $null -eq $o.Response }

Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "Passed: $script:pass  Failed: $script:fail"
if ($script:fail -gt 0) { exit 1 } else { exit 0 }

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot '..\Ferrotap\Ferrotap.psd1') -Force
Import-Module (Join-Path $PSScriptRoot '..\Ferrotap\Ferrotap.Debug.psm1') -Force
$script:p = 0; $script:f = 0
function Check($n, [scriptblock]$t) { try { if ((& $t) -eq $true) { $script:p++ } else { $script:f++; Write-Host "FAIL: $n" -ForegroundColor Red } } catch { $script:f++; Write-Host "FAIL: $n threw: $($_.Exception.Message)" -ForegroundColor Red } }
$a = New-FerrotapRule -Name 'login' -Pattern '~m POST & ~u /login' -MatchKind dsl -Status 401
$b = New-FerrotapRule -Name 'any api' -Pattern '/api/'
$c = New-FerrotapRule -Name 'users' -Pattern '/api/users' -Status 201
Check 'syntax ok' { @(Test-FerrotapRuleSyntax -Rule $a).Count -eq 0 }
$bad = [pscustomobject]@{ name = 'bad'; match_kind = 'regex'; pattern = '(['; status = 200 }
$bad2 = [pscustomobject]@{ name = 'bad2'; match_kind = 'dsl'; pattern = '~m'; status = 700 }
Check 'bad regex' { @(Test-FerrotapRuleSyntax -Rule $bad).Count -ge 1 }
Check 'bad dsl+status' { @(Test-FerrotapRuleSyntax -Rule $bad2).Count -ge 2 }
Check 'trace winner' { $t = Trace-FerrotapRequest -Url 'http://x/api/users' -Rule @($a, $b, $c) -Quiet; ($t.Winner -eq 2) -and ($t.Rows[2].Verdict -eq 'shadowed') }
Check 'trace none' { $null -eq (Trace-FerrotapRequest -Url 'http://x/zzz' -Rule @($a, $b) -Quiet).Winner }
Check 'debug match' { (Debug-FerrotapRule -Rule $a -Method POST -Url 'http://x/login').Matched }
Check 'debug nomatch' { -not (Debug-FerrotapRule -Rule $a -Method GET -Url 'http://x/login').Matched }
Show-FerrotapRule -Rule @($a, $bad)
Write-Host "Passed: $script:p  Failed: $script:f"
if ($script:f) { exit 1 }

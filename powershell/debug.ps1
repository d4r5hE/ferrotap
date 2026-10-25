Import-Module (Join-Path $PSScriptRoot 'Ferrotap\Ferrotap.psd1') -Force
$r = New-FerrotapRule -Pattern 'x' -Status 201 -Body 'hi'
$r | ConvertTo-Json -Depth 5 | Write-Host
$m = Get-Module Ferrotap
& $m {
    param($r)
    $x = ConvertTo-FtRule $r
    Write-Host ('type body: ' + $(if ($null -eq $x.body) { 'null' } else { $x.body.GetType().FullName }))
    Write-Host ('type status: ' + $(if ($null -eq $x.status) { 'null' } else { $x.status.GetType().FullName }))
    try { $b = [System.Text.Encoding]::UTF8.GetBytes($x.body); Write-Host ('bytes ok ' + $b.GetType().FullName) } catch { Write-Host ('GetBytes: ' + $_.Exception.Message) }
    try { $s = [System.Text.Encoding]::UTF8.GetString($b); Write-Host 'getstring ok' } catch { Write-Host ('GetString: ' + $_.Exception.Message) }
    try { $o = [pscustomobject]@{ PSTypeName = 'Ferrotap.Response'; Rule = $x.name; Status = 1 }; Write-Host 'obj ok' } catch { Write-Host ('obj: ' + $_.Exception.Message) }
} $r

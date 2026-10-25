Import-Module (Join-Path $PSScriptRoot 'Ferrotap\Ferrotap.psd1') -Force
try {
    $r = New-FerrotapRule -Pattern 'x' -Status 201 -Body 'hi'
    $res = Get-FerrotapRuleResponse -Rule $r
    $res | Out-String | Write-Host
} catch {
    Write-Host $_.Exception.ToString()
    Write-Host $_.InvocationInfo.PositionMessage
    Write-Host $_.ScriptStackTrace
}

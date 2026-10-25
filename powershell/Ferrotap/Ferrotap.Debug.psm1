#requires -Version 5.1
<#
Ferrotap.Debug: interactive debugging + rule inspection on top of the Ferrotap module (public cmdlets only).
  Test-FerrotapRuleSyntax  validate rules (objects or rules.json) and report problems with hints
  Debug-FerrotapRule       step-through evaluation of one rule against one request (-Interactive pauses per step)
  Trace-FerrotapRequest    dry-run: show every rule's verdict for a request, mark the winner
  Show-FerrotapRule        human-readable inspection of rules
#>
Set-StrictMode -Version 2.0

function Get-DbgProp($o, [string]$n) { if ($null -ne $o -and $o.PSObject.Properties[$n]) { $o.$n } else { $null } }

function New-DbgIssue($Index, $Rule, $Field, $Sev, $Msg, $Hint) {
    [pscustomobject]@{ PSTypeName = 'Ferrotap.Issue'; Index = $Index; Rule = $Rule; Field = $Field; Severity = $Sev; Message = $Msg; Hint = $Hint }
}

function Resolve-DbgRules($Rule, [string]$Path) {
    if ($Rule) { return @($Rule) }
    if (-not $Path) { $Path = if ($env:FERROTAP_RULES_DIR) { $env:FERROTAP_RULES_DIR } else { '.' } }
    @(Get-FerrotapRule -Path $Path)
}

function Test-FerrotapRuleSyntax {
    <#
    .SYNOPSIS Validate rules and return one issue object per problem (Index, Rule, Field, Severity, Message, Hint). Empty output = all good.
    .EXAMPLE Test-FerrotapRuleSyntax -Path D:\rules\rules.json
    #>
    [CmdletBinding()]
    param([Parameter(ValueFromPipeline)]$Rule, [string]$Path)
    begin { $all = New-Object System.Collections.Generic.List[object] }
    process { if ($Rule) { $all.Add($Rule) } }
    end {
        if ($all.Count -eq 0) {
            if (-not $Path) { $Path = if ($env:FERROTAP_RULES_DIR) { $env:FERROTAP_RULES_DIR } else { '.' } }
            $file = if (Test-Path -LiteralPath $Path -PathType Container) { Join-Path $Path 'rules.json' } else { $Path }
            if (-not (Test-Path -LiteralPath $file)) { return New-DbgIssue 0 '' 'file' 'Error' "File not found: $file" 'Check the path or FERROTAP_RULES_DIR.' }
            try { $parsed = Get-Content -LiteralPath $file -Raw -Encoding UTF8 | ConvertFrom-Json } catch {
                return New-DbgIssue 0 '' 'file' 'Error' "Invalid JSON: $($_.Exception.Message)" 'rules.json must be an array of rule objects.'
            }
            foreach ($p in @($parsed)) { $all.Add($p) }
        }
        $i = 0
        foreach ($r in $all) {
            $i++
            $name = [string](Get-DbgProp $r 'name')
            $kind = [string](Get-DbgProp $r 'match_kind'); if (-not $kind) { $kind = 'contains' }
            $pat = [string](Get-DbgProp $r 'pattern')
            $action = [string](Get-DbgProp $r 'action'); if (-not $action) { $action = 'canned' }
            if ($kind -notin 'contains', 'regex', 'dsl') { New-DbgIssue $i $name 'match_kind' 'Error' "Unknown match_kind '$kind'" 'Use contains, regex or dsl.' }
            if ([string]::IsNullOrWhiteSpace($pat)) { New-DbgIssue $i $name 'pattern' 'Error' 'Pattern is empty' 'An empty pattern never matches.' }
            elseif ($kind -eq 'regex') {
                try { [void][regex]::new($pat, 'IgnoreCase') } catch { New-DbgIssue $i $name 'pattern' 'Error' "Bad regex: $($_.Exception.Message)" 'Escape literal dots/brackets with a backslash.' }
            }
            elseif ($kind -eq 'dsl') {
                try { [void](ConvertFrom-FerrotapFilter $pat) } catch { New-DbgIssue $i $name 'pattern' 'Error' $_.Exception.Message 'Operators: ~u ~d ~m ~c <arg>, combine with & | ! ( ).' }
            }
            $st = Get-DbgProp $r 'status'
            if ($null -ne $st -and ([int]$st -lt 100 -or [int]$st -gt 599)) { New-DbgIssue $i $name 'status' 'Error' "Status $st out of range" 'Use 100-599.' }
            $m = [string](Get-DbgProp $r 'method')
            if ($m -and $m -cne $m.ToUpperInvariant()) { New-DbgIssue $i $name 'method' 'Warning' "Method '$m' is not upper-case" "Use '$($m.ToUpperInvariant())'." }
            if ($action -eq 'file') {
                $f = [string](Get-DbgProp $r 'file')
                if (-not $f) { New-DbgIssue $i $name 'file' 'Error' 'action is file but file is empty' 'Set file to a path.' }
                elseif (-not (Test-Path -LiteralPath $f -PathType Leaf)) { New-DbgIssue $i $name 'file' 'Warning' "File not found: $f" 'The rule will answer 404.' }
            } elseif ($action -ne 'canned') { New-DbgIssue $i $name 'action' 'Error' "Unknown action '$action'" 'Use canned or file.' }
        }
    }
}

function Debug-FerrotapRule {
    <#
    .SYNOPSIS Step through one rule's evaluation for a request. -Interactive waits for Enter between steps (q quits).
    .EXAMPLE Debug-FerrotapRule -Rule $r -Method POST -Url https://a.io/login -Interactive
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)]$Rule, [string]$Method = 'GET', [Parameter(Mandatory)][string]$Url, [switch]$Interactive)
    $steps = New-Object System.Collections.Generic.List[object]
    $en = Get-DbgProp $Rule 'enabled'; if ($null -eq $en) { $en = $true }
    $kind = [string](Get-DbgProp $Rule 'match_kind'); if (-not $kind) { $kind = 'contains' }
    $pat = [string](Get-DbgProp $Rule 'pattern')
    $rm = [string](Get-DbgProp $Rule 'method')
    $steps.Add([pscustomobject]@{ Step = 1; Check = 'enabled'; Passed = [bool]$en; Detail = $(if ($en) { 'rule is enabled' } else { 'rule is disabled' }) })
    $mOk = (-not $rm) -or ($rm -ieq $Method)
    $steps.Add([pscustomobject]@{ Step = 2; Check = 'method'; Passed = $mOk; Detail = $(if ($rm) { "rule wants $rm, request is $Method" } else { 'rule accepts any method' }) })
    $syn = @(Test-FerrotapRuleSyntax -Rule $Rule | Where-Object Severity -eq 'Error')
    $steps.Add([pscustomobject]@{ Step = 3; Check = 'syntax'; Passed = ($syn.Count -eq 0); Detail = $(if ($syn.Count) { $syn[0].Message } else { "$kind pattern '$pat' is valid" }) })
    $k = 4
    if ($kind -eq 'dsl' -and $syn.Count -eq 0) {
        $f = Test-FerrotapFilter $pat -Method $Method -Url $Url
        foreach ($a in @($f.Atoms)) {
            $steps.Add([pscustomobject]@{ Step = $k++; Check = 'atom'; Passed = $null; Detail = ($a | Out-String).Trim() })
        }
    }
    $res = if ($syn.Count -eq 0) { Test-FerrotapRule -Rule $Rule -Method $Method -Url $Url } else { $null }
    $final = [bool]($res -and $res.Matched)
    $steps.Add([pscustomobject]@{ Step = $k; Check = 'verdict'; Passed = $final; Detail = $(if ($res) { $res.Reason } else { 'not evaluated (syntax error)' }) })
    foreach ($s in $steps) {
        if ($Interactive) {
            $ans = Read-Host ("[{0}] {1} ... Enter=run, q=quit" -f $s.Step, $s.Check)
            if ($ans -eq 'q') { break }
        }
        $mark = if ($null -eq $s.Passed) { '.' } elseif ($s.Passed) { 'PASS' } else { 'FAIL' }
        Write-Host ("  step {0} {1,-8} {2,-5} {3}" -f $s.Step, $s.Check, $mark, $s.Detail)
    }
    [pscustomobject]@{ PSTypeName = 'Ferrotap.DebugResult'; Matched = $final; Steps = $steps.ToArray() }
}

function Trace-FerrotapRequest {
    <#
    .SYNOPSIS Dry-run a request through all rules (first enabled match wins); shows every rule's verdict.
    .EXAMPLE Trace-FerrotapRequest -Url https://a.io/api/users -Method GET -Path D:\rules
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory, Position = 0)][string]$Url, [string]$Method = 'GET', [object[]]$Rule, [string]$Path, [switch]$Quiet)
    $rules = Resolve-DbgRules $Rule $Path
    $winner = $null; $i = 0; $rows = @()
    foreach ($r in $rules) {
        $i++
        $t = Test-FerrotapRule -Rule $r -Method $Method -Url $Url
        $won = $false
        if ($t.Matched -and -not $winner) { $winner = $i; $won = $true }
        $verdict = if ($won) { 'WINNER' } elseif ($t.Matched) { 'shadowed' } else { 'no match' }
        $rows += [pscustomobject]@{ PSTypeName = 'Ferrotap.TraceRow'; Index = $i; Rule = [string](Get-DbgProp $r 'name'); Verdict = $verdict; Reason = $t.Reason }
    }
    if (-not $Quiet) {
        Write-Host "Trace: $Method $Url  ($($rules.Count) rule(s))"
        foreach ($x in $rows) { Write-Host ("  #{0} {1,-10} {2,-20} {3}" -f $x.Index, $x.Verdict, $x.Rule, $x.Reason) }
        if (-not $winner) { Write-Host '  -> no rule matched; request would pass through to the network' }
    }
    [pscustomobject]@{ PSTypeName = 'Ferrotap.Trace'; Method = $Method; Url = $Url; Winner = $winner; Rows = $rows }
}

function Show-FerrotapRule {
    <#
    .SYNOPSIS Print a readable summary of rules (plus any syntax issues).
    #>
    [CmdletBinding()]
    param([object[]]$Rule, [string]$Path)
    $rules = Resolve-DbgRules $Rule $Path
    $i = 0
    foreach ($r in $rules) {
        $i++
        $on = Get-DbgProp $r 'enabled'; if ($null -eq $on) { $on = $true }
        $m = [string](Get-DbgProp $r 'method'); if (-not $m) { $m = '*' }
        Write-Host ("#{0} [{1}] {2}" -f $i, $(if ($on) { 'on ' } else { 'off' }), (Get-DbgProp $r 'name'))
        Write-Host ("     {0} {1}: {2}  -> {3} {4}" -f $m, (Get-DbgProp $r 'match_kind'), (Get-DbgProp $r 'pattern'), (Get-DbgProp $r 'action'), (Get-DbgProp $r 'status'))
        foreach ($e in @(Test-FerrotapRuleSyntax -Rule $r)) { Write-Host "     $($e.Severity): $($e.Field) - $($e.Message)" }
    }
}

Export-ModuleMember -Function Test-FerrotapRuleSyntax, Debug-FerrotapRule, Trace-FerrotapRequest, Show-FerrotapRule


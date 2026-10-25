#requires -Version 5.1
<#
Ferrotap PowerShell module.
 - Rule objects use the same field names as rules.json of the Rust app (name, enabled, match_kind, pattern,
   method, action, status, headers[{enabled,key,value}], body, file), so Save-FerrotapRule output loads in Ferrotap.
 - match_kind: contains | regex | dsl   (dsl = mitmproxy-style filter, same grammar as src/bpdsl.rs)
 - All matching is pure (input snapshot -> structured result), no network or proxy needed.
#>
Set-StrictMode -Version 2.0

function Get-FtRulesDir {
    if ($env:FERROTAP_RULES_DIR) { return $env:FERROTAP_RULES_DIR }
    return 'D:\Projects\Ferrotap\rules'
}

function Get-FtProp {
    param($Obj, [string[]]$Names)
    if ($null -eq $Obj) { return $null }
    foreach ($n in $Names) {
        if ($Obj -is [System.Collections.IDictionary]) {
            if ($Obj.Contains($n)) { return $Obj[$n] }
        } else {
            $p = $Obj.PSObject.Properties[$n]
            if ($p -and $null -ne $p.Value) { return $p.Value }
        }
    }
    return $null
}

# ======================= Filter DSL: tokenizer / parser / evaluator =======================

function ConvertTo-FtToken {
    param([string]$Text)
    $tokens = New-Object System.Collections.Generic.List[object]
    $i = 0; $n = $Text.Length
    while ($i -lt $n) {
        $c = $Text[$i]; $cs = [string]$c
        if ([char]::IsWhiteSpace($c)) { $i++; continue }
        if ($cs -in @('(', ')', '!', '&', '|')) {
            $tokens.Add([pscustomobject]@{ Kind = 'op'; Value = $cs; Pos = $i }); $i++; continue
        }
        if ($cs -eq '"') {
            $sb = New-Object System.Text.StringBuilder
            $start = $i; $i++; $closed = $false
            while ($i -lt $n) {
                $d = $Text[$i]
                if ($d -eq '\' -and ($i + 1) -lt $n -and ($Text[$i + 1] -eq '"' -or $Text[$i + 1] -eq '\')) {
                    [void]$sb.Append($Text[$i + 1]); $i += 2; continue
                }
                if ($d -eq '"') { $closed = $true; $i++; break }
                [void]$sb.Append($d); $i++
            }
            if (-not $closed) { throw "Filter syntax error: unterminated quote at position $start" }
            $tokens.Add([pscustomobject]@{ Kind = 'word'; Value = $sb.ToString(); Pos = $start }); continue
        }
        if ($cs -eq '~') {
            $j = $i + 1
            while ($j -lt $n -and [char]::IsLetter($Text[$j])) { $j++ }
            $tokens.Add([pscustomobject]@{ Kind = 'tilde'; Value = $Text.Substring($i + 1, $j - $i - 1).ToLowerInvariant(); Pos = $i })
            $i = $j; continue
        }
        $j = $i
        while ($j -lt $n) {
            $e = $Text[$j]
            if ([char]::IsWhiteSpace($e) -or ([string]$e -in @('(', ')', '&', '|', '"'))) { break }
            $j++
        }
        $tokens.Add([pscustomobject]@{ Kind = 'word'; Value = $Text.Substring($i, $j - $i); Pos = $i })
        $i = $j
    }
    return , $tokens
}

function ConvertTo-FtStatusRanges {
    param([string]$Spec)
    $out = New-Object System.Collections.Generic.List[object]
    foreach ($part in $Spec.Split(',')) {
        $p = $part.Trim()
        if ($p -match '^([1-5])xx$') {
            $d = [int]$Matches[1]; $out.Add(@(($d * 100), ($d * 100 + 99)))
        } elseif ($p -match '^(\d{3})-(\d{3})$') {
            $lo = [int]$Matches[1]; $hi = [int]$Matches[2]
            if ($lo -gt $hi) { throw "Filter syntax error: bad status range '$p'" }
            $out.Add(@($lo, $hi))
        } elseif ($p -match '^\d{3}$') {
            $out.Add(@([int]$p, [int]$p))
        } else {
            throw "Filter syntax error: bad status spec '$Spec' (use 404, 5xx, 400-499 or a comma list)"
        }
    }
    return , $out
}

function New-FtRegex {
    param([string]$Pattern)
    try {
        return New-Object System.Text.RegularExpressions.Regex($Pattern, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase, [TimeSpan]::FromSeconds(1))
    } catch {
        throw "Filter syntax error: invalid regex '$Pattern': $($_.Exception.Message)"
    }
}

function Read-FtOr {
    param($St)
    $left = Read-FtAnd $St
    while ($St.Pos -lt $St.Tokens.Count -and $St.Tokens[$St.Pos].Kind -eq 'op' -and $St.Tokens[$St.Pos].Value -eq '|') {
        $St.Pos++
        $right = Read-FtAnd $St
        $left = [pscustomobject]@{ Type = 'or'; Left = $left; Right = $right }
    }
    return $left
}

function Read-FtAnd {
    param($St)
    $left = Read-FtNot $St
    while ($St.Pos -lt $St.Tokens.Count -and $St.Tokens[$St.Pos].Kind -eq 'op' -and $St.Tokens[$St.Pos].Value -eq '&') {
        $St.Pos++
        $right = Read-FtNot $St
        $left = [pscustomobject]@{ Type = 'and'; Left = $left; Right = $right }
    }
    return $left
}

function Read-FtNot {
    param($St)
    if ($St.Pos -lt $St.Tokens.Count -and $St.Tokens[$St.Pos].Kind -eq 'op' -and $St.Tokens[$St.Pos].Value -eq '!') {
        $St.Pos++
        $inner = Read-FtNot $St
        return [pscustomobject]@{ Type = 'not'; Inner = $inner }
    }
    return Read-FtAtom $St
}

function Read-FtAtom {
    param($St)
    if ($St.Pos -ge $St.Tokens.Count) { throw 'Filter syntax error: unexpected end of filter' }
    $t = $St.Tokens[$St.Pos]; $St.Pos++
    if ($t.Kind -eq 'op' -and $t.Value -eq '(') {
        $e = Read-FtOr $St
        if ($St.Pos -ge $St.Tokens.Count -or $St.Tokens[$St.Pos].Value -ne ')' -or $St.Tokens[$St.Pos].Kind -ne 'op') {
            throw "Filter syntax error: missing ')' for '(' at position $($t.Pos)"
        }
        $St.Pos++
        return $e
    }
    if ($t.Kind -ne 'tilde') { throw "Filter syntax error: unexpected '$($t.Value)' at position $($t.Pos) (expected a ~ operator)" }
    $op = $t.Value
    if ($op -in @('all', 'q', 's')) {
        return [pscustomobject]@{ Type = 'atom'; Op = $op; Arg = $null; Regex = $null; Ranges = $null; Text = "~$op" }
    }
    if ($op -notin @('u', 'd', 'm', 'c')) { throw "Filter syntax error: unknown operator '~$op' at position $($t.Pos)" }
    if ($St.Pos -ge $St.Tokens.Count -or $St.Tokens[$St.Pos].Kind -ne 'word') { throw "Filter syntax error: '~$op' needs an argument" }
    $arg = $St.Tokens[$St.Pos].Value; $St.Pos++
    $re = $null; $ranges = $null
    if ($op -in @('u', 'd')) { $re = New-FtRegex $arg }
    if ($op -eq 'c') { $ranges = ConvertTo-FtStatusRanges $arg }
    return [pscustomobject]@{ Type = 'atom'; Op = $op; Arg = $arg; Regex = $re; Ranges = $ranges; Text = "~$op $arg" }
}

function Format-FtNode {
    param($Node)
    switch ($Node.Type) {
        'atom' { return $Node.Text }
        'not'  { return '!(' + (Format-FtNode $Node.Inner) + ')' }
        'and'  { return '(' + (Format-FtNode $Node.Left) + ' & ' + (Format-FtNode $Node.Right) + ')' }
        'or'   { return '(' + (Format-FtNode $Node.Left) + ' | ' + (Format-FtNode $Node.Right) + ')' }
    }
}

function Get-FtHost {
    param([string]$Url)
    if ($Url -match '^[A-Za-z][A-Za-z0-9+.\-]*://(?:[^/?#@]*@)?(\[[^\]]+\]|[^/?#:]+)') {
        return $Matches[1].Trim('[', ']')
    }
    return ''
}

function Invoke-FtNode {
    param($Node, $Ctx, $Trace)
    switch ($Node.Type) {
        'or' {
            if (Invoke-FtNode $Node.Left $Ctx $Trace) { return $true }
            return [bool](Invoke-FtNode $Node.Right $Ctx $Trace)
        }
        'and' {
            $tmp = New-Object System.Collections.Generic.List[string]
            if (-not (Invoke-FtNode $Node.Left $Ctx $tmp)) { return $false }
            if (-not (Invoke-FtNode $Node.Right $Ctx $tmp)) { return $false }
            foreach ($x in $tmp) { $Trace.Add($x) }
            return $true
        }
        'not' {
            $tmp = New-Object System.Collections.Generic.List[string]
            if (Invoke-FtNode $Node.Inner $Ctx $tmp) { return $false }
            $Trace.Add((Format-FtNode $Node))
            return $true
        }
        'atom' {
            $r = $false
            switch ($Node.Op) {
                'all' { $r = $true }
                'q'   { $r = ($Ctx.Stage -eq 'request') }
                's'   { $r = ($Ctx.Stage -eq 'response') }
                'm'   { $r = ($Ctx.Method -ieq $Node.Arg) }
                'u'   { try { $r = $Node.Regex.IsMatch($Ctx.Url) } catch [System.Text.RegularExpressions.RegexMatchTimeoutException] { $r = $false } }
                'd'   { try { $r = $Node.Regex.IsMatch($Ctx.Host) } catch [System.Text.RegularExpressions.RegexMatchTimeoutException] { $r = $false } }
                'c'   {
                    if ($Ctx.Stage -eq 'response' -and $null -ne $Ctx.Status) {
                        foreach ($rg in $Node.Ranges) { if ($Ctx.Status -ge $rg[0] -and $Ctx.Status -le $rg[1]) { $r = $true; break } }
                    }
                }
            }
            if ($r) { $Trace.Add($Node.Text) }
            return [bool]$r
        }
    }
}

function ConvertFrom-FerrotapFilter {
    <#
    .SYNOPSIS Parse a Ferrotap/mitmproxy-style filter (e.g. '~m GET & ~u /api/ & !~c 5xx') into a syntax tree.
    Throws a descriptive 'Filter syntax error' on bad input.
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory, ValueFromPipeline, Position = 0)][AllowEmptyString()][string]$Filter)
    process {
        $tokens = ConvertTo-FtToken $Filter
        if ($tokens.Count -eq 0) { throw 'Filter syntax error: empty filter' }
        $st = @{ Tokens = $tokens; Pos = 0 }
        $ast = Read-FtOr $st
        if ($st.Pos -lt $tokens.Count) {
            $t = $tokens[$st.Pos]
            throw "Filter syntax error: unexpected '$($t.Value)' at position $($t.Pos)"
        }
        $ast
    }
}

function Test-FerrotapFilter {
    <#
    .SYNOPSIS Evaluate a filter against a request/response snapshot. Returns a structured result (or [bool] with -Quiet).
    .EXAMPLE Test-FerrotapFilter '~m POST & ~u /login' -Method POST -Url https://a.io/login
    .EXAMPLE Test-FerrotapFilter '~c 5xx' -Url https://a.io/ -Status 503
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory, Position = 0)][string]$Filter,
        [string]$Method = 'GET',
        [string]$Url = '',
        [Nullable[int]]$Status = $null,
        [ValidateSet('auto', 'request', 'response')][string]$Stage = 'auto',
        [switch]$Quiet
    )
    $ast = ConvertFrom-FerrotapFilter $Filter
    if ($Stage -eq 'auto') { $Stage = if ($null -ne $Status) { 'response' } else { 'request' } }
    $ctx = @{ Method = $Method; Url = $Url; Host = (Get-FtHost $Url); Status = $Status; Stage = $Stage }
    $trace = New-Object System.Collections.Generic.List[string]
    $m = [bool](Invoke-FtNode $ast $ctx $trace)
    if ($Quiet) { return $m }
    [pscustomobject]@{
        PSTypeName = 'Ferrotap.FilterResult'
        Matched    = $m
        Filter     = Format-FtNode $ast
        Stage      = $Stage
        Atoms      = $trace.ToArray()
        Method     = $Method
        Url        = $Url
        Status     = $Status
    }
}

# ======================= Rules =======================

function ConvertTo-FtRule {
    # Normalise any object (e.g. from JSON) into a Ferrotap.Rule with all fields present.
    param($Obj)
    $hdr = @()
    $h = Get-FtProp $Obj @('headers')
    if ($null -ne $h) {
        foreach ($row in @($h)) {
            $en = Get-FtProp $row @('enabled'); if ($null -eq $en) { $en = $true }
            $hdr += [pscustomobject][ordered]@{ enabled = [bool]$en; key = [string](Get-FtProp $row @('key')); value = [string](Get-FtProp $row @('value')) }
        }
    }
    $v = @{}
    foreach ($pair in @(@('name', ''), @('match_kind', 'contains'), @('pattern', ''), @('method', ''), @('action', 'canned'), @('body', ''), @('file', ''))) {
        $x = Get-FtProp $Obj @($pair[0]); $v[$pair[0]] = if ($null -eq $x) { $pair[1] } else { [string]$x }
    }
    $en = Get-FtProp $Obj @('enabled'); if ($null -eq $en) { $en = $true }
    $st = Get-FtProp $Obj @('status'); if ($null -eq $st) { $st = 200 }
    [pscustomobject][ordered]@{
        PSTypeName = 'Ferrotap.Rule'
        name       = $v['name']
        enabled    = [bool]$en
        match_kind = $v['match_kind']
        pattern    = $v['pattern']
        method     = $v['method']
        action     = $v['action']
        status     = [int]$st
        headers    = $hdr
        body       = $v['body']
        file       = $v['file']
    }
}

function New-FerrotapRule {
    <#
    .SYNOPSIS Create a Ferrotap Auto Responder rule object (validates regex / filter immediately).
    .EXAMPLE New-FerrotapRule -Name 'mock users' -Pattern '/api/users' -Status 200 -Body '[]' -Header @{ 'Content-Type' = 'application/json' }
    .EXAMPLE New-FerrotapRule -Pattern '~m GET & ~u \.png$' -MatchKind dsl -File D:\img\x.png | Save-FerrotapRule
    #>
    [CmdletBinding()]
    [OutputType('Ferrotap.Rule')]
    param(
        [Parameter(Mandatory, Position = 0, ValueFromPipelineByPropertyName)][string]$Pattern,
        [Parameter(ValueFromPipelineByPropertyName)][string]$Name = '',
        [ValidateSet('contains', 'regex', 'dsl')][string]$MatchKind = 'contains',
        [string]$Method = '',
        [ValidateRange(100, 599)][int]$Status = 200,
        [System.Collections.IDictionary]$Header,
        [string]$Body = '',
        [string]$File = '',
        [switch]$Disabled
    )
    process {
        if ([string]::IsNullOrWhiteSpace($Pattern)) { throw 'Pattern must not be empty' }
        if ($MatchKind -eq 'regex') { [void](New-FtRegex $Pattern) }
        if ($MatchKind -eq 'dsl') { [void](ConvertFrom-FerrotapFilter $Pattern) }
        if ($File -and $Body) { throw 'Use either -Body or -File, not both' }
        $rows = @()
        if ($Header) { foreach ($k in $Header.Keys) { $rows += [pscustomobject][ordered]@{ enabled = $true; key = [string]$k; value = [string]$Header[$k] } } }
        ConvertTo-FtRule ([pscustomobject]@{
            name = $Name; enabled = (-not $Disabled); match_kind = $MatchKind; pattern = $Pattern; method = $Method.ToUpperInvariant()
            action = $(if ($File) { 'file' } else { 'canned' }); status = $Status; headers = $rows; body = $Body; file = $File
        })
    }
}

function Get-FerrotapRule {
    <#
    .SYNOPSIS Read rules from a rules.json (file, or a folder containing it; default FERROTAP_RULES_DIR).
    #>
    [CmdletBinding()]
    [OutputType('Ferrotap.Rule')]
    param([Parameter(Position = 0)][string]$Path = (Get-FtRulesDir), [string]$Name = '*')
    $file = if (Test-Path -LiteralPath $Path -PathType Container) { Join-Path $Path 'rules.json' } else { $Path }
    if (-not (Test-Path -LiteralPath $file)) { return }
    $raw = Get-Content -LiteralPath $file -Raw -Encoding UTF8
    if ([string]::IsNullOrWhiteSpace($raw)) { return }
    try { $parsed = $raw | ConvertFrom-Json } catch { throw "Cannot parse ${file}: $($_.Exception.Message)" }
    foreach ($o in @($parsed)) {
        $r = ConvertTo-FtRule $o
        if ($r.name -like $Name) { $r }
    }
}

function Save-FerrotapRule {
    <#
    .SYNOPSIS Write rules (from the pipeline) to rules.json in the Ferrotap format. -Append keeps existing rules.
    #>
    [CmdletBinding(SupportsShouldProcess)]
    param(
        [Parameter(Mandatory, ValueFromPipeline)]$Rule,
        [string]$Path = (Get-FtRulesDir),
        [switch]$Append
    )
    begin { $all = New-Object System.Collections.Generic.List[object] }
    process { $all.Add((ConvertTo-FtRule $Rule)) }
    end {
        $file = if ($Path -like '*.json') { $Path } else { Join-Path $Path 'rules.json' }
        $final = @()
        if ($Append) { $final += @(Get-FerrotapRule -Path $file) }
        $final += $all.ToArray()
        if ($PSCmdlet.ShouldProcess($file, "Write $($final.Count) rule(s)")) {
            $dir = Split-Path -Parent $file
            if ($dir -and -not (Test-Path -LiteralPath $dir)) { [void](New-Item -ItemType Directory -Path $dir -Force) }
            $plain = foreach ($r in $final) {
                [ordered]@{ name = $r.name; enabled = $r.enabled; match_kind = $r.match_kind; pattern = $r.pattern; method = $r.method
                    action = $r.action; status = $r.status
                    headers = @($r.headers | ForEach-Object { [ordered]@{ enabled = $_.enabled; key = $_.key; value = $_.value } })
                    body = $r.body; file = $r.file }
            }
            $json = ConvertTo-Json -InputObject @($plain) -Depth 6
            [System.IO.File]::WriteAllText($file, $json, (New-Object System.Text.UTF8Encoding($false)))
            Get-Item -LiteralPath $file
        }
    }
}

function Test-FtRuleCore {
    param($Rule, [string]$Method, [string]$Url, $Status, [string]$Stage)
    $enabled = Get-FtProp $Rule @('enabled'); if ($null -eq $enabled) { $enabled = $true }
    $pattern = [string](Get-FtProp $Rule @('pattern'))
    $rmethod = [string](Get-FtProp $Rule @('method'))
    $kind = [string](Get-FtProp $Rule @('match_kind')); if (-not $kind) { $kind = 'contains' }
    $res = @{ Matched = $false; Reason = ''; Atoms = @() }
    if (-not $enabled) { $res.Reason = 'rule is disabled'; return $res }
    if (-not $pattern) { $res.Reason = 'rule has an empty pattern'; return $res }
    if ($rmethod -and $rmethod -ine $Method) { $res.Reason = "method mismatch (rule $($rmethod.ToUpperInvariant()), request $Method)"; return $res }
    switch ($kind) {
        'regex' {
            try { $re = New-FtRegex $pattern } catch { $res.Reason = $_.Exception.Message; return $res }
            try { $ok = $re.IsMatch($Url) } catch { $ok = $false }
            $res.Matched = [bool]$ok
            $res.Reason = if ($ok) { 'URL matches regex' } else { 'URL does not match regex' }
        }
        'dsl' {
            try { $t = Test-FerrotapFilter -Filter $pattern -Method $Method -Url $Url -Status $Status -Stage $Stage }
            catch { $res.Reason = $_.Exception.Message; return $res }
            $res.Matched = $t.Matched; $res.Atoms = $t.Atoms
            $res.Reason = if ($t.Matched) { 'filter matched: ' + ($t.Atoms -join ', ') } else { "filter did not match ($($t.Filter))" }
        }
        default {
            $ok = $Url.Contains($pattern)
            $res.Matched = $ok
            $res.Reason = if ($ok) { 'URL contains pattern' } else { 'URL does not contain pattern' }
        }
    }
    return $res
}

function Test-FerrotapRule {
    <#
    .SYNOPSIS Does this rule match a request? Returns a structured result explaining why (or [bool] with -Quiet).
    .EXAMPLE New-FerrotapRule -Pattern '/api/' | Test-FerrotapRule -Url https://x.io/api/users -Method GET
    #>
    [CmdletBinding()]
    [OutputType('Ferrotap.MatchResult')]
    param(
        [Parameter(Mandatory, ValueFromPipeline)]$Rule,
        [Parameter(Mandatory)][string]$Url,
        [string]$Method = 'GET',
        [Nullable[int]]$Status = $null,
        [ValidateSet('auto', 'request', 'response')][string]$Stage = 'auto',
        [switch]$Quiet
    )
    process {
        if ($Stage -eq 'auto') { $Stage = if ($null -ne $Status) { 'response' } else { 'request' } }
        $r = Test-FtRuleCore $Rule $Method $Url $Status $Stage
        if ($Quiet) { return [bool]$r.Matched }
        [pscustomobject]@{
            PSTypeName = 'Ferrotap.MatchResult'
            Rule       = [string](Get-FtProp $Rule @('name'))
            Matched    = [bool]$r.Matched
            Reason     = $r.Reason
            Atoms      = @($r.Atoms)
            Method     = $Method
            Url        = $Url
            Status     = $Status
        }
    }
}

function Get-FerrotapRuleResponse {
    <#
    .SYNOPSIS Build the response a rule would produce (status, headers, body), like the Auto Responder.
    #>
    [CmdletBinding()]
    [OutputType('Ferrotap.Response')]
    param([Parameter(Mandatory, ValueFromPipeline)]$Rule)
    process {
        $r = ConvertTo-FtRule $Rule
        $hdr = New-Object System.Collections.Generic.List[object]
        foreach ($h in $r.headers) { if ($h.enabled -and $h.key.Trim()) { $hdr.Add([pscustomobject]@{ Name = $h.key.Trim(); Value = $h.value }) } }
        $hasCt = [bool]($hdr | Where-Object { $_.Name -ieq 'content-type' })
        $status = $r.status; $bytes = $null
        if ($r.action -eq 'file') {
            if (Test-Path -LiteralPath $r.file -PathType Leaf) {
                $bytes = [System.IO.File]::ReadAllBytes($r.file)
                if (-not $hasCt) {
                    $ext = [System.IO.Path]::GetExtension($r.file).ToLowerInvariant()
                    $ct = switch ($ext) {
                        '.json' { 'application/json' }
                        '.html' { 'text/html; charset=utf-8' }
                        '.htm'  { 'text/html; charset=utf-8' }
                        '.txt'  { 'text/plain; charset=utf-8' }
                        '.js'   { 'application/javascript' }
                        '.css'  { 'text/css' }
                        '.xml'  { 'application/xml' }
                        '.png'  { 'image/png' }
                        '.jpg'  { 'image/jpeg' }
                        '.jpeg' { 'image/jpeg' }
                        '.gif'  { 'image/gif' }
                        default { 'application/octet-stream' }
                    }
                    $hdr.Add([pscustomobject]@{ Name = 'Content-Type'; Value = $ct })
                }
            } else {
                $status = 404; $hdr.Clear()
                $hdr.Add([pscustomobject]@{ Name = 'Content-Type'; Value = 'text/plain; charset=utf-8' })
                $bytes = [System.Text.Encoding]::UTF8.GetBytes("AutoResponder: cannot read file $($r.file)")
            }
        } else {
            if (-not $hasCt) { $hdr.Add([pscustomobject]@{ Name = 'Content-Type'; Value = 'text/plain; charset=utf-8' }) }
            $bytes = [System.Text.Encoding]::UTF8.GetBytes($r.body)
        }
        [pscustomobject]@{
            PSTypeName = 'Ferrotap.Response'
            Rule       = $r.name
            Status     = $status
            Headers    = $hdr.ToArray()
            BodyBytes  = $bytes
            Body       = [System.Text.Encoding]::UTF8.GetString($bytes)
        }
    }
}

function Invoke-FerrotapTap {
    <#
    .SYNOPSIS Dry-run sessions through the rule list (first enabled match wins, like the Auto Responder).
    Input: URL strings, or objects with Method/Url/Status properties (e.g. parsed sessions.json). Needs no running proxy.
    .EXAMPLE 'https://x.io/api/users' | Invoke-FerrotapTap
    .EXAMPLE Get-Content sessions.json -Raw | ConvertFrom-Json | Invoke-FerrotapTap -MatchedOnly
    #>
    [CmdletBinding()]
    [OutputType('Ferrotap.TapResult')]
    param(
        [Parameter(Mandatory, ValueFromPipeline)]$Session,
        [object[]]$Rule,
        [string]$RulesPath = (Get-FtRulesDir),
        [switch]$MatchedOnly
    )
    begin {
        $rules = if ($PSBoundParameters.ContainsKey('Rule')) { @($Rule | ForEach-Object { ConvertTo-FtRule $_ }) } else { @(Get-FerrotapRule -Path $RulesPath) }
    }
    process {
        if ($Session -is [string]) { $method = 'GET'; $url = $Session; $status = $null }
        else {
            $method = [string](Get-FtProp $Session @('Method', 'method', 'verb')); if (-not $method) { $method = 'GET' }
            $url = [string](Get-FtProp $Session @('Url', 'url', 'uri', 'Uri'))
            $status = Get-FtProp $Session @('Status', 'status', 'StatusCode', 'status_code')
            if ($null -ne $status) { $status = [int]$status }
        }
        $hit = $null; $why = 'no enabled rule matched'
        foreach ($r in $rules) {
            $t = Test-FtRuleCore $r $method $url $null 'request'
            if ($t.Matched) { $hit = $r; $why = $t.Reason; break }
        }
        if ($MatchedOnly -and -not $hit) { return }
        [pscustomobject]@{
            PSTypeName = 'Ferrotap.TapResult'
            Method     = $method
            Url        = $url
            Matched    = [bool]$hit
            Rule       = $(if ($hit) { $hit.name } else { $null })
            Reason     = $why
            Response   = $(if ($hit) { Get-FerrotapRuleResponse $hit } else { $null })
        }
    }
}

Export-ModuleMember -Function New-FerrotapRule, Get-FerrotapRule, Save-FerrotapRule, Test-FerrotapRule, Test-FerrotapFilter, ConvertFrom-FerrotapFilter, Get-FerrotapRuleResponse, Invoke-FerrotapTap



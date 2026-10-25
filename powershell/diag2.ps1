$r = [pscustomobject]@{ name='x'; file=''; body='hi'; action='canned'; status=201; headers=@() }
$hdr = New-Object System.Collections.Generic.List[object]
$status = 201
$bytes = [System.Text.Encoding]::UTF8.GetBytes($r.body)
$bstr = [System.Text.Encoding]::UTF8.GetString($bytes)
Write-Host "bytes: $($bytes.Length)"
$arr = $hdr.ToArray()
$res = [pscustomobject]@{
    PSTypeName = 'Ferrotap.Response'
    Rule = $r.name
    Status = [int]$status
    Headers = $arr
    BodyBytes = $bytes
    Body = $bstr
}
Write-Host "Done: $($res.Body)"

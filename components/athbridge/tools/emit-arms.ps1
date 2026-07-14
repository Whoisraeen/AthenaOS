$in = Join-Path $env:TEMP "athena-winapi\apiset-hosts.txt"
Get-Content $in | ForEach-Object {
    $p = $_ -split "`t"
    $apiset = $p[0]
    $host1 = (($p[1] -split ',')[0]) -replace ':.*',''
    if ($host1 -ne '(no-forwarders)') {
        '        "{0}" => "{1}",' -f $apiset, $host1
    }
}

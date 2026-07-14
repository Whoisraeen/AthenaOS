# Derive the authoritative API Set -> host DLL map from this machine's real
# forwarder stubs in C:\Windows\System32\downlevel. Each api-ms-win-*.dll
# forwards its exports to a real host DLL (kernel32 / kernelbase / ...).
#
# Emits, per api set DLL: "apiset<TAB>host1:count1,host2:count2,...".
# The dominant host is the redirection target AthBridge uses.
# Pure ASCII, output to $env:TEMP (never the repo).
param(
    [string]$Dumpbin = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64\dumpbin.exe",
    [string]$OutDir  = "$env:TEMP\athena-winapi"
)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$stubs = Get-ChildItem "C:\Windows\System32\downlevel\api-ms-win-*.dll" -ErrorAction SilentlyContinue
$lines = @()
foreach ($s in $stubs) {
    $raw = & $Dumpbin /EXPORTS $s.FullName 2>&1
    $hosts = @{}
    foreach ($line in $raw) {
        if ($line -match 'forwarded to ([A-Za-z0-9_-]+)\.') {
            $h = $matches[1].ToLower()
            if ($hosts.ContainsKey($h)) { $hosts[$h]++ } else { $hosts[$h] = 1 }
        }
    }
    $apiset = $s.BaseName.ToLower()
    if ($hosts.Count -eq 0) {
        $lines += ("{0}`t(no-forwarders)" -f $apiset)
    } else {
        $parts = $hosts.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object { "{0}:{1}" -f $_.Key, $_.Value }
        $lines += ("{0}`t{1}" -f $apiset, ($parts -join ","))
    }
}
$lines | Sort-Object | Set-Content "$OutDir\apiset-hosts.txt"
Write-Output ("mapped {0} api set stubs -> {1}\apiset-hosts.txt" -f $stubs.Count, $OutDir)
Write-Output "=== distinct hosts ==="
$allHosts = @{}
foreach ($l in $lines) {
    if ($l -match "`t(.+)$") {
        foreach ($p in ($matches[1] -split ',')) {
            if ($p -match '^([a-z0-9_-]+):') {
                $h = $matches[1]
                if ($allHosts.ContainsKey($h)) { $allHosts[$h]++ } else { $allHosts[$h] = 1 }
            }
        }
    }
}
$allHosts.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object { "{0,-16} {1}" -f $_.Key, $_.Value }

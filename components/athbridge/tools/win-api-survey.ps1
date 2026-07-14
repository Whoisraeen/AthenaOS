# AthBridge Win32 API survey - ground-truth data gathering.
#
# Runs on a real Windows box against C:\Windows\System32 to produce:
#   1. exports\<dll>.txt   - authoritative exported-name list per core DLL
#   2. import-freq.txt     - "count<TAB>dll!name" ranked by how many real
#                            System32 .exe binaries import each symbol
#
# This is the data-driven hit list the AthBridge Wine strategy calls for
# (docs/components/athbridge-wine-strategy.md section 7, Phase A item 1):
# implement coverage in real-world frequency order instead of guessing.
#
# Pure ASCII (PS 5.1 safe). Output goes to $env:TEMP (never the repo).
param(
    [string]$Dumpbin = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64\dumpbin.exe",
    [string]$OutDir  = "$env:TEMP\athena-winapi",
    [int]$MaxExe     = 500
)

if (-not (Test-Path $Dumpbin)) { Write-Error "dumpbin not found: $Dumpbin"; exit 1 }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
New-Item -ItemType Directory -Force -Path "$OutDir\exports" | Out-Null

$coreDlls = @(
    "kernel32","kernelbase","ntdll","user32","gdi32","gdi32full","advapi32",
    "msvcrt","ucrtbase","ole32","combase","oleaut32","shell32","shlwapi",
    "comctl32","comdlg32","ws2_32","winmm","version","setupapi","iphlpapi",
    "crypt32","bcrypt","ncrypt","psapi","userenv","dbghelp","dwmapi","uxtheme",
    "rpcrt4","secur32","wininet","winhttp","d3d11","d3d12","dxgi","d3d9",
    "opengl32","dsound","dinput8","xinput1_4","imm32","gdiplus"
)

Write-Output "=== EXPORTS ==="
foreach ($d in $coreDlls) {
    $path = "C:\Windows\System32\$d.dll"
    if (Test-Path $path) {
        $raw = & $Dumpbin /EXPORTS $path 2>&1
        $names = $raw |
            Select-String -Pattern '^\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]{8}\s+(\S+)' |
            ForEach-Object { $_.Matches[0].Groups[1].Value } |
            Sort-Object -Unique
        $names | Set-Content "$OutDir\exports\$d.txt"
        Write-Output ("{0,-14} {1}" -f $d, $names.Count)
    }
}

Write-Output "=== IMPORT FREQUENCY ==="
$freq = @{}
$exes = Get-ChildItem "C:\Windows\System32\*.exe" -ErrorAction SilentlyContinue |
        Select-Object -First $MaxExe
$scanned = 0
foreach ($e in $exes) {
    $scanned++
    $raw = & $Dumpbin /IMPORTS $e.FullName 2>&1
    $curDll = $null
    foreach ($line in $raw) {
        if ($line -match '^\s+(\S+\.[Dd][Ll][Ll])\s*$') { $curDll = $matches[1].ToLower(); continue }
        if ($curDll -and $line -match '^\s+[0-9A-Fa-f]+\s+([A-Za-z_][A-Za-z0-9_@]+)\s*$') {
            $key = "$curDll!$($matches[1])"
            if ($freq.ContainsKey($key)) { $freq[$key]++ } else { $freq[$key] = 1 }
        }
    }
}
$freq.GetEnumerator() |
    Sort-Object Value -Descending |
    ForEach-Object { "{0}`t{1}" -f $_.Value, $_.Name } |
    Set-Content "$OutDir\import-freq.txt"

Write-Output ("scanned {0} exes, {1} distinct dll!name pairs" -f $scanned, $freq.Count)
Write-Output ("output: {0}" -f $OutDir)

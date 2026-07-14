# Resolve AthenaOS panic backtrace offsets to kernel functions.
#
# Usage:
#   powershell -File scripts\resolve-panic.ps1 0x123abc 0x456def ...
#
# Feed it the "+0x...." file-offsets printed by the panic handler
# (the second column of each "[PANIC]  0x....  +0x...." line). Each is
# matched to the kernel function whose symbol range contains it.
param([Parameter(ValueFromRemainingArguments=$true)][string[]]$Offsets)

$bin = (rustc --print sysroot) + "\lib\rustlib\x86_64-pc-windows-msvc\bin"
$kernel = Join-Path $PSScriptRoot "..\target\x86_64-unknown-none\release\kernel"
if (-not (Test-Path $kernel)) { Write-Error "kernel ELF not found: $kernel"; exit 1 }
Write-Output "[resolve] using ELF: $kernel"

# Build a sorted (addr, name) table once.
$lines = & "$bin\llvm-nm.exe" --numeric-sort --demangle $kernel 2>$null
$syms = foreach ($l in $lines) {
    if ($l -match '^([0-9a-fA-F]+)\s+\S+\s+(.+)$') {
        [pscustomobject]@{ Addr = [Convert]::ToUInt64($matches[1],16); Name = $matches[2] }
    }
}
$syms = $syms | Sort-Object Addr

foreach ($o in $Offsets) {
    $val = [Convert]::ToUInt64(($o -replace '^\+',''), 16)
    $hit = $null
    foreach ($s in $syms) { if ($s.Addr -le $val) { $hit = $s } else { break } }
    if ($hit) {
        $delta = $val - $hit.Addr
        Write-Output ("+{0:x}  ->  {1}  (+{2:x} into fn)" -f $val, $hit.Name, $delta)
    } else {
        Write-Output ("+{0:x}  ->  <no symbol below this address>" -f $val)
    }
}

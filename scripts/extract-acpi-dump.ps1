# extract-acpi-dump.ps1 — pull self-dumped ACPI tables out of a BOOTLOG dump.
#
# The kernel base64-dumps any firmware table that fails AML parsing into the
# bootlog ring (kernel/src/acpi_full.rs::dump_table_to_bootlog), framed as:
#     [acpi-dump] BEGIN <name>.dat
#     <base64 lines>
#     [acpi-dump] END <name>.dat
# This script decodes every block in a BOOTLOG dump (the output of
# scripts\read-bootlog.ps1) into target\acpi_dump\<name>.dat — exactly the
# layout the kernel's `embed_test_dsdt` feature and the host AML harness eat.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\extract-acpi-dump.ps1
#   powershell ... -File scripts\extract-acpi-dump.ps1 -In BOOTLOG.dump.txt -OutDir target\acpi_dump

param(
    [string]$In = "BOOTLOG.dump.txt",
    [string]$OutDir = "target\acpi_dump"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $In)) { throw "input not found: $In (run scripts\read-bootlog.ps1 first)" }
New-Item -ItemType Directory -Force $OutDir | Out-Null

$name = $null
$sb = $null
$count = 0
foreach ($line in [System.IO.File]::ReadLines((Resolve-Path $In))) {
    if ($line -match '^\[acpi-dump\] BEGIN (\S+)') {
        $name = $Matches[1]
        $sb = New-Object System.Text.StringBuilder
        continue
    }
    if ($line -match '^\[acpi-dump\] END (\S+)') {
        if ($null -ne $sb -and $Matches[1] -eq $name) {
            $bytes = [Convert]::FromBase64String($sb.ToString())
            $out = Join-Path $OutDir $name
            [System.IO.File]::WriteAllBytes($out, $bytes)
            $sig = [System.Text.Encoding]::ASCII.GetString($bytes[0..3])
            Write-Host ("[extract-acpi] {0}: {1} bytes (sig '{2}')" -f $out, $bytes.Length, $sig)
            # The embed feature also wants the DSDT at target\dsdt.dat.
            if ($name -ieq 'dsdt.dat') {
                [System.IO.File]::WriteAllBytes("target\dsdt.dat", $bytes)
                Write-Host "[extract-acpi] also wrote target\dsdt.dat"
            }
            $count++
        }
        $name = $null; $sb = $null
        continue
    }
    if ($null -ne $sb -and $line -match '^[A-Za-z0-9+/=]+$') {
        [void]$sb.Append($line.Trim())
    }
}
if ($count -eq 0) {
    Write-Host "[extract-acpi] no [acpi-dump] blocks found — every table parsed cleanly (or the dump predates the self-dump kernel)"
} else {
    Write-Host "[extract-acpi] extracted $count table(s) -> $OutDir"
}

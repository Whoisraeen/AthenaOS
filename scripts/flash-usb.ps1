# flash-usb.ps1 - write a AthenaOS disk image to a USB stick, SAFELY.
#
# Safety model (because a prior install wiped a Windows partition):
#   * HARD GUARD: refuses to write any disk whose BusType is not 'USB'. Your
#     internal NVMe/SATA drives are physically unreachable by this script even
#     if you fat-finger the disk number.
#   * Auto-detects the single USB disk; if 0 or >1 are present it makes you pass
#     -Disk N explicitly and prints the candidates.
#   * Shows the exact target (number, model, size) and requires you to type the
#     disk's SIZE to confirm (skip with -Force).
#
# Usage (ADMIN PowerShell):
#   powershell -ExecutionPolicy Bypass -File scripts\flash-usb.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\flash-usb.ps1 -Disk 2
#   powershell -ExecutionPolicy Bypass -File scripts\flash-usb.ps1 -Image target\x86_64-unknown-none\release\kernel.uefi.img
#
# After it finishes: eject the stick, boot Athena from it (Secure Boot OFF),
# bring it back, then run scripts\read-bootlog.ps1 to pull BOOTLOG.TXT.

param(
    [int]$Disk = -1,
    [string]$Image = "target\x86_64-unknown-none\release\kernel.uefi.img",
    [switch]$Force
)

$ErrorActionPreference = "Stop"

# -- preconditions ------------------------------------------------------------

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "raw disk write needs an ADMIN PowerShell."
}
if (-not (Test-Path $Image)) {
    throw "image not found: $Image  (build it: cargo run -p xtask --release -- build --release --safe)"
}
$imageFull = (Resolve-Path $Image).Path
$imageSize = (Get-Item $imageFull).Length
Write-Host "[flash-usb] image: $imageFull ($([math]::Round($imageSize/1MB,1)) MB)"

# -- pick the USB disk (auto or explicit), then HARD-GUARD on BusType ----------

if ($Disk -lt 0) {
    $usb = @(Get-Disk | Where-Object { $_.BusType -eq 'USB' })
    if ($usb.Count -eq 0) { throw "no USB disks found - insert the stick, then re-run (or pass -Disk N from 'Get-Disk')." }
    if ($usb.Count -gt 1) {
        $list = ($usb | ForEach-Object { "  disk $($_.Number): $($_.FriendlyName) ($([math]::Round($_.Size/1GB,1)) GB)" }) -join "`n"
        throw "multiple USB disks found - pass -Disk N:`n$list"
    }
    $Disk = $usb[0].Number
}

$target = Get-Disk -Number $Disk
if ($null -eq $target) { throw "no disk number $Disk" }

# THE GUARD. Internal drives are NVMe/SATA/RAID, never 'USB'.
if ($target.BusType -ne 'USB') {
    throw "REFUSING: disk $Disk is '$($target.BusType)' ($($target.FriendlyName)), not USB. This script only writes USB sticks so it can never touch your internal drives."
}
if ($target.Size -gt 256GB) {
    throw "REFUSING: disk $Disk is $([math]::Round($target.Size/1GB)) GB - too large for a boot stick; aborting out of caution. Pass a smaller USB or double-check the number."
}

$sizeGB = [math]::Round($target.Size/1GB,1)
Write-Host ""
Write-Host "[flash-usb] TARGET  -> disk $Disk : $($target.FriendlyName)  ($sizeGB GB, $($target.BusType))" -ForegroundColor Yellow
Write-Host "[flash-usb] This ERASES that disk completely and writes the AthenaOS image." -ForegroundColor Yellow
Write-Host ""

if (-not $Force) {
    $answer = Read-Host "Type the disk SIZE in GB ($sizeGB) to confirm, or anything else to abort"
    if ($answer -ne "$sizeGB") { Write-Host "[flash-usb] aborted - no changes made."; exit 1 }
}

# -- write -------------------------------------------------------------------

Write-Host "[flash-usb] clearing partitions on disk $Disk ..."
Clear-Disk -Number $Disk -RemoveData -RemoveOEM -Confirm:$false -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Set-Disk -Number $Disk -IsOffline $true -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

$dev = "\\.\PhysicalDrive$Disk"
Write-Host "[flash-usb] writing image to $dev ..."
$in  = [System.IO.File]::Open($imageFull, 'Open', 'Read', 'Read')
$out = [System.IO.File]::Open($dev, 'Open', 'ReadWrite', 'None')
try {
    $buf = New-Object byte[] (4MB)
    $written = [long]0
    while ($true) {
        $n = $in.Read($buf, 0, $buf.Length)
        if ($n -le 0) { break }
        $out.Write($buf, 0, $n)
        $written += $n
        Write-Progress -Activity "Flashing $dev" -Status "$([math]::Round($written/1MB,1)) / $([math]::Round($imageSize/1MB,1)) MB" -PercentComplete (($written / $imageSize) * 100)
    }
    $out.Flush()
    Write-Host "[flash-usb] wrote $([math]::Round($written/1MB,1)) MB."
} finally {
    $in.Close(); $out.Close()
}

Set-Disk -Number $Disk -IsOffline $false -ErrorAction SilentlyContinue
Write-Progress -Activity "Flashing" -Completed
Write-Host ""
Write-Host "[flash-usb] DONE. Eject the stick, boot Athena from it (Secure Boot OFF, UEFI), then run:" -ForegroundColor Green
Write-Host "    powershell -ExecutionPolicy Bypass -File scripts\read-bootlog.ps1" -ForegroundColor Green

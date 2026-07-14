# get-bootlog.ps1 - one-click: pull BOOTLOG.TXT off the RaeenOS USB and open it.
#
# Windows won't show the RaeenOS stick in Explorer (its partition is an EFI
# System Partition, which Windows refuses to mount on removable media). This
# reads the log straight off the raw disk instead - same end result as dragging
# the file, but it works.
#
# Use: double-click scripts\get-bootlog.bat, approve the UAC prompt. The log
# lands on your Desktop as BOOTLOG.dump.txt and opens in Notepad. No typing.
#
# Portable: copy get-bootlog.bat + get-bootlog.ps1 + read-bootlog.ps1 into any
# folder on any Windows PC and double-click - no repo needed.

param(
    [string]$Out = "$env:USERPROFILE\Desktop\BOOTLOG.dump.txt",
    [int]$Disk = -1
)
$ErrorActionPreference = "Stop"
$self = $MyInvocation.MyCommand.Path
$here = Split-Path -Parent $self
$reader = Join-Path $here "read-bootlog.ps1"
if (-not (Test-Path $reader)) { Write-Host "read-bootlog.ps1 not found next to this script."; Start-Sleep 5; exit 1 }

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
$isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

if (-not $isAdmin) {
    # Re-launch elevated (raw disk read needs admin), wait, then open the result.
    $a = @('-NoProfile','-ExecutionPolicy','Bypass','-File',"`"$self`"",'-Out',"`"$Out`"")
    if ($Disk -ge 0) { $a += @('-Disk',"$Disk") }
    try {
        Start-Process powershell -Verb RunAs -Wait -ArgumentList $a
    } catch {
        Write-Host "Elevation was cancelled - the log read needs admin. Nothing was changed."
        Start-Sleep 5; exit 1
    }
    if (Test-Path $Out) {
        Write-Host "Saved: $Out"
        Start-Process notepad $Out
    } else {
        Write-Host "No log was produced (is the RaeenOS stick plugged in?)."
        Start-Sleep 5
    }
    return
}

# Elevated: do the actual raw read.
$rargs = @('-Out', $Out)
if ($Disk -ge 0) { $rargs += @('-Disk', $Disk) }
& $reader @rargs

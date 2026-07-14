# netlog-listen.ps1 - capture AthenaOS boot logs broadcast over the LAN.
#
# AthenaOS (kernel/src/netlog.rs) UDP-broadcasts its in-RAM boot log on port
# 51514     no DHCP lease, no USB stick, no disk writes on the target. Run this
# BEFORE booting Athena, leave it running, boot; the log assembles here live.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\netlog-listen.ps1
#   ... boot Athena ... Ctrl+C when "snapshot complete" appears (or whenever).
#
# Output: BOOTLOG.netlog.txt in the current directory (override with -Out).
# Windows Firewall will ask once to allow inbound UDP for PowerShell     allow
# on Private networks.
#
# Wire format per datagram: "RLG1" + boot_id(u32 LE) + seq(u16 LE) +
# total(u16 LE) + chunk bytes. Snapshots are re-sent; chunks are
# last-write-wins keyed by seq, so repeated passes fill holes.

param(
    [int]$Port = 51514,
    [string]$Out = "BOOTLOG.netlog.txt"
)

$ErrorActionPreference = "Stop"

$udp = New-Object System.Net.Sockets.UdpClient
$udp.ExclusiveAddressUse = $false
$udp.Client.SetSocketOption([Net.Sockets.SocketOptionLevel]::Socket, [Net.Sockets.SocketOptionName]::ReuseAddress, $true)
$udp.Client.Bind((New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Any, $Port)))
$udp.Client.ReceiveTimeout = 1000

Write-Host "[netlog-listen] listening on UDP $Port     boot Athena now (Ctrl+C to stop)"

$chunks = @{}          # seq -> byte[]
$bootId = $null
$total = 0
$lastSave = Get-Date

function Save-Log {
    if ($chunks.Count -eq 0) { return }
    $ms = New-Object System.IO.MemoryStream
    foreach ($seq in ($chunks.Keys | Sort-Object)) { $ms.Write($chunks[$seq], 0, $chunks[$seq].Length) }
    # Accept an absolute -Out too: Join-Path rejects a rooted path ("format is
    # not supported"), which silently lost a whole capture on 2026-07-06. Resolve
    # against the current dir only when $Out is relative.
    if ([System.IO.Path]::IsPathRooted($Out)) { $dest = $Out } else { $dest = Join-Path (Get-Location) $Out }
    [IO.File]::WriteAllBytes($dest, $ms.ToArray())
}

try {
    while ($true) {
        $remote = New-Object System.Net.IPEndPoint([System.Net.IPAddress]::Any, 0)
        try { $data = $udp.Receive([ref]$remote) } catch { continue }  # timeout tick

        if ($data.Length -lt 12) { continue }
        if ([Text.Encoding]::ASCII.GetString($data, 0, 4) -ne "RLG1") { continue }

        $id  = [BitConverter]::ToUInt32($data, 4)
        $seq = [BitConverter]::ToUInt16($data, 8)
        $tot = [BitConverter]::ToUInt16($data, 10)

        if ($null -eq $bootId -or $id -ne $bootId) {
            if ($null -ne $bootId) { Write-Host "" }
            Write-Host ("[netlog-listen] NEW BOOT from {0} (boot_id=0x{1:x8}, {2} chunks expected)" -f $remote.Address, $id, $tot)
            $bootId = $id; $chunks.Clear()
        }
        $total = $tot
        $chunks[[int]$seq] = $data[12..($data.Length-1)]

        Write-Host ("`r[netlog-listen] {0}/{1} chunks" -f $chunks.Count, $total) -NoNewline
        if ($chunks.Count -ge $total -and $total -gt 0) {
            Save-Log
            Write-Host ("`n[netlog-listen] snapshot COMPLETE -> {0} ({1} chunks). Still listening for newer passes..." -f $Out, $total)
        } elseif (((Get-Date) - $lastSave).TotalSeconds -gt 5) {
            Save-Log; $lastSave = Get-Date   # periodic partial save
        }
    }
} finally {
    Save-Log
    $udp.Close()
    Write-Host ("`n[netlog-listen] saved {0} chunk(s) -> {1}" -f $chunks.Count, $Out)
}

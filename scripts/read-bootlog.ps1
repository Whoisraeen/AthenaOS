# read-bootlog.ps1 - extract BOOTLOG.TXT from a AthenaOS boot stick without
# mounting it.
#
# Why: Windows refuses to assign a drive letter to an ESP-typed partition on
# removable media (diskpart VDS: "The operation is not supported on removable
# media"), so the obvious diskpart/mountvol route is a dead end. This script
# instead reads the raw disk (\\.\PhysicalDriveN), walks GPT/MBR -> FAT16/32
# ESP -> root directory -> BOOTLOG.TXT cluster chain - the same steps
# kernel/src/bootlog_persist.rs performs when writing it.
#
# Usage (ADMIN PowerShell):
#   powershell -ExecutionPolicy Bypass -File scripts\read-bootlog.ps1            # auto-detect the USB disk
#   powershell -ExecutionPolicy Bypass -File scripts\read-bootlog.ps1 -Disk 2    # explicit disk number
#   powershell -ExecutionPolicy Bypass -File scripts\read-bootlog.ps1 -ImagePath target\usb-msc.img   # test on an image file
#
# Output: BOOTLOG.dump.txt next to the current directory (override with -Out),
# plus the first lines printed to the console. Zero-padding gaps inside the
# file (between the locked early-boot region and the latest ring tail) are
# collapsed to a visible marker.

param(
    [int]$Disk = -1,
    [string]$ImagePath = "",
    [string]$Out = "BOOTLOG.dump.txt"
)

$ErrorActionPreference = "Stop"

# -- raw byte access ----------------------------------------------------------

$script:Stream = $null

function Open-Target {
    if ($ImagePath -ne "") {
        if (-not (Test-Path $ImagePath)) { throw "image not found: $ImagePath" }
        $script:Stream = [System.IO.File]::Open($ImagePath, 'Open', 'Read', 'ReadWrite')
        Write-Host "[read-bootlog] reading image file: $ImagePath"
        return
    }

    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "raw disk access needs an ADMIN PowerShell (or use -ImagePath for a file)"
    }

    $n = $Disk
    if ($n -lt 0) {
        $usb = @(Get-Disk | Where-Object { $_.BusType -eq 'USB' })
        if ($usb.Count -eq 0) { throw "no USB disks found - pass -Disk N explicitly (see 'Get-Disk')" }
        if ($usb.Count -gt 1) {
            $list = ($usb | ForEach-Object { "  disk $($_.Number): $($_.FriendlyName) ($([math]::Round($_.Size/1GB))GB)" }) -join "`n"
            throw "multiple USB disks found - pass -Disk N:`n$list"
        }
        $n = $usb[0].Number
        Write-Host "[read-bootlog] auto-detected USB disk $n ($($usb[0].FriendlyName))"
    }
    $script:Stream = [System.IO.File]::Open("\\.\PhysicalDrive$n", 'Open', 'Read', 'ReadWrite')
}

function Read-Sectors([long]$Lba, [int]$Count) {
    $buf = New-Object byte[] ($Count * 512)
    $null = $script:Stream.Seek($Lba * 512, 'Begin')
    $read = 0
    while ($read -lt $buf.Length) {
        $r = $script:Stream.Read($buf, $read, $buf.Length - $read)
        if ($r -le 0) { throw "short read at LBA $Lba" }
        $read += $r
    }
    return $buf
}

function U16([byte[]]$b, [int]$o) { return [uint32][BitConverter]::ToUInt16($b, $o) }
function U32([byte[]]$b, [int]$o) { return [uint32][BitConverter]::ToUInt32($b, $o) }
function U64([byte[]]$b, [int]$o) { return [uint64][BitConverter]::ToUInt64($b, $o) }

# -- partition table walk (GPT preferred, MBR fallback) -----------------------

function Find-EspStart {
    $mbr = Read-Sectors 0 1
    if ($mbr[510] -ne 0x55 -or $mbr[511] -ne 0xAA) { throw "sector 0 has no MBR/GPT signature" }

    if ($mbr[446 + 4] -eq 0xEE) {
        # GPT: header at LBA 1 points at the partition entry array.
        $hdr = Read-Sectors 1 1
        if ([Text.Encoding]::ASCII.GetString($hdr, 0, 8) -ne "EFI PART") { throw "protective MBR but no GPT header" }
        $entryLba = U64 $hdr 72
        $entrySize = U32 $hdr 84
        $entries = Read-Sectors ([long]$entryLba) 1
        # EFI System Partition type GUID C12A7328-F81F-11D2-BA4B-00A0C93EC93B (LE bytes below)
        $espGuid = @(0x28,0x73,0x2A,0xC1,0x1F,0xF8,0xD2,0x11,0xBA,0x4B,0x00,0xA0,0xC9,0x3E,0xC9,0x3B)
        for ($i = 0; $i -lt [int](512 / $entrySize); $i++) {
            $o = $i * $entrySize
            $match = $true
            for ($j = 0; $j -lt 16; $j++) { if ($entries[$o + $j] -ne $espGuid[$j]) { $match = $false; break } }
            if ($match) { return [long](U64 $entries ($o + 32)) }
        }
        throw "GPT present but no ESP-type partition in the first entry sector"
    }

    # MBR: first FAT-ish partition (FAT32 0x0B/0x0C, FAT16 0x04/0x06/0x0E, ESP 0xEF).
    for ($i = 0; $i -lt 4; $i++) {
        $o = 446 + $i * 16
        $type = $mbr[$o + 4]
        if (@(0x0B,0x0C,0x04,0x06,0x0E,0xEF) -contains $type) {
            $start = U32 $mbr ($o + 8)
            if ($start -gt 0) { return [long]$start }
        }
    }
    throw "MBR present but no FAT partition entry"
}

# -- FAT16/FAT32 (mirrors kernel parse_vbr_any + bootlog_persist locate) ------

function Read-BootlogFile([long]$EspStart) {
    $vbr = Read-Sectors $EspStart 1
    if ($vbr[510] -ne 0x55 -or $vbr[511] -ne 0xAA) { throw "ESP VBR missing 0x55AA signature" }
    $bps      = U16 $vbr 11
    $spc      = [uint32]$vbr[13]
    $reserved = U16 $vbr 14
    $numFats  = [uint32]$vbr[16]
    $rootEnts = U16 $vbr 17
    $fatSz16  = U16 $vbr 22
    $fatSz32  = U32 $vbr 36
    $rootClus = U32 $vbr 44
    if ($bps -ne 512) { throw "unsupported bytes/sector: $bps" }

    $isFat32 = ($fatSz16 -eq 0 -and $fatSz32 -ne 0)
    if ($isFat32) { $fatSz = $fatSz32 } else { $fatSz = $fatSz16 }
    $fatLba = $EspStart + $reserved
    $rootDirSectors = [long][math]::Ceiling(($rootEnts * 32) / 512.0)
    if ($isFat32) {
        $dataStart = $fatLba + $numFats * $fatSz
        Write-Host "[read-bootlog] FAT32 ESP at LBA $EspStart (root cluster $rootClus)"
    } else {
        $rootLba = $fatLba + $numFats * $fatSz
        $dataStart = $rootLba + $rootDirSectors
        Write-Host "[read-bootlog] FAT16 ESP at LBA $EspStart (root dir $rootDirSectors sectors at LBA $rootLba)"
    }

    # FAT entry reader (cached one sector at a time).
    $script:fatCacheLba = -1
    $script:fatCache = $null
    function Get-FatEntry([uint32]$cluster) {
        if ($isFat32) { $w = 4 } else { $w = 2 }
        $byteOff = [long]$cluster * $w
        $lba = $fatLba + [long][math]::Floor($byteOff / 512)
        $off = [int]($byteOff % 512)
        if ($lba -ne $script:fatCacheLba) {
            $script:fatCache = Read-Sectors $lba 1
            $script:fatCacheLba = $lba
        }
        if ($isFat32) { return (U32 $script:fatCache $off) -band 0x0FFFFFFF }
        return U16 $script:fatCache $off
    }
    function Test-Eoc([uint32]$v) {
        if ($isFat32) { return $v -ge 0x0FFFFFF8 }
        return $v -ge 0xFFF8
    }

    # Scan one 32-byte-entry directory sector for "BOOTLOG TXT". Returns
    # @{cluster;size}, the string "END" at end-of-directory, or $null.
    function Find-InDirSector([byte[]]$sec) {
        for ($slot = 0; $slot -lt 16; $slot++) {
            $o = $slot * 32
            if ($sec[$o] -eq 0x00) { return "END" }
            if ($sec[$o] -eq 0xE5) { continue }
            $attr = $sec[$o + 11]
            if (($attr -band 0x0F) -eq 0x0F) { continue }   # LFN fragment
            if (($attr -band 0x08) -ne 0)    { continue }   # volume label
            $name = [Text.Encoding]::ASCII.GetString($sec, $o, 11)
            if ($name -eq "BOOTLOG TXT") {
                $hi = U16 $sec ($o + 20)
                $lo = U16 $sec ($o + 26)
                return @{ cluster = (($hi -shl 16) -bor $lo); size = (U32 $sec ($o + 28)) }
            }
        }
        return $null
    }

    # Locate the directory entry.
    $entry = $null
    if ($isFat32) {
        $cluster = $rootClus
        $guard = 0
        while ($cluster -ge 2 -and -not (Test-Eoc $cluster) -and $guard -lt 64 -and $null -eq $entry) {
            $guard++
            $lba = $dataStart + ([long]$cluster - 2) * $spc
            for ($s = 0; $s -lt $spc; $s++) {
                $r = Find-InDirSector (Read-Sectors ($lba + $s) 1)
                if ($r -is [string]) { break }
                if ($null -ne $r) { $entry = $r; break }
            }
            if ($null -eq $entry) { $cluster = Get-FatEntry $cluster }
        }
    } else {
        for ($s = 0; $s -lt $rootDirSectors; $s++) {
            $r = Find-InDirSector (Read-Sectors ($rootLba + $s) 1)
            if ($r -is [string]) { break }
            if ($null -ne $r) { $entry = $r; break }
        }
    }
    if ($null -eq $entry) { throw "BOOTLOG.TXT not found in the ESP root directory - was this stick flashed with a current image?" }
    Write-Host "[read-bootlog] BOOTLOG.TXT: first cluster $($entry.cluster), $($entry.size) bytes"

    # Follow the chain and collect the data.
    $ms = New-Object System.IO.MemoryStream
    $cluster = [uint32]$entry.cluster
    $hops = 0
    while ($cluster -ge 2 -and -not (Test-Eoc $cluster) -and $ms.Length -lt $entry.size -and $hops -lt 4096) {
        $hops++
        $lba = $dataStart + ([long]$cluster - 2) * $spc
        $data = Read-Sectors $lba ([int]$spc)
        $ms.Write($data, 0, $data.Length)
        $cluster = Get-FatEntry $cluster
    }
    $bytes = $ms.ToArray()
    if ($bytes.Length -gt $entry.size) { $bytes = $bytes[0..($entry.size - 1)] }
    return $bytes
}

# -- main ---------------------------------------------------------------------

try {
    Open-Target
    $espStart = Find-EspStart
    $bytes = Read-BootlogFile $espStart
} finally {
    if ($null -ne $script:Stream) { $script:Stream.Close() }
}

# Collapse zero-padding runs (between the locked early region and the latest
# ring tail) into a visible marker, then save as text.
$text = [Text.Encoding]::UTF8.GetString($bytes)
$text = [regex]::Replace($text, "`0{16,}", "`n`n[... zero padding ...]`n`n")
$text = $text -replace "`0", ""
[System.IO.File]::WriteAllText($Out, $text, (New-Object Text.UTF8Encoding $false))

$lines = $text -split "`n"
Write-Host "[read-bootlog] saved $($bytes.Length) bytes -> $Out ($($lines.Count) lines)"
Write-Host "[read-bootlog] -- first 25 lines --"
$lines | Select-Object -First 25 | ForEach-Object { Write-Host "  $_" }
Write-Host "[read-bootlog] -- (full log in $Out) --"

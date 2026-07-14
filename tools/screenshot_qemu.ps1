# QMP-driven QEMU screenshot harness for RaeenOS (ADR 0004).
# Boots a built disk image headlessly, waits for a serial sentinel + a compositor
# settle delay, then captures the framebuffer via QMP `screendump` format=png
# (QEMU 7.1+; avoids the PPM->PNG striping artifact in project memory).
# Lead-run only (subagent Bash is sandboxed). Pure ASCII (PS 5.1 safe).
param(
  [Parameter(Mandatory=$true)][string]$Image,
  [Parameter(Mandatory=$true)][string]$Out,
  [string]$Marker = "System successfully booted",
  [double]$Settle = 7.0,
  [double]$Timeout = 120.0,
  [int]$QmpPort = 5599,
  [string]$Smp = "2",
  [switch]$Uefi
)
$ErrorActionPreference = "Stop"
$qemu = $env:RAEEN_QEMU
if (-not $qemu) { $qemu = "C:\Program Files\qemu\qemu-system-x86_64.exe" }
$serial = Join-Path $env:TEMP "raeen-screenshot-serial.log"
if (Test-Path $serial) { Remove-Item $serial -Force }
$outAbs = [System.IO.Path]::GetFullPath($Out)

# Mirror xtask run_qemu arg order EXACTLY (UEFI boots there): OVMF pflash FIRST,
# then the boot image as a default-if drive, then the dummy virtio disk +
# isa-debug-exit, so OVMF finds the ESP boot device (otherwise -no-reboot exits
# before serial and QMP connect fails).
$qargs = @()
if ($Uefi) {
  $ovmf = $env:RAEEN_OVMF
  if (-not $ovmf) { $ovmf = "C:\Program Files\qemu\share\edk2-x86_64-code.fd" }
  $qargs += @("-drive", "if=pflash,format=raw,readonly=on,file=$ovmf")
}
$qargs += @("-drive", "file=$Image,format=raw")
$dummy = Join-Path (Split-Path $Image -Parent) "virtio-shot.img"
if (-not (Test-Path $dummy)) { [System.IO.File]::WriteAllBytes($dummy, (New-Object byte[] (1MB))) }
$qargs += @(
  "-drive", "file=$dummy,format=raw,if=virtio,index=1",
  "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
  "-m", "2G",
  "-smp", $Smp,
  "-display", "none",
  "-no-reboot",
  "-qmp", "tcp:127.0.0.1:$QmpPort,server,nowait",
  "-serial", "file:$($serial -replace '\\','/')"
)
Write-Host "[shot] launching QEMU"
$proc = Start-Process -FilePath $qemu -ArgumentList $qargs -PassThru -WindowStyle Hidden

function Wait-Marker($path, $mark, $timeoutSec) {
  $deadline = (Get-Date).AddSeconds($timeoutSec)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $path) {
      $txt = Get-Content $path -Raw -ErrorAction SilentlyContinue
      if ($txt -and $txt.Contains($mark)) { return $true }
    }
    Start-Sleep -Milliseconds 500
  }
  return $false
}

function Qmp-Recv($stream, $reader) {
  $line = $reader.ReadLine()
  if ($line) { return $line } else { return "" }
}

try {
  if (Wait-Marker $serial $Marker $Timeout) {
    Write-Host "[shot] marker seen; settling $Settle s"
  } else {
    Write-Host "[shot] WARN: marker not seen in $Timeout s; capturing anyway"
  }
  Start-Sleep -Seconds $Settle

  $client = $null
  for ($i = 0; $i -lt 40; $i++) {
    try { $client = New-Object System.Net.Sockets.TcpClient("127.0.0.1", $QmpPort); break }
    catch { Start-Sleep -Milliseconds 250 }
  }
  if (-not $client) { Write-Host "[shot] FAIL: QMP connect"; exit 2 }
  $stream = $client.GetStream()
  $reader = New-Object System.IO.StreamReader($stream)
  $writer = New-Object System.IO.StreamWriter($stream)
  $writer.NewLine = "`r`n"; $writer.AutoFlush = $true

  $greeting = Qmp-Recv $stream $reader
  Write-Host "[shot] QMP greeting: $([bool]($greeting -match 'QMP'))"
  $writer.WriteLine('{"execute":"qmp_capabilities"}'); [void](Qmp-Recv $stream $reader)
  $fn = $outAbs -replace '\\','/'
  $writer.WriteLine('{"execute":"screendump","arguments":{"filename":"' + $fn + '","format":"png"}}')
  $resp = Qmp-Recv $stream $reader
  Write-Host "[shot] screendump resp: $resp"
  if ($resp -match '"error"') {
    $ppm = ($fn -replace '\.png$','.ppm')
    $writer.WriteLine('{"execute":"screendump","arguments":{"filename":"' + $ppm + '"}}')
    $resp2 = Qmp-Recv $stream $reader
    Write-Host "[shot] ppm fallback resp: $resp2"
    $outAbs = ($outAbs -replace '\.png$','.ppm')
  }
  $writer.WriteLine('{"execute":"quit"}')
  $client.Close()
} finally {
  Start-Sleep -Milliseconds 500
  if (-not $proc.HasExited) { $proc.Kill() }
}

if (Test-Path $outAbs) {
  Write-Host "[shot] OK: $outAbs ($((Get-Item $outAbs).Length) bytes)"
  exit 0
}
Write-Host "[shot] FAIL: no output produced"
exit 3

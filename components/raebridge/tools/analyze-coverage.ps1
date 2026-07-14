# Cross-reference the real-world import frequency survey against the shims
# actually implemented in winapi_shims.rs, and rank the STILL-MISSING imports
# by how many real System32 binaries need them. This is the data-driven
# coverage hit list (raebridge-wine-strategy.md section 7, Phase A item 1).
#
# "Implemented" = the function name appears as a real shim entry in the
# execution-path shim_table() (macro entry!/nt_entry!/... or an explicit
# ("<dll>.dll","<name>",...) tuple). Name-based coverage is conservative for
# the MISSING set: a name present nowhere is definitely unimplemented.
# Pure ASCII. Reads $env:TEMP survey output; writes the ranked gap list there.
param(
    [string]$Shims  = "components\raebridge\src\winapi_shims.rs",
    [string]$OutDir = "$env:TEMP\raeen-winapi",
    [int]$Top       = 80
)

$freqFile = Join-Path $OutDir "import-freq.txt"
if (-not (Test-Path $freqFile)) { Write-Error "run win-api-survey.ps1 first"; exit 1 }

# 1. Implemented shim names from winapi_shims.rs.
$impl = New-Object System.Collections.Generic.HashSet[string]
$src = Get-Content $Shims
foreach ($line in $src) {
    if ($line -match '(?:entry|nt_entry|adv_entry|user_entry|gdi_entry|comdlg_entry)!\("([^"]+)"') {
        [void]$impl.Add($matches[1])
    }
    if ($line -match '\("[A-Za-z0-9_]+\.dll",\s*"([^"]+)"') {
        [void]$impl.Add($matches[1])
    }
}
Write-Output ("implemented shim names: {0}" -f $impl.Count)

# 2. Aggregate survey by function NAME (summed across all contract DLLs), and
#    remember the dominant contract so we know the target module.
$count = @{}
$example = @{}
foreach ($line in (Get-Content $freqFile)) {
    $p = $line -split "`t"
    if ($p.Count -lt 2) { continue }
    $c = [int]$p[0]
    $dllname = $p[1]
    $bang = $dllname.IndexOf('!')
    if ($bang -lt 0) { continue }
    $dll = $dllname.Substring(0, $bang)
    $fn  = $dllname.Substring($bang + 1)
    if ($count.ContainsKey($fn)) { $count[$fn] += $c } else { $count[$fn] = $c; $example[$fn] = $dll }
    if ($c -gt 0 -and $count[$fn] -eq $c) { $example[$fn] = $dll }
}

# 3. Missing = summed-frequency names not implemented anywhere.
$missing = @()
foreach ($fn in $count.Keys) {
    if (-not $impl.Contains($fn)) {
        $missing += [pscustomobject]@{ Name = $fn; Total = $count[$fn]; Via = $example[$fn] }
    }
}
$ranked = $missing | Sort-Object Total -Descending
$out = Join-Path $OutDir "missing-ranked.txt"
$ranked | ForEach-Object { "{0}`t{1}`t{2}" -f $_.Total, $_.Name, $_.Via } | Set-Content $out
Write-Output ("missing distinct names: {0}  ->  {1}" -f $ranked.Count, $out)
Write-Output ("=== TOP {0} STILL-MISSING (count / name / via-contract) ===" -f $Top)
$ranked | Select-Object -First $Top | ForEach-Object { "{0,5}  {1,-40} {2}" -f $_.Total, $_.Name, $_.Via }

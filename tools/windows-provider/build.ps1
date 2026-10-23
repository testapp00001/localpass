# Builds the Windows passkey provider (release) and lays out the loose-file package in target\provider\pkg.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\windows-provider\build.ps1 [-TargetDir <cargo target dir>]
#
# -TargetDir defaults to $env:CARGO_TARGET_DIR, else target\provider-build under the repo. It is never the
# repo's target\release, so a running localpass.exe is never touched. Does not register anything.
param(
    [string]$TargetDir = $env:CARGO_TARGET_DIR,
    [switch]$NoBuild
)
. (Join-Path $PSScriptRoot 'common.ps1')

if (-not $TargetDir) { $TargetDir = Join-Path $RepoRoot 'target\provider-build' }
$Exe = Join-Path $TargetDir "release\$ExeName"

if (-not $NoBuild) {
    # A running provider keeps its exe locked; Windows starts it again on demand.
    Stop-Provider
    $previous = $env:CARGO_TARGET_DIR
    $env:CARGO_TARGET_DIR = $TargetDir
    Push-Location $RepoRoot
    # cargo writes its progress to stderr: that must not count as a PowerShell error.
    $ErrorActionPreference = 'Continue'
    try {
        cargo build --release -p lp-winauth --bin lp-winauth-provider 2>&1 | Select-Object -Last 15 | ForEach-Object { "$_" }
        $cargoExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = 'Stop'
        Pop-Location
        $env:CARGO_TARGET_DIR = $previous
    }
    if ($cargoExit -ne 0) { throw "cargo build failed (exit $cargoExit)" }
}
if (-not (Test-Path $Exe)) { throw "$Exe not found (build first)." }

# The shipped manifest carries the final class id; it must be the one compiled into the exe (src/provider.rs).
$clsid = (& $Exe clsid).Trim().Trim('{', '}')
if ($clsid -notmatch '^[0-9A-Fa-f]{8}-([0-9A-Fa-f]{4}-){3}[0-9A-Fa-f]{12}$') { throw "unexpected class id '$clsid'" }

Stop-Provider
$assets = Join-Path $PkgDir 'Assets'
New-Item -ItemType Directory -Force $assets | Out-Null
Copy-Item $Exe (Join-Path $PkgDir $ExeName) -Force
foreach ($logo in 'StoreLogo', 'Square44x44Logo', 'Square150x150Logo') {
    Copy-Item (Join-Path $RepoRoot "app\src-tauri\icons\$logo.png") $assets -Force
}
$template = Get-Content -Raw -Encoding UTF8 (Join-Path $RepoRoot 'crates\lp-winauth\package\AppxManifest.xml')
if ($template -notmatch [regex]::Escape("<com:Class Id=`"$clsid`"")) { throw "package\AppxManifest.xml does not name the class id $clsid the exe has" }
[System.IO.File]::WriteAllText((Join-Path $PkgDir 'AppxManifest.xml'), $template, (New-Object System.Text.UTF8Encoding($false)))

$size = [math]::Round((Get-Item (Join-Path $PkgDir $ExeName)).Length / 1MB, 1)
Write-Host "built: $PkgDir ($ExeName $size MB, class {$clsid})"

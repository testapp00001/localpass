# Shared by build.ps1, register.ps1, unregister.ps1 and status.ps1 (dot-sourced). No admin rights needed.
$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$PackageName = 'LocalPass.Passkeys'
$ExeName = 'lp-winauth-provider.exe'
# The loose-file package lives here (under the repo's target\, so `cargo clean` deletes it: unregister first).
$ProviderDir = Join-Path $RepoRoot 'target\provider'
$PkgDir = Join-Path $ProviderDir 'pkg'
# The execution alias runs the exe WITH package identity (needed by register/unregister/status/pair).
$Alias = Join-Path $env:LOCALAPPDATA "Microsoft\WindowsApps\$ExeName"
$RegKey = 'HKCU:\Software\LocalPass\Provider'

# Only the provider's own processes are ever stopped here, never localpass.exe.
function Stop-Provider {
    Get-Process -Name 'lp-winauth-provider' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

function Get-ProviderPackage {
    Get-AppxPackage -Name $PackageName -ErrorAction SilentlyContinue
}

# Runs the alias (package identity) and returns its output lines; $script:AliasExit has the exit code.
function Invoke-Alias {
    param([string[]]$Arguments)
    if (-not (Test-Path $Alias)) { throw "The execution alias $Alias does not exist (is the package registered?)." }
    $output = & $Alias @Arguments 2>&1
    $script:AliasExit = $LASTEXITCODE
    $output | ForEach-Object { "$_" }
}

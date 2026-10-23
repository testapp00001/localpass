# Registers the loose-file package built by build.ps1 (Developer Mode, no certificate), registers LocalPass with
# Windows' passkey providers (DISABLED: you enable it in Settings with Windows Hello), and records where the
# app is so the provider can start it.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\windows-provider\register.ps1 [-AppPath <localpass.exe>]
#
# Idempotent: a package that is already registered from the same folder is kept (same version twice would
# fail with 0x80073CF9); `lp-winauth-provider register` updates an existing registration and keeps its key.
param(
    [string]$AppPath
)
. (Join-Path $PSScriptRoot 'common.ps1')

$manifest = Join-Path $PkgDir 'AppxManifest.xml'
if (-not (Test-Path $manifest)) { throw "$manifest not found: run build.ps1 first." }

$existing = Get-ProviderPackage
if ($existing -and ($existing.InstallLocation -ne $PkgDir)) {
    throw "$PackageName is registered from $($existing.InstallLocation), not $PkgDir. Run unregister.ps1 first."
}
if (-not $existing) {
    Add-AppxPackage -Register $manifest
    Write-Host "package registered: $PackageName"
} else {
    Write-Host "package already registered: $PackageName"
}

# The execution alias appears a moment after registration.
for ($i = 0; $i -lt 40 -and -not (Test-Path $Alias); $i++) { Start-Sleep -Milliseconds 250 }
if (-not (Test-Path $Alias)) { throw "The execution alias $Alias did not appear." }

$output = Invoke-Alias @('register')
$output | ForEach-Object { Write-Host "  $_" }
if ($script:AliasExit -ne 0) { throw "lp-winauth-provider register failed (exit $script:AliasExit)" }

# Where the app is: the provider starts it (minimized) when its pipe is missing. Default: the running
# release build, else the repo's release build.
if (-not $AppPath) {
    $running = Get-Process -Name 'localpass' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -like '*\release\localpass.exe' } | Select-Object -First 1
    if ($running) { $AppPath = $running.Path }
    elseif (Test-Path (Join-Path $RepoRoot 'target\release\localpass.exe')) { $AppPath = Join-Path $RepoRoot 'target\release\localpass.exe' }
}
if ($AppPath -and (Test-Path $AppPath)) {
    New-Item -Path $RegKey -Force | Out-Null
    Set-ItemProperty -Path $RegKey -Name 'AppPath' -Value (Resolve-Path $AppPath).Path
    Write-Host "app path: $AppPath"
} else {
    Write-Host "app path: not set (pass -AppPath <localpass.exe> so the provider can start the app)"
}
Write-Host "done. LocalPass is registered but DISABLED: enable it in Settings > Accounts > Passkeys > Advanced options."

# Removes the passkey provider: unregisters the authenticator from Windows (through the package identity, which
# must happen BEFORE the package goes away, or the per-user HKLM entry is orphaned), removes the package and
# deletes target\provider. The pairing in %LOCALAPPDATA%\LocalPass\provider is kept; pass -ForgetPairing to
# delete it as well.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\windows-provider\unregister.ps1 [-ForgetPairing]
param(
    [switch]$ForgetPairing
)
. (Join-Path $PSScriptRoot 'common.ps1')

$package = Get-ProviderPackage
if ($package) {
    if (Test-Path $Alias) {
        $output = Invoke-Alias @('unregister')
        $output | ForEach-Object { Write-Host "  $_" }
        if ($script:AliasExit -ne 0) {
            throw "unregister failed (exit $script:AliasExit): NOT removing the package, so the registration can still be removed. Fix and run again."
        }
    } else {
        throw "The execution alias is missing, so the authenticator cannot be unregistered: NOT removing the package."
    }
    Stop-Provider
    Remove-AppxPackage -Package $package.PackageFullName
    Write-Host "package removed: $PackageName"
} else {
    Write-Host "package not registered: $PackageName"
}

if (Test-Path $ProviderDir) {
    Stop-Provider
    Remove-Item -Recurse -Force $ProviderDir
    Write-Host "deleted $ProviderDir"
}
if (Test-Path $RegKey) {
    Remove-ItemProperty -Path $RegKey -Name 'AppPath' -ErrorAction SilentlyContinue
    if (-not (Get-Item $RegKey).GetValueNames().Count) { Remove-Item $RegKey -ErrorAction SilentlyContinue }
}
if ($ForgetPairing) {
    $data = Join-Path $env:LOCALAPPDATA 'LocalPass\provider'
    Remove-Item (Join-Path $data 'client.bin') -Force -ErrorAction SilentlyContinue
    Write-Host "pairing forgotten (also remove the client in LocalPass > Settings)"
}
Write-Host "done."

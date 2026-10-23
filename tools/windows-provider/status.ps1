# Shows the state of the passkey provider: package, registration in Windows, pairing, app, recent log.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\windows-provider\status.ps1 [-Log <lines>]
param(
    [int]$Log = 8
)
. (Join-Path $PSScriptRoot 'common.ps1')

$package = Get-ProviderPackage
if ($package) {
    Write-Host "package: $($package.PackageFullName)  status=$($package.Status)  signature=$($package.SignatureKind)  dev=$($package.IsDevelopmentMode)"
    Write-Host "         $($package.InstallLocation)"
} else {
    Write-Host "package: $PackageName is not registered"
}
Write-Host "alias:   $(if (Test-Path $Alias) { $Alias } else { 'missing' })"
$processes = Get-Process -Name 'lp-winauth-provider' -ErrorAction SilentlyContinue
Write-Host "server:  $(if ($processes) { 'running, pid ' + ($processes.Id -join ',') } else { 'not running (Windows starts it on demand)' })"

# The OS keeps one key per registered plugin, per user. Reading it needs no admin rights.
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$plugins = "HKLM:\SOFTWARE\Microsoft\FIDO\$sid\Plugins"
if (Test-Path $plugins) {
    Get-ChildItem $plugins | ForEach-Object {
        $p = Get-ItemProperty $_.PSPath
        $state = switch ($p.State) { 0 { 'disabled' } 1 { 'ENABLED' } default { "state $($p.State)" } }
        Write-Host ("plugin:  {0}  {1}  {2}  PackageSignatureKind={3}" -f $_.PSChildName, $p.Name, $state, $p.PackageSignatureKind)
    }
} else {
    Write-Host "plugin:  no registrations under $plugins"
}

if (Test-Path $Alias) {
    Write-Host "--- lp-winauth-provider status"
    Invoke-Alias @('status') | ForEach-Object { Write-Host "  $_" }
}

$logFile = Join-Path $env:LOCALAPPDATA 'LocalPass\provider\provider.log'
if ($Log -gt 0 -and (Test-Path $logFile)) {
    Write-Host "--- last $Log line(s) of $logFile"
    Get-Content $logFile -Tail $Log | ForEach-Object { Write-Host "  $_" }
}

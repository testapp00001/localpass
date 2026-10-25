# Run by the NSIS uninstall hook (windows/hooks.nsh) before the installed files are deleted. Shipped in
# the install folder as uninstall-cleanup.ps1. It undoes what Settings > Browser extension and
# Settings > Windows passkeys set up outside the install folder. Every path arrives in an environment
# variable (never spliced into a command line), so tests can point it at throwaway places:
#
#   LP_DIR            the install folder's windows-passkeys folder (the registered package's location)
#   LP_ALIAS          the provider's execution alias (%LOCALAPPDATA%\Microsoft\WindowsApps\...)
#   LP_EXE            the installed localpass.exe
#   LP_NM_KEYS        ';'-separated PowerShell registry paths of the native-messaging host (Chrome, Edge)
#   LP_APPPATH_KEY    the registry key whose AppPath value tells the provider where the app is
#   LP_PROVIDER_DATA  %LOCALAPPDATA%\LocalPass\provider (the provider's pairing secret and log)
#   LP_DELETE_DATA    1 when the user ticked "delete app data"
#
# Exit code: 0 = done, or nothing of ours to undo; 3 = the Windows passkey package is still registered
# (unregister failed), so the uninstaller tells the user how to finish. Nothing here may block an uninstall.

$ErrorActionPreference = 'Stop'

# Two spellings of one folder (case, trailing backslash, "..") must compare equal.
function Normalize([string]$path) {
    if (-not $path) { return '' }
    try { return [IO.Path]::GetFullPath($path).TrimEnd('\') } catch { return $path.TrimEnd('\') }
}

# 1. The browsers' registration. The key's default value is the host manifest, which lives in the app's
#    data folder and is shared with other builds of LocalPass (a development build), so a key is removed
#    only when its manifest names THIS install's exe. A missing or unreadable manifest leaves it alone.
foreach ($key in ($env:LP_NM_KEYS -split ';' | Where-Object { $_ })) {
    try {
        if (-not (Test-Path -LiteralPath $key)) { continue }
        $manifest = (Get-Item -LiteralPath $key).GetValue('')
        if (-not $manifest -or -not (Test-Path -LiteralPath $manifest -PathType Leaf)) { continue }
        $exe = (Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json).path
        if ($exe -and (Normalize $exe) -eq (Normalize $env:LP_EXE)) {
            Remove-Item -LiteralPath $key -Recurse -Force
        }
    } catch {}
}

# 2. The Windows passkey provider. Only a package registered from THIS install folder is touched.
#    `unregister` takes LocalPass out of Windows' list and only works while the package exists, so it
#    must come first; if it fails the package is left registered (removing it would orphan the entry in
#    Windows' list, which only a reinstall plus Settings > Remove can clear).
$code = 0
try {
    $package = Get-AppxPackage -Name LocalPass.Passkeys
    if ($package -and (Normalize $package.InstallLocation) -eq (Normalize $env:LP_DIR)) {
        if (-not (Test-Path -LiteralPath $env:LP_ALIAS -PathType Leaf)) { throw 'the provider alias is missing' }
        & $env:LP_ALIAS unregister
        if ($LASTEXITCODE -ne 0) { throw 'unregister failed' }
        # A provider process of this folder keeps its exe locked; one of another copy is none of our business.
        $folder = (Normalize $env:LP_DIR) + '\'
        Get-Process -Name lp-winauth-provider -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and (Normalize $_.Path).StartsWith($folder, [StringComparison]::OrdinalIgnoreCase) } |
            Stop-Process -Force -ErrorAction SilentlyContinue
        $package | Remove-AppxPackage
        Remove-ItemProperty -LiteralPath $env:LP_APPPATH_KEY -Name AppPath -ErrorAction SilentlyContinue
    }
} catch {
    $code = 3
}

# 3. The provider's own data (the DPAPI pairing secret and its log, which names websites).
if ($env:LP_DELETE_DATA -eq '1' -and $env:LP_PROVIDER_DATA -and
    (Split-Path -Leaf (Normalize $env:LP_PROVIDER_DATA)) -eq 'provider') {
    Remove-Item -LiteralPath $env:LP_PROVIDER_DATA -Recurse -Force -ErrorAction SilentlyContinue
}

exit $code

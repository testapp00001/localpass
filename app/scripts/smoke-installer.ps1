# Smoke test of the NSIS installer on a PC that already has its owner's own LocalPass state (data folders,
# browser registration, a registered Windows passkey provider, an autostart entry). It installs silently into a
# throwaway folder, starts the installed app once under a throwaway profile, uninstalls silently, and proves the
# owner's state is exactly what it was. Nothing here ticks "delete app data".
#
#   powershell -File app/scripts/smoke-installer.ps1 -Installer <path to LocalPass_x.y.z_x64-setup.exe>
#
# What it protects (compared before and after, and restored if the stock uninstaller removed it):
#   * HKCU Run value "LocalPass" (the stock template deletes it on every real uninstall: restored here),
#   * the Chrome and Edge native-messaging keys, HKCU\Software\LocalPass\Provider\AppPath,
#   * the registered LocalPass.Passkeys package, %LOCALAPPDATA%\LocalPass\provider, the app's data folders,
#   * the owner's Start menu / desktop LocalPass shortcuts (backed up, restored).
# It refuses to run if an installed copy is registered (that would be upgraded, not tested) and starts the app
# with LOCALPASS_PROFILE and a throwaway WebView2 folder so the owner's profile and web view data are untouched.
#
# To prove the uninstall hook really ran with the right inputs, the installed uninstall-cleanup.ps1 is replaced
# by a wrapper that records the LP_* variables and the exit code of the real script (kept next to it), then
# runs the real script. The real script finds nothing of this install to undo (the owner's browser keys name
# another exe, the owner's package lives in another folder), which is itself part of the check.
#
# Exit code 0 when every check passes.
param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [int]$LaunchSeconds = 10
)

$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path -LiteralPath $Installer).Path
$results = [System.Collections.Generic.List[object]]::new()
function Check([string]$name, [bool]$ok, [string]$detail = '') {
    $results.Add([pscustomobject]@{ Check = $name; Ok = $ok; Detail = $detail })
    Write-Host ("{0,-4} {1}{2}" -f ($(if ($ok) { 'ok' } else { 'FAIL' })), $name, $(if ($detail) { "  [$detail]" } else { '' }))
}

$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\LocalPass'
$nmKeys = @(
    'HKCU:\Software\Google\Chrome\NativeMessagingHosts\app.localpass.host',
    'HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\app.localpass.host'
)
$providerKey = 'HKCU:\Software\LocalPass\Provider'
$installKey = 'HKCU:\Software\localpass\LocalPass'   # written by the installer (case-insensitive: next to \Provider)
$providerData = Join-Path $env:LOCALAPPDATA 'LocalPass\provider'
$dataDirs = @((Join-Path $env:APPDATA 'app.localpass'), (Join-Path $env:LOCALAPPDATA 'app.localpass'))
$shortcuts = @(
    (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\LocalPass.lnk'),
    (Join-Path ([Environment]::GetFolderPath('Desktop')) 'LocalPass.lnk')
)

function Default-Value([string]$key) {
    if (-not (Test-Path -LiteralPath $key)) { return $null }
    return (Get-Item -LiteralPath $key).GetValue('')
}

function Snapshot {
    $files = foreach ($dir in $dataDirs) {
        if (Test-Path -LiteralPath $dir) {
            # names and sizes only, and not the throwaway profile or web view folders of this test
            Get-ChildItem -LiteralPath $dir -Recurse -File -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -notlike '*\profiles\smoke-*' } |
                ForEach-Object { $_.FullName.Substring($dir.Length) }
        }
    }
    $provider = if (Test-Path -LiteralPath $providerData) {
        Get-ChildItem -LiteralPath $providerData -Recurse -File -ErrorAction SilentlyContinue |
            ForEach-Object { "{0}:{1}" -f $_.Name, (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
    }
    [ordered]@{
        Run          = (Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue).LocalPass
        Chrome       = Default-Value $nmKeys[0]
        Edge         = Default-Value $nmKeys[1]
        AppPath      = (Get-ItemProperty -LiteralPath $providerKey -ErrorAction SilentlyContinue).AppPath
        ProviderKey  = (Test-Path -LiteralPath $providerKey)
        Package      = (Get-AppxPackage -Name LocalPass.Passkeys -ErrorAction SilentlyContinue).InstallLocation
        ProviderData = @($provider | Sort-Object)
        AppFiles     = @($files | Sort-Object)
        Shortcuts    = @($shortcuts | ForEach-Object { Test-Path -LiteralPath $_ })
    }
}
function Same($a, $b) { ($a | ConvertTo-Json -Depth 6 -Compress) -ceq ($b | ConvertTo-Json -Depth 6 -Compress) }
# Which snapshot fields differ, and for lists what was added (+) or removed (-): the detail of a failed check.
function Show-Diff($a, $b) {
    $out = foreach ($k in $a.Keys) {
        if (Same $a[$k] $b[$k]) { continue }
        if ($a[$k] -is [array] -and $b[$k] -is [array]) {
            $plus = @($b[$k] | Where-Object { $a[$k] -notcontains $_ }) | Select-Object -First 6
            $minus = @($a[$k] | Where-Object { $b[$k] -notcontains $_ }) | Select-Object -First 6
            "$k +[$($plus -join '; ')] -[$($minus -join '; ')]"
        } else { "$k '$($a[$k])' -> '$($b[$k])'" }
    }
    $text = ($out -join ' | ')
    if (-not $text) { $text = 'no field differs; before=' + ($a | ConvertTo-Json -Depth 6 -Compress) + ' after=' + ($b | ConvertTo-Json -Depth 6 -Compress) }
    if ($text.Length -gt 900) { $text.Substring(0, 900) + '...' } else { $text }
}

# ---- preconditions -----------------------------------------------------------------------------------------
if (Test-Path -LiteralPath $uninstallKey) { throw "an installed LocalPass is registered ($uninstallKey): this test would upgrade it. Stop." }
$work = Join-Path $env:TEMP ("lp-smoke-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$dir = Join-Path $work 'LocalPass'
$profileName = 'smoke-' + [Guid]::NewGuid().ToString('N').Substring(0, 6)
New-Item -ItemType Directory -Path $work | Out-Null
$before = Snapshot
$runBefore = $before.Run
$linkBackup = @{}
foreach ($l in $shortcuts) { if (Test-Path -LiteralPath $l) { $backup = Join-Path $work ([IO.Path]::GetFileName((Split-Path $l -Parent)) + '-' + [IO.Path]::GetFileName($l)); Copy-Item -LiteralPath $l -Destination $backup; $linkBackup[$l] = $backup } }
Write-Host "work folder: $work`nprofile: $profileName`nbefore: Run=$([bool]$before.Run) chrome=$([bool]$before.Chrome) edge=$([bool]$before.Edge) package=$($before.Package)"

$app = $null
try {
    # ---- install ---------------------------------------------------------------------------------------------
    $install = Start-Process -FilePath $Installer -ArgumentList @('/S', "/D=$dir") -Wait -PassThru
    Check 'installer exits 0' ($install.ExitCode -eq 0) "exit $($install.ExitCode)"
    foreach ($f in 'localpass.exe', 'uninstall.exe', 'uninstall-cleanup.ps1', 'extension\manifest.json', 'extension\background.js', 'windows-passkeys\lp-winauth-provider.exe', 'windows-passkeys\AppxManifest.xml') {
        Check "installed: $f" (Test-Path -LiteralPath (Join-Path $dir $f))
    }
    $uninstallProps = Get-ItemProperty -LiteralPath $uninstallKey -ErrorAction SilentlyContinue
    Check 'uninstall entry registered' ($null -ne $uninstallProps) "$($uninstallProps.DisplayVersion)"
    $manifest = Get-Content -LiteralPath (Join-Path $dir 'extension\manifest.json') -Raw | ConvertFrom-Json
    Check 'extension version equals the app version' ($manifest.version -eq $uninstallProps.DisplayVersion) "extension $($manifest.version), app $($uninstallProps.DisplayVersion)"
    $exeVersion = (Get-Item -LiteralPath (Join-Path $dir 'localpass.exe')).VersionInfo.ProductVersion
    Check 'exe version equals the app version' ($exeVersion -like "$($uninstallProps.DisplayVersion)*") $exeVersion

    # ---- start the installed app once, in a throwaway profile ------------------------------------------------
    $env:LOCALPASS_PROFILE = $profileName
    $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $work 'webview2'
    $app = Start-Process -FilePath (Join-Path $dir 'localpass.exe') -PassThru
    Remove-Item Env:LOCALPASS_PROFILE, Env:WEBVIEW2_USER_DATA_FOLDER
    Start-Sleep -Seconds $LaunchSeconds
    Check 'installed app is still running after start-up' (-not $app.HasExited) "after $LaunchSeconds s"
    $afterLaunch = Snapshot
    # the installer's own Start menu and desktop shortcuts exist at this point: the uninstall check below covers them
    $withoutLinks = { param($s) $c = [ordered]@{}; foreach ($k in $s.Keys) { if ($k -ne 'Shortcuts') { $c[$k] = $s[$k] } }; $c }
    $b0 = & $withoutLinks $before; $b1 = & $withoutLinks $afterLaunch
    Check 'start-up changed none of the owner state' (Same $b0 $b1) (Show-Diff $b0 $b1)
    if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force; $app.WaitForExit(10000) | Out-Null }
    Start-Sleep -Seconds 2

    # ---- wrap the cleanup script so the hook's inputs and result can be read ---------------------------------
    $script = Join-Path $dir 'uninstall-cleanup.ps1'
    $real = Join-Path $dir 'uninstall-cleanup.real.ps1'
    $marker = Join-Path $dir 'hook-marker.txt'
    Move-Item -LiteralPath $script -Destination $real
    @'
$marker = Join-Path $PSScriptRoot 'hook-marker.txt'
Get-ChildItem Env:LP_* | ForEach-Object { "$($_.Name)=$($_.Value)" } | Set-Content -LiteralPath $marker
& (Join-Path $PSScriptRoot 'uninstall-cleanup.real.ps1')
$code = $LASTEXITCODE
"EXIT=$code" | Add-Content -LiteralPath $marker
exit $code
'@ | Set-Content -LiteralPath $script -Encoding UTF8

    # ---- uninstall (in place, so that we can wait for it) ----------------------------------------------------
    $un = Start-Process -FilePath (Join-Path $dir 'uninstall.exe') -ArgumentList @('/S', "_?=$dir") -Wait -PassThru
    Check 'uninstaller exits 0' ($un.ExitCode -eq 0) "exit $($un.ExitCode)"
    $lines = if (Test-Path -LiteralPath $marker) { Get-Content -LiteralPath $marker } else { @() }
    $env_ = @{}
    foreach ($l in $lines) { $k, $v = $l -split '=', 2; $env_[$k] = $v }
    Check 'the uninstall hook ran the cleanup script' ($lines.Count -gt 0)
    Check 'hook: LP_DIR is the installed windows-passkeys folder' ($env_['LP_DIR'] -eq (Join-Path $dir 'windows-passkeys')) $env_['LP_DIR']
    Check 'hook: LP_EXE is the installed exe' ($env_['LP_EXE'] -eq (Join-Path $dir 'localpass.exe')) $env_['LP_EXE']
    Check 'hook: LP_DELETE_DATA is 0 in a silent uninstall' ($env_['LP_DELETE_DATA'] -eq '0') $env_['LP_DELETE_DATA']
    Check 'hook: LP_PROVIDER_DATA is the provider data folder' ($env_['LP_PROVIDER_DATA'] -eq $providerData) $env_['LP_PROVIDER_DATA']
    Check 'hook: LP_ALIAS is the WindowsApps alias' ($env_['LP_ALIAS'] -like '*\Microsoft\WindowsApps\lp-winauth-provider.exe') $env_['LP_ALIAS']
    Check 'hook: LP_NM_KEYS names the Chrome and Edge keys' ($env_['LP_NM_KEYS'] -eq ($nmKeys -join ';')) $env_['LP_NM_KEYS']
    Check 'hook: LP_APPPATH_KEY is the provider key' ($env_['LP_APPPATH_KEY'] -eq $providerKey) $env_['LP_APPPATH_KEY']
    Check 'hook: the real script exited 0 (nothing of this install to undo)' ($lines -contains 'EXIT=0') (($lines | Select-Object -Last 1))
    foreach ($f in 'localpass.exe', 'extension\manifest.json', 'windows-passkeys\lp-winauth-provider.exe') {
        Check "uninstalled: $f is gone" (-not (Test-Path -LiteralPath (Join-Path $dir $f)))
    }
    Check 'uninstall entry removed' (-not (Test-Path -LiteralPath $uninstallKey))

    # ---- the owner's state ------------------------------------------------------------------------------------
    $after = Snapshot
    foreach ($name in 'Chrome', 'Edge', 'AppPath', 'ProviderKey', 'Package', 'ProviderData', 'AppFiles', 'Shortcuts') {
        Check "owner state unchanged: $name" (Same $before[$name] $after[$name])
    }
    $runNow = $after.Run
    if ($runBefore -and -not $runNow) {
        Write-Host "note: the stock uninstaller deleted the Run value 'LocalPass' (expected); restoring it"
    } else {
        Check 'owner state unchanged: Run' (Same $runBefore $runNow)
    }
}
finally {
    # restore whatever the stock uninstaller removes and clean up everything this test created
    if ($app -and -not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item Env:LOCALPASS_PROFILE, Env:WEBVIEW2_USER_DATA_FOLDER -ErrorAction SilentlyContinue
    if ($runBefore) {
        if (-not (Test-Path -LiteralPath $runKey)) { New-Item -Path $runKey -Force | Out-Null }
        Set-ItemProperty -LiteralPath $runKey -Name LocalPass -Value $runBefore
    }
    if (Test-Path -LiteralPath $installKey) { Remove-Item -LiteralPath $installKey -Recurse -Force }   # only our subkey, never \Provider
    if (Test-Path -LiteralPath $uninstallKey) { Remove-Item -LiteralPath $uninstallKey -Recurse -Force }
    foreach ($l in $linkBackup.Keys) { if (-not (Test-Path -LiteralPath $l)) { Copy-Item -LiteralPath $linkBackup[$l] -Destination $l } }
    foreach ($d in $dataDirs) { $p = Join-Path $d "profiles\$profileName"; if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Recurse -Force } }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

$final = Snapshot
Check 'final: all owner state is back to what it was (Run included)' (Same $before $final)
$failed = @($results | Where-Object { -not $_.Ok })
Write-Host ("`n{0} checks, {1} failed" -f $results.Count, $failed.Count)
exit $(if ($failed.Count -eq 0) { 0 } else { 1 })

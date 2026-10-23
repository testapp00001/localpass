; Installer hooks for LocalPass (Tauri: bundle > windows > nsis > installerHooks, set in tauri.windows.conf.json).
;
; Settings > Windows passkeys registers a small package (LocalPass.Passkeys) from the install folder's
; "windows-passkeys" subfolder and adds LocalPass to Windows' list of passkey providers. Uninstalling must
; undo both BEFORE the files go: the provider can only take itself out of Windows' list through its
; execution alias, which exists only while the package is registered. Failures are ignored (an uninstall
; must never be blocked), and nothing is touched unless the package was registered from THIS install
; folder, so uninstalling one copy cannot break a registration that belongs to another.
;
; $$ is a literal $ in NSIS. The two paths travel in environment variables, so no path is ever spliced into
; the PowerShell text. Notes: docs/windows-passkey-provider-notes.md, "Phase 4.5".

!macro NSIS_HOOK_PREINSTALL
  ; A provider process left over from the previous version (Windows starts it on demand and it exits when
  ; idle) keeps its exe locked, which would make the installer ask to retry.
  ${DisableX64FSRedirection}
  nsExec::Exec /TIMEOUT=15000 '"$SYSDIR\taskkill.exe" /F /IM lp-winauth-provider.exe'
  Pop $0
  ${EnableX64FSRedirection}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; /UPDATE is an in-place update by an updater: the registration stays. (Choosing "uninstall before
  ; installing" on the installer's reinstall page does remove it; the default there keeps it.)
  ${If} $UpdateMode <> 1
  System::Call 'Kernel32::SetEnvironmentVariable(t "LP_DIR", t "$INSTDIR\windows-passkeys")'
  System::Call 'Kernel32::SetEnvironmentVariable(t "LP_ALIAS", t "$LOCALAPPDATA\Microsoft\WindowsApps\lp-winauth-provider.exe")'
  ${DisableX64FSRedirection}
  nsExec::Exec /TIMEOUT=90000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -Command "try { $$p = Get-AppxPackage -Name LocalPass.Passkeys; if ($$p -and ($$p.InstallLocation -eq $$env:LP_DIR)) { try { & $$env:LP_ALIAS unregister } catch {}; Get-Process -Name lp-winauth-provider -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; $$p | Remove-AppxPackage; Remove-ItemProperty -Path HKCU:\Software\LocalPass\Provider -Name AppPath -ErrorAction SilentlyContinue } } catch {}"'
  Pop $0
  ${EnableX64FSRedirection}
  ${EndIf}
!macroend

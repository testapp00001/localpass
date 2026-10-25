; Installer hooks for LocalPass (Tauri: bundle > windows > nsis > installerHooks, set in tauri.windows.conf.json).
;
; Settings > Browser extension writes native-messaging keys into HKCU, and Settings > Windows passkeys
; registers a small package (LocalPass.Passkeys) from the install folder's "windows-passkeys" subfolder,
; adds LocalPass to Windows' list of passkey providers and keeps a pairing secret in
; %LOCALAPPDATA%\LocalPass\provider. Uninstalling must undo all of it BEFORE the files go: the provider can
; only take itself out of Windows' list through its execution alias, which exists only while the package
; is registered. The work is done by uninstall-cleanup.ps1 (shipped in the install folder, see
; tauri.windows.conf.json), which says what it touches and when it refuses; a test runs it against throwaway
; registry keys and folders (app/test/uninstall-cleanup.test.ts). An uninstall is never blocked: when the
; package cannot be unregistered the script leaves it and exits 3, and the user is told how to finish.
; Nothing is touched unless it belongs to THIS install folder, so uninstalling one copy cannot break a
; registration that belongs to another.
;
; $$ is a literal $ in NSIS. The paths travel in environment variables, so no path is ever spliced into
; the PowerShell command line. Notes: docs/windows-passkey-provider-notes.md, "Phase 4.5".

!macro NSIS_HOOK_PREINSTALL
  ; A provider process left over from the previous version (Windows starts it on demand and it exits when
  ; idle) keeps its exe locked, which would make the installer ask to retry.
  ${DisableX64FSRedirection}
  nsExec::Exec /TIMEOUT=15000 '"$SYSDIR\taskkill.exe" /F /IM lp-winauth-provider.exe'
  Pop $0
  ${EnableX64FSRedirection}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; /UPDATE is an in-place update by an updater: the registrations stay. A normal user-run upgrade is NOT that:
  ; the installer's reinstall page preselects "Uninstall before installing", which runs the OLD uninstaller as
  ; "uninstall.exe _?=<dir>" (no /UPDATE), so the whole block below runs and the browser keys, the Windows passkeys
  ; registration and (in the stock template) the Run value go. Choosing "Do not uninstall" keeps them (the README
  ; says so). Tauri has no hook before that page; changing it needs a custom NSIS template (open follow-up in
  ; docs/handoff-phase4-5.md).
  ${If} $UpdateMode <> 1
  ${AndIf} ${FileExists} "$INSTDIR\uninstall-cleanup.ps1"
    System::Call 'Kernel32::SetEnvironmentVariable(t "LP_DIR", t "$INSTDIR\windows-passkeys")'
    System::Call 'Kernel32::SetEnvironmentVariable(t "LP_ALIAS", t "$LOCALAPPDATA\Microsoft\WindowsApps\lp-winauth-provider.exe")'
    System::Call 'Kernel32::SetEnvironmentVariable(t "LP_EXE", t "$INSTDIR\${MAINBINARYNAME}.exe")'
    System::Call 'Kernel32::SetEnvironmentVariable(t "LP_NM_KEYS", t "HKCU:\Software\Google\Chrome\NativeMessagingHosts\app.localpass.host;HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\app.localpass.host")'
    System::Call 'Kernel32::SetEnvironmentVariable(t "LP_APPPATH_KEY", t "HKCU:\Software\LocalPass\Provider")'
    System::Call 'Kernel32::SetEnvironmentVariable(t "LP_PROVIDER_DATA", t "$LOCALAPPDATA\LocalPass\provider")'
    ; The stock "delete app data" box (only asked in the interactive uninstaller) also covers the provider's data.
    ${If} $DeleteAppDataCheckboxState = 1
      System::Call 'Kernel32::SetEnvironmentVariable(t "LP_DELETE_DATA", t "1")'
    ${Else}
      System::Call 'Kernel32::SetEnvironmentVariable(t "LP_DELETE_DATA", t "0")'
    ${EndIf}
    ${DisableX64FSRedirection}
    nsExec::Exec /TIMEOUT=90000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\uninstall-cleanup.ps1"'
    Pop $0
    ${EnableX64FSRedirection}
    ${If} $0 == "3"
    ${AndIfNot} ${Silent}
      MessageBox MB_OK|MB_ICONEXCLAMATION "LocalPass could not take itself out of Windows' list of passkey providers, so an entry for it stays under Settings > Accounts > Passkeys.$\r$\n$\r$\nTo remove it: install LocalPass again, open Settings > Windows passkeys in LocalPass, choose Remove, then uninstall."
    ${EndIf}
  ${EndIf}
!macroend

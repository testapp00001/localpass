This folder is filled by `bun scripts/stage-provider.ts` (the last step of `bun run build:all`, which
`tauri build` runs): the Windows passkey provider exe, its AppxManifest.xml and its logos. Everything
here except this file is generated and git-ignored. The Windows-only Tauri config
(src-tauri/tauri.windows.conf.json) ships the folder as "windows-passkeys" next to localpass.exe; Settings >
Windows passkeys registers the package from there. This file exists so that builds that did not run the
script (cargo test, clippy, tauri dev) still find the folder.

# Changelog

All notable changes to LocalPass. Generated from commit messages (Conventional Commits) by git-cliff.

## 0.3.0 - 2026-11-15

### Added
- **extension:** Firefox decision window closes with Esc and on a click outside
- **app:** Secure notes stay hidden until shown
- **app:** Read a 2FA setup QR from a picture on desktop
- Find duplicate logins and move the extra copies to trash
- Use Firefox toolbar picker and retire file storage
- Unify browser panels and improve Linux integration


### Documentation
- Phase 7 plan, handoff and the agent and plan-history rules
- Record the Windows checks of 7.1 and 7.4 and set up passkeys at the PC itself


### Fixed
- **ci:** Locate broker helper with newer Cargo layouts
- **ci:** Isolate extension test files
- **ci:** Support Windows helper paths and deadline assertions


### Maintenance
- Faster Rust builds, no debug info for dependencies and only the shipped Android ABIs
- Link with rust-lld.exe on Windows


### Other
- Merge pull request #1 from testapp00001/phase7

Bring Phase 7 vault tools and Phase 8 browser experience into main


## 0.2.2 - 2026-10-31

### Added
- Run LocalPass on Fedora Linux
- **extension:** Firefox build, CI and release jobs for Fedora


### Documentation
- Note the v0.2.1 draft release in the Phase 6 handoff


### Fixed
- **app:** Keep the master-password check code off Windows and Android where unused
- **app:** Follow the app theme in native controls, so dark selects are readable on Linux


### Maintenance
- Use the sdkmanager name of the Android 37 platform


## 0.2.0 - 2026-10-25

### Maintenance
- Release 0.2.0 builds, uninstall cleanup and installer smoke test (Phase 5.4-5.5)
- GitHub Actions CI and release workflow, changelog and Releases link
- Check files out with LF line endings
- Run every test binary and print the runner OS version
- Run the passkey-plugin tests only where Windows implements the API
- Put each changelog entry on its own line


### Other
- Shared core, sync, extension, passkeys, Android and Windows providers, Phase 5 hardening



# LocalPass

A password manager with **no cloud**. Your vault lives only on your devices and
syncs directly between them whenever they share a network: home Wi-Fi, office
LAN, or a phone hotspot.

Typical flow: create the vault on your phone, pair your home laptop, later pair
your work laptop at the office. When the work laptop comes home it syncs with
the home laptop directly, even if the phone is off. Changes hop between
devices, so they all converge.

## Layout

| Path | What it is |
|---|---|
| `crates/lp-core` | Crypto, signed op format, CRDT materializer, device roster, password generator, TOTP |
| `crates/lp-store` | SQLite storage: op-log, metadata, peer address book |
| `crates/lp-sync` | Noise transport, sync protocol, SPAKE2 pairing, mDNS discovery, background service |
| `crates/lp-engine` | The API every front end uses: vault, items, pairing, revocation, import/export, sync lifecycle |
| `crates/lp-import` | Bitwarden JSON/CSV, KeePass `.kdbx`, browser CSV import; Bitwarden-format export |
| `crates/lp-broker` | The local pipe between the app and the browser extension or the Windows passkey provider: pairing, requests, the native-messaging helper |
| `crates/lp-winauth` | Windows passkey provider: the COM server Windows starts, which relays requests to the app through the broker |
| `crates/lp-cli` | `lp`: headless sync peer and test harness |
| `app/` | Desktop app: Tauri 2 + React + TypeScript + Tailwind (also builds the Android app) |
| `extension/` | Chrome and Edge extension (TypeScript, built with Bun): autofill, saving logins, passkeys |

## Running

Prerequisites: Rust (stable), [bun](https://bun.sh), and WebView2 (preinstalled on Windows 11).

```sh
cd app
bun install
bun run tauri dev           # desktop app with hot reload
```

To run a second instance on the same PC (for example, to try pairing), build
once and start it with its own profile:

```sh
cargo build -p localpass --features custom-protocol
set LOCALPASS_PROFILE=second && target\debug\localpass.exe
```

Release installer (NSIS, per-user): `cd app && bun run tauri build`.

### Android

You need Android Studio (its SDK and bundled JDK), the NDK (r27), and the Rust
Android targets (listed in `rust-toolchain.toml`). On Windows, also turn on
**Developer Mode** (Settings → System → For developers): the Tauri CLI links the
compiled library into the Gradle project with a symlink, which Windows only
allows with it.

```sh
set JAVA_HOME=C:\Program Files\Android\Android Studio\jbr
set ANDROID_HOME=%LOCALAPPDATA%\Android\Sdk
set NDK_HOME=%ANDROID_HOME%\ndk\27.3.13750724
cd app
bun run tauri android dev                                   # run on a phone connected by USB
bun run tauri android build --debug --target aarch64 --apk  # installable debug APK
```

Test on a real phone with USB debugging on. The Android emulator sits behind
its own NAT and can't see your laptop's network, so pairing and sync can't be
tried there.

To install the signed release APK on a phone and turn on autofill and passkeys, follow
[docs/android-setup.md](docs/android-setup.md).

### Release builds

```powershell
cd app
bun run tauri build                                   # Windows: target\release\bundle\nsis\LocalPass_<version>_x64-setup.exe
bun run tauri android build --target aarch64 --apk    # phone: app-universal-release.apk (add x86_64 to also run on the emulator)
adb install -r src-tauri\gen\android\app\build\outputs\apk\universal\release\app-universal-release.apk
```

Both take 10 to 20 minutes the first time (full optimisation). The Android release APK has to be
signed before a phone will install it. Gradle signs it when
`app/src-tauri/gen/android/keystore.properties` exists (git-ignored; keys `password`, `keyAlias`,
`storeFile`). Create the key once with `keytool -genkeypair -keystore <file>.jks -alias localpass
-keyalg RSA -keysize 4096 -validity 10000`, and keep it: an update only installs over an app
signed with the same key. Release builds don't shrink code with R8, which crashed at startup on
Android 17 and isn't worth the risk for this reflection-heavy stack.

### Installing and updating

LocalPass has no updater and never connects to the internet to look for one (by design). Releases are
published on the [GitHub release page](https://github.com/testapp00001/localpass/releases); Settings has a
**Releases** link that opens it in your browser. Looking for a new version is up to you. A release is
four files: the Windows installer (`LocalPass_<version>_x64-setup.exe`), the Android APK
(`localpass-<version>-universal-release.apk`, for 64-bit ARM phones and x86_64 emulators), the browser
extension (`localpass-extension-<version>.zip`) and `SHA256SUMS.txt`. The app and the extension share one
version number; update both together. Download the files (or copy them to each device by USB stick or shared
folder) and check them first:

```powershell
(Get-FileHash .\LocalPass_<version>_x64-setup.exe -Algorithm SHA256).Hash.ToLower()    # must equal the line in SHA256SUMS.txt
```

**Windows.** The installer is not signed with a code-signing certificate. If it came through a browser
download, Windows SmartScreen says "Windows protected your PC": choose **More info → Run anyway**. Files
copied by USB stick or from a shared folder usually show no warning. To update, quit LocalPass (tray icon → Quit) and run the new installer. It notices the installed version and
asks how to install; **choose "Do not uninstall"** (the second option, not the preselected "Uninstall before
installing"). The files are then replaced in place and everything stays: your vault and settings (they live in your
user profile, not in the install folder), the browser registration, the Windows passkeys registration and
start at login. With the preselected choice the installer first runs the old uninstaller, which removes the
browser registration (Settings → Browser extension → Set up again), the Windows passkeys registration (Settings →
Windows passkeys → Set up, turn the Windows switch on, Pair) and the start-at-login entry; the vault is not
touched. After the update, reload the extension: open `chrome://extensions` (or `edge://extensions`) and click
**Reload** on LocalPass; the extension you loaded from Settings → Browser extension → Open is the folder inside the
install folder, which the installer has just replaced. Keep the install folder the same, because the Windows passkey
provider and the browser registration point into it; if Windows passkeys stop working after an update, choose
**Set up again** in Settings → Windows passkeys.

**Android.** Copy the APK to the phone and open it; Android asks once to allow installing from that app.
To update, install the new APK over the old one. This works because every release is signed with the same
key and has a higher version code (the version code comes from the version number: 0.2.0 is 2000), and your
vault stays. If Android refuses with a signature conflict, the APK was signed with a different key: do not
uninstall the app to get past it, because that deletes the vault on that phone (it comes back by pairing and
syncing with another device only if another device still has it).

**Optional, Android:** [Obtainium](https://github.com/ImranR98/Obtainium) can watch the GitHub release page
above, tell you about new APKs and install them. It is a separate app that does the looking; LocalPass itself
stays offline.

**Checking where a file came from.** Every release file has a build attestation made by the release workflow:
`gh attestation verify <file> --repo testapp00001/localpass`.

### Making a release (maintainers)

Releases are built by GitHub Actions, not on a laptop. Commit messages follow
[Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `security:`, `docs:` ...), which is
what the changelog is generated from.

```powershell
bun scripts/release.ts 0.3.0     # bumps every version file, regenerates CHANGELOG.md, commits "chore(release): v0.3.0", tags v0.3.0
git push origin main v0.3.0      # the tag starts .github/workflows/release.yml
```

The workflow checks the tag against the version files, builds the installer and the extension zip on Windows and
the signed APK on Linux, adds `SHA256SUMS.txt` and build attestations, and creates a **draft** release with the
changelog section as its notes. Open the draft on GitHub, check the files, then press **Publish**. The Android
signing key is stored as the secrets `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD` and
`ANDROID_KEY_ALIAS` of the repository environment `release`; the build fails if the APK is signed with any key
other than the one pinned in the workflow (`ANDROID_CERT_SHA256`). `.github/workflows/ci.yml` runs formatting,
clippy and all tests on every push and pull request.

### CLI

```sh
cargo run -p lp-cli -- --help
lp --data-dir A init --name Phone                  # create a vault
lp --data-dir A invite                             # shows a pairing code
lp --data-dir B join 1234-5678 --name Laptop       # on another device/directory
lp --data-dir B serve                              # background sync until Ctrl+C
lp doctor                                          # what does this network allow?
```

### Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd app && bun run build
```

`crates/lp-engine/tests/scenario.rs` runs the phone → home → work story end to
end over real TCP. It also covers revocation, wrong pairing codes, sync while
locked, conflicts, and import/backup.

## Browser extension (Windows: Chrome and Edge)

The extension fills logins from your vault and offers to save new ones. It talks to the LocalPass
app on the same computer through Windows' native-messaging mechanism; nothing goes over a network.

1. Build it once: `cd extension && bun install && bun run build`.
2. In LocalPass open Settings → Browser extension and choose **Set up**. This registers LocalPass as
   the extension's helper for Chrome and Edge (per user, no administrator rights).
3. In the browser open `chrome://extensions` (or `edge://extensions`), turn on Developer mode, choose
   **Load unpacked**, and pick the `extension/dist` folder (the Settings page has an Open button).
4. Click the LocalPass icon in the toolbar and choose **Connect**. Approve the request in the app after
   checking that both show the same code.

Passwords are only sent to the page when you pick a login in the menu or popup, never when a page
loads, and only to the site the login was saved for. Logins for other sites, pages on plain http
(except localhost) and login forms inside other sites' frames are not filled. The popup can also turn off
the browser's own password saving. Disconnect a browser in Settings at any time: it loses access at once,
even if a page was already open. The in-page buttons stay inactive on pages with a `filter` or `opacity`
on `<html>`; use the popup there (details under Security notes and limits).

Two things always ask you first. Searching the whole vault from the popup, or filling a login saved for
a different website than the page, needs Windows Hello every time (the extension cannot do it
silently, even with the vault unlocked). In the popup, **Show all my logins** is one prompt (then you
filter the list in the popup; reopening the popup within two minutes keeps it), and filling a login
from that list asks again. If you say no, nothing is shown or filled. And when a page asks to save a password that would replace one
you already have (also one saved for another address of the same site, such as `example.com` and
`login.example.com`), or to create a passkey for an account that already has one, LocalPass shows a
**Replace?** question in its window; nothing is changed unless you choose Replace. A brand-new login
or passkey is saved without asking.

### Passkeys in the browser

On sites that support passkeys the extension offers to create one in your vault and to sign in with
it. A passkey is a vault item like any other, so it syncs to your other devices, and its private key
never leaves the LocalPass app: the extension only receives the signed response. Passkeys use the
ES256 algorithm. Security keys, other algorithms and sites a passkey can't be used on are left to
the browser's own dialog. By default Windows Hello confirms each passkey use (Settings → Browser
extension). The popup has a **Use LocalPass for passkeys** switch.

Not supported in the extension: Firefox. For other apps and browsers on Windows see the next section.

## Windows passkeys (any app or browser, Windows 11)

LocalPass can also appear in Windows' own passkey sheet, so any app or browser that uses Windows'
passkey support (not only Chrome and Edge with the extension) can save passkeys in LocalPass and sign
in with them. The private key still never leaves the LocalPass app: a small provider program that
Windows starts only relays the request, and Windows Hello confirms each use in LocalPass.

**Needs:** Windows 11 with build 26100.6725 or later (24H2 with the September 2025 update), **Developer
Mode** on (Settings → System → For developers; LocalPass is not signed with a certificate Windows trusts, so
Windows only installs the provider in that mode, and the settings page tells you when it is off) and
Windows Hello.

**Set up:** in LocalPass open Settings → **Windows passkeys** and follow the one button at a time:

1. **Set up** adds LocalPass to Windows' list of passkey providers (switched off).
2. **Turn on in Windows settings** opens Settings → Accounts → Passkeys → Advanced options. Switch
   LocalPass on and confirm with Windows Hello. Only you can do this step; an app cannot.
3. **Pair** connects Windows to the app once: confirm the 6-digit code in the LocalPass dialog.

If you also use the browser extension, turn off its **Use LocalPass for passkeys** switch (extension
popup), or the browser asks twice. The app starts (minimized) when Windows needs it and it is closed;
a locked vault is unlocked on the way. Passkeys that arrive from your phone or are deleted reach
Windows' list a few seconds after the change, while LocalPass is unlocked. **Remove** in the same
place takes LocalPass out of Windows' list again; uninstalling LocalPass does it too.

**Limits:** ES256 passkeys only. When several LocalPass passkeys exist for one website, Windows sign-in
may fail (there is no account picker yet); use the browser extension there. The provider belongs to
the folder LocalPass is installed in: if you move LocalPass or run another copy (a development build),
choose **Set up again** there, which turns the Windows switch off until you turn it on again.
What other programs running as you can read or change here is listed under Security notes and limits.

## Passkeys and autofill on Android

On Android 14 and later LocalPass can be the phone's passkey provider, and on any Android version
it can be the autofill service for passwords. Both work only in web browsers, for the website in
front, and only in browsers on Google's trusted-browser list (bundled with the app; nothing is
downloaded). Chrome is the one tested; the other listed browsers should work the same way.

Passkeys and autofill follow one rule: the browser says which website a page belongs to, and
LocalPass believes it only when the app in front is a browser whose package name and signing
certificate (SHA-256) are on that list. Any other app, even one that shows a web view claiming to
be a bank, gets no logins, no unlock prompt and no save offer; the screen that opens after you pick
a login checks this again from the screen the system hands it, not from what the request says.
What is left: the trusted browser reports the page's domain and LocalPass cannot check that
itself, so it trusts that browser to be right; a browser that is not on the list gets nothing;
native (non-browser) apps are not supported. LocalPass never fills on its own: it hands over a
password only after you pick that login.

1. Turn them on from LocalPass → Settings → **Autofill and passkeys**: its buttons open Android's
   own pages (**Preferred service**, **Passwords & passkeys**), where you choose LocalPass. Android
   does not let an app see whether it is a passkey provider, so that row shows no status. The
   section also shows whether the Android 17 local-network permission is allowed. Step by step:
   [docs/android-setup.md](docs/android-setup.md).
2. For passwords in Chrome, also open Chrome → Settings → **Autofill services** and choose
   **Autofill using another service**.
3. A passkey is created or used after a fingerprint or screen-lock check. If the vault is locked,
   LocalPass first asks for the fingerprint (or the master password) in the passkey list, and that
   one check covers the passkey you pick from that list, once; an unlock done for autofill or for
   saving a password never counts for a passkey. **Passkeys need a screen lock** (PIN, pattern,
   password or fingerprint): on a phone without one LocalPass shows "Set a screen lock to use
   passkeys" and refuses, instead of making a passkey that claims the owner was checked; Settings
   shows the same warning. Passwords and autofill do not need it. If a site asks to make a passkey
   for an account that already has one in LocalPass, LocalPass asks "Replace your passkey?" before
   replacing it. The prompts name the website (the browser's, checked by LocalPass) first; the names
   a site gives itself are shortened. Passwords are only handed to the page after you pick that login.

4. After you sign in to a website with a login LocalPass does not have, or with a changed password,
   Android asks "Save to LocalPass?". Tap **Save** and the login is stored in the vault (a new item
   named after the site, or the password of the existing login for that username updated; the
   same password again changes nothing). Backing out of the prompt saves nothing. If the vault is
   locked, LocalPass opens its unlock screen (fingerprint, else master password) first and saves
   after that; if you back out of it, nothing is saved. On Android 8 to 8.1, which cannot open a
   screen from the prompt, a locked vault only shows a message and nothing is saved. Only forms
   with a username and a password field are offered, never a search box or a lone password field.

Passkeys and saved logins created on the phone sync to your other devices like any item. The
website for a saved login comes from the browser in front exactly as for filling, so only a
trusted browser can name it (see above). The password is only held in memory, and only while the vault is
locked and the unlock screen is open (at most two minutes); it is never written to a file, log or
notification.

## How it works

- **Op-log CRDT.** Each device appends signed ops to its own log. Sync swaps
  version vectors and sends the missing ops. Fields merge last-writer-wins by
  hybrid logical clock. Older values are kept as history, and concurrent edits
  are flagged so you can pick a version. Nothing is silently lost.
- **Keys.**
  - The vault key encrypts item data (XChaCha20-Poly1305). On each device it is
    wrapped by an Argon2id key derived from that device's master password.
  - Sync only moves ciphertext and checks Ed25519 signatures, so it runs while
    the vault is locked.
  - The network key and the device's Noise key are protected by a device secret
    (DPAPI on Windows).
- **Trust.** Every device carries a short signed proof that an existing member
  added it. A device that has never met another member can still prove it
  belongs to the vault.
- **Transport.** `Noise_XXpsk0` over TCP (default port 47821). Anyone without
  the vault's network key is rejected at the first message. Only private and
  link-local addresses are dialed.
- **Discovery.** mDNS with random names and hourly-rotating tags, so devices
  can't be tracked across networks. Remembered addresses and manual addresses
  are fallbacks.
- **Pairing.** An 8-digit code (or QR) drives SPAKE2: an attacker gets one
  online guess per attempt, and the inviter stops after 3 wrong tries. The new
  device must also be approved on the inviting device.

## Security notes and limits

- There is no account and no recovery: if you forget the master password and
  lose every device, the data is gone. Make encrypted backups (Settings →
  Encrypted backup). The encrypted backup is also the only export that carries
  your passkeys, private keys included, so keep it as safe as the vault. The
  plain JSON export (for moving to another password manager) leaves passkeys
  out: no other manager could use them, and a private key does not belong in an
  unencrypted file. Settings and `lp export --plain` tell you how many were left
  out.
- A thief who knows a device's master password can act as that device until
  you remove it (Devices → Remove). Other devices learn about the removal the
  next time they sync. Removing a device doesn't rotate the vault keys yet.
- LocalPass does not protect you from malware that already runs as your own
  Windows user. Such a program can read the memory of the unlocked app, log what
  you type, and read the browser extension's pairing secret (kept in the browser's
  profile folder) to talk to the app as if it were your paired browser. The
  approvals described in this file (confirming a pairing code, Windows Hello for
  logins saved for another site, **Replace?**) stop web pages and programs that
  were never paired. They do not stop a program that already has your user's
  access. Keep your PC free of untrusted software and your Windows account locked.
- Office and guest networks often block device-to-device traffic or multicast.
  Use "Connect by address" or a phone hotspot. `lp doctor` shows what is blocked.
- Windows Firewall asks to allow LocalPass the first time it listens. Allow it
  on private networks, or the other devices can't start a sync with this one.
- On Android, edits sync right away while the app is open. In the background,
  Android only lets LocalPass run about every 15 minutes (later while the phone
  is dozing), so changes from your laptop can take that long to reach a closed
  app. Opening the app syncs at once. The first time, Android 17 asks to allow
  access to devices on your local network; allow it.
- Scanning QR codes on Android (pairing, 2FA setup) uses the camera through
  Google's ML Kit and Play services. Recognition runs on the phone, but Play
  services may download its scanning model the first time, and phones without
  Play services can't scan. Everything else works without Google services; type
  the pairing code or the 2FA key instead.
- A device with a badly wrong clock can win edits it shouldn't. The app warns
  when a peer's clock is off by more than a minute.
- Pairing can be slowed down by someone else on the same network: a host there
  can keep one pairing attempt waiting for up to about 15 seconds at a time. That
  is a nuisance, not a way around the code (see Pairing above). If pairing hangs,
  cancel and try again, or pair over a phone hotspot.
- Import only files you trust. A KeePass `.kdbx` file states how much
  key-derivation work opening it needs, and LocalPass does not cap that yet, so a
  hostile file can make an import run for a very long time or use a lot of
  memory. If an import hangs, close LocalPass and do not open that file again.
- Matching logins to websites uses the registrable domain: a login saved for
  `example.com` is also offered on `login.example.com` and `shop.example.com`,
  never on `example.org` or `example.com.evil.test`. The match is https only
  (plain http only on localhost). A saved address without a port matches the
  site on any port; one saved with a port (`https://example.com:8443`) matches
  only that port. Saving a password that would replace one stored for another
  address of the same site asks first (the **Replace?** question).
- The browser extension's buttons on a page are inactive on pages that put a
  `filter` or `opacity` on the `<html>` element (Dark Reader's filter mode, some
  grayscale sites): Chrome then does not call them plainly visible, and LocalPass
  ignores clicks it cannot be sure you saw. That fails safe, and the toolbar popup
  still fills.
- **Windows: programs running as you.** LocalPass does not defend the Windows
  passkey provider against other programs running as you. What they can read or
  change there:
  - Windows keeps its own list of your LocalPass passkeys (website, user name
    and credential id, never a private key) so its sign-in sheet can show them.
    Any program running as you can read that list, also while the vault is
    locked.
  - The provider's files (in LocalPass's install folder), its command alias and
    the recorded location of the LocalPass app can be changed by any program
    running as you, and nothing checks them. Windows would run a replaced
    provider as the passkey provider and hand it the create and sign-in
    requests; the app would still ask Windows Hello for each use.
  - The provider's COM server (the part Windows starts) accepts calls from any
    program running as you. Create and sign-in requests carry Windows' signature
    and are checked; the cancel and lock-status calls are not signed. The
    provider's log file names the website of every request and its request id, so
    such a program can cancel a request in progress or ask whether the vault is
    unlocked.
- If LocalPass cannot create the local pipe that the extension and the Windows
  provider use, it records that once in its log and does not retry or show a
  message. From then on they cannot reach the app. Restart LocalPass.
- If Windows or the browser cancels a passkey request at the last moment (for
  example you close the sheet just as the passkey is being made), the passkey
  LocalPass already created stays in the vault although the website never got
  it. It is unused: delete that item.
- On Android, passkeys and autofill take the website from the browser in front
  (see "Passkeys and autofill on Android" for the trusted-browser rule). Autofill
  takes the first website the browser reports for the screen, not necessarily the
  one belonging to the frame that holds the password field. On a page that
  embeds another site's login form in a frame, a login saved for the outer site
  can therefore be offered for the framed form (the desktop extension refuses
  that). LocalPass hands over a password only after you pick that login, so look
  at which form you are filling before you pick.

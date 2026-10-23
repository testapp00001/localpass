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
| `crates/lp-cli` | `lp`: headless sync peer and test harness |
| `app/` | Desktop app: Tauri 2 + React + TypeScript + Tailwind |

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
set NDK_HOME=%ANDROID_HOME%
dk.3.13750724
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
even if a page was already open.

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
  Encrypted backup).
- A thief who knows a device's master password can act as that device until
  you remove it (Devices → Remove). Other devices learn about the removal the
  next time they sync. Removing a device doesn't rotate the vault keys yet.
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

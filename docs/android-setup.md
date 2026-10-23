# LocalPass on your Android phone

A checklist for putting the release APK on a real phone and turning everything on. Nothing here
needs an account or the internet: the app is one file, and your vault stays on your devices.

The app runs on Android 7 or later. Autofill needs Android 8, passkeys need Android 14. Syncing
needs the phone and your PC on the same Wi-Fi.

## 1. Install

1. Get the file `localpass-<version>-universal-release.apk` onto the phone: plug it in and copy it
   over, or use `adb install -r localpass-<version>-universal-release.apk`. Do not download it
   from anywhere else.
2. Optional check: the `.sha256` file next to it holds the file's SHA-256. On the PC,
   `certutil -hashfile localpass-<version>-universal-release.apk SHA256` must print the same value.
3. Open the file on the phone (Files app). Android asks to allow installs from this app: allow it
   once, install, then turn that permission off again.
4. If Android says "app not installed", an older copy signed with a different key is on the phone.
   Remove it first (see "Uninstall") and install again.

## 2. First start

1. Open LocalPass. When Android asks about the **local network** (Android 17), choose **Allow**.
   Without it, finding and syncing with your other devices does not work. You can turn it on later
   from LocalPass → Settings → Autofill and passkeys → Local network → Allow (then Permissions →
   Local network).
2. Pick one:
   - **Create a new vault** if this is your first device. Choose a long master password. It cannot
     be recovered: there is no account and no reset.
   - **I already use LocalPass on another device** to join your PC's vault. On the PC open
     Devices → Add device, then enter the 8-digit code (or scan the QR code) on the phone and
     approve it on the PC. Both must be on the same Wi-Fi.
3. In Settings, turn on **Unlock with fingerprint or face** if you want it. The master password
   always still works.

## 3. Turn on autofill and passkeys

Open LocalPass → **Settings** → **Autofill and passkeys**.

1. **Autofill passwords → Turn on.** Android shows its **Preferred service** list. Tap
   **LocalPass**, then **Change**. The row now says **On** when you come back.
2. **Passkeys → Open settings.** Android 14 or later only. Make sure LocalPass is the preferred
   service, or switched on under **Additional services**. On Android 17, choosing LocalPass as the
   preferred service in step 1 already does this. Android does not tell apps whether this is on,
   so the row never says On or Off: check the Android page itself.
3. **Chrome needs one more switch.** In Chrome: menu → Settings → **Autofill services** → choose
   **Autofill using another service**, then restart Chrome.

## 4. Try it on a real site

- **Save a password.** Sign in to a website in Chrome. Android asks **Save to LocalPass?**. Tap
  **Save**. If LocalPass is locked, it asks for your fingerprint or master password first, then
  saves.
- **Fill a password.** Open the site again and tap the username field. Pick the LocalPass entry,
  unlock if asked, and pick the login. Nothing is filled until you pick one.
- **Passkey.** Go to a site that supports passkeys (for example webauthn.io), create a passkey and
  choose LocalPass when Android asks where to save it. Signing in later asks for your fingerprint or
  screen lock. Passkeys need a screen lock on the phone: without one LocalPass says "Set a screen
  lock to use passkeys" and does nothing (Settings → Autofill and passkeys shows the same warning).
  If the site asks for a passkey for an account that already has one, LocalPass asks "Replace your
  passkey?" first.

## 5. Keeping it up to date

Install the newer `.apk` over the old one (step 1). Your vault and settings stay. It only works if
the new file was signed with the same key as the old one, so keep using files from the same builder.
Sync your devices before and after an update. Settings shows the installed version; each new file
needs a higher version than the one on the phone (Android refuses to install an older one over it).

Version 0.1.1 (the current file) fixes the camera screen for scanning QR codes (the 2FA setup code
when you edit a login, and the pairing code when joining a vault): its **Cancel** button and the
phone's **Back** button now close the camera instead of leaving you stuck on it. 0.1.0 does not
have this fix.

## 6. Uninstall

Settings → Apps → LocalPass → Uninstall. This deletes the vault **from this phone only**. If it is
your only device, first make an encrypted backup (LocalPass → Settings → Encrypted backup) and keep
it somewhere safe. Afterwards Android's autofill and passkey settings fall back to another service;
check Settings → Passwords, passkeys & accounts if something looks off.

## What it does not do

- Autofill and passkeys work **only in web browsers on Google's trusted-browser list** (Chrome has
  been tested; others such as Edge and Brave are on the list but untried). Other apps, and browsers
  not on the list, get nothing.
- LocalPass believes the website the browser reports. It cannot check that for itself.
- Passkeys use the ES256 algorithm only. Security keys and other algorithms fall back to the
  browser's own flow.
- Syncing needs both devices awake and on the same network. In the background Android lets
  LocalPass sync about every 15 minutes; opening the app syncs at once.
- The vault locks one minute after you leave the app (and after the inactivity time in Settings).
- There is no way to recover a forgotten master password.

// Typed wrappers around the Rust commands (see src-tauri/src/commands.rs).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type VaultStatus = "noVault" | "locked" | "unlocked";

export interface Settings {
  autoLockMinutes: number;
  clipboardClearSeconds: number;
  closeToTray: boolean;
  startAtLogin: boolean;
  theme: "system" | "light" | "dark";
  passkeyVerifyEveryTime: boolean;
}

export interface AppInfo {
  status: VaultStatus;
  deviceName: string | null;
  deviceId: string | null;
  suggestedDeviceName: string;
  platform: string;
  version: string;
  quickUnlock: { kind: "windowsHello" | "biometric" | "none"; available: boolean; enabled: boolean };
  settings: Settings;
}

export interface Strength {
  score: number;
  guessesLog10: number;
  crackTime: string;
  warning: string | null;
  suggestions: string[];
}

export interface ItemQuery {
  search?: string;
  folder?: string | null;
  trash?: boolean;
  favorites?: boolean;
}

export type ItemKind = "login" | "note" | "card" | "identity" | "passkey";

export interface ItemSummary {
  id: string;
  kind: ItemKind;
  title: string;
  subtitle: string;
  url: string | null;
  favorite: boolean;
  folder: string | null;
  hasPassword: boolean;
  hasTotp: boolean;
  trashed: boolean;
  updatedMs: number;
  conflicted: boolean;
}

export interface CardFields {
  holder: string;
  brand: string;
  number: string;
  expMonth: string;
  expYear: string;
  cvv: string;
}

export interface CustomField {
  id: string | null;
  label: string;
  value: string;
  hidden: boolean;
}

export interface ItemInput {
  id: string | null;
  kind: ItemKind;
  title: string;
  username: string;
  password: string;
  urls: string[];
  notes: string;
  totp: string;
  folder: string | null;
  favorite: boolean;
  card: CardFields | null;
  custom: CustomField[];
}

export interface HistoryEntry {
  value: string;
  setMs: number;
  device: string;
}

export interface ConflictValue {
  op: string;
  value: string;
  setMs: number;
  device: string;
}

export interface ConflictView {
  field: string;
  current: ConflictValue;
  alternatives: ConflictValue[];
}

export interface PasskeyInfo {
  rpId: string;
  rpName: string;
  userName: string;
  userDisplayName: string;
  createdMs: number;
}

export interface ItemView extends Omit<ItemInput, "id"> {
  id: string;
  passkey: PasskeyInfo | null;
  trashed: boolean;
  createdMs: number;
  updatedMs: number;
  updatedBy: string;
  passwordHistory: HistoryEntry[];
  conflicts: ConflictView[];
}

export interface TotpCode {
  code: string;
  period: number;
  remaining: number;
}

export interface FolderView {
  id: string;
  name: string;
  items: number;
}

export interface DeviceInfo {
  id: string;
  fingerprint: string;
  name: string;
  platform: string;
  status: "active" | "revoked" | "pending";
  thisDevice: boolean;
  genesis: boolean;
  addedBy: string | null;
  addedMs: number;
  lastSeenMs: number | null;
  lastSyncMs: number | null;
  lastError: string | null;
  online: boolean;
  addrs: string[];
}

export interface InviteInfo {
  code: string;
  codeDisplay: string;
  addrs: string[];
  qrText: string;
  qrSvg: string;
  expiresInSecs: number;
}

export interface PairingRequest {
  requestId: number;
  deviceName: string;
  platform: string;
  fingerprint: string;
}

export interface PeerStatus {
  device: string | null;
  name: string | null;
  addrs: string[];
  source: string;
  visible: boolean;
  busy: boolean;
  lastOkSecsAgo: number | null;
  failures: number;
  lastError: string | null;
}

export interface SyncStatus {
  running: boolean;
  port: number | null;
  listenAddrs: string[];
  peers: PeerStatus[];
  inviteActive: boolean;
}

export interface ImportReport {
  format: string;
  imported: number;
  duplicates: number;
  warnings: string[];
}

export interface PasswordOptions {
  length: number;
  lowercase: boolean;
  uppercase: boolean;
  digits: boolean;
  symbols: boolean;
  avoidAmbiguous: boolean;
}

/** Who is connected to the broker: a browser extension, or the Windows passkey provider. */
export type ClientKind = "extension" | "provider";

export interface BrowserClient {
  id: string;
  name: string;
  kind: ClientKind;
  createdMs: number;
  lastUsedMs: number;
}

export interface BrowserStatus {
  available: boolean;
  extensionId: string;
  extensionFolder: string | null;
  chrome: boolean;
  edge: boolean;
  clients: BrowserClient[];
}

/** Where the Windows passkey provider set-up stands (Settings > Windows passkeys). `available` is false
 *  where the section should be hidden (not Windows, or a development profile). */
export interface WindowsPasskeysStatus {
  available: boolean;
  supported: boolean;
  windowsVersion: string | null;
  developerMode: boolean;
  providerExeFound: boolean;
  packageRegistered: boolean;
  packageInBundle: boolean;
  registeredInWindows: boolean;
  enabledInWindows: boolean;
  paired: boolean;
  appPathOk: boolean;
  problem: string | null;
}

export type UiEvent =
  | { type: "status"; status: VaultStatus }
  | { type: "vaultChanged" }
  | { type: "syncCompleted"; peer: string; received: number; sent: number; clockSkewMs: number }
  | { type: "syncFailed"; addr: string; error: string }
  | { type: "peerDiscovered"; device: string | null }
  | { type: "networkChanged" }
  | ({ type: "pairingRequest" } & PairingRequest)
  | { type: "pairingFinished"; device: string | null; error: string | null }
  | { type: "inviteWrongCode"; attemptsLeft: number }
  | { type: "forkDetected" }
  | { type: "extensionPairing"; requestId: number; name: string; code: string; kind: ClientKind }
  | { type: "extensionPairingCancelled"; requestId: number }
  | { type: "confirmOverwrite"; requestId: number; kind: "login" | "passkey"; title: string; username: string; site: string; savedFor: string | null; sameHost: boolean }
  | { type: "confirmOverwriteCancelled"; requestId: number }
  | { type: "windowShown" };

/** One part of the phone's set-up: "unknown" means Android does not let an app read it. */
export type SetupState = "unsupported" | "on" | "off" | "unknown";
export interface AndroidSetup {
  sdk: number;
  autofill: SetupState;
  credentialProvider: SetupState;
  /** "off" when the phone has no screen lock, which passkeys need. */
  screenLock: SetupState;
  localNetwork: SetupState;
}
export type SetupTarget = "autofill" | "credentials" | "app" | "security";

export function onEvent(handler: (e: UiEvent) => void): Promise<UnlistenFn> {
  return listen<UiEvent>("lp-event", (e) => handler(e.payload));
}

/** Errors from Rust arrive as strings. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  touch: () => invoke<void>("touch"),
  passwordStrength: (password: string) => invoke<Strength>("password_strength", { password }),

  createVault: (password: string, deviceName: string) => invoke<void>("create_vault", { password, deviceName }),
  joinVault: (code: string, addrs: string[], password: string, deviceName: string) =>
    invoke<void>("join_vault", { code, addrs, password, deviceName }),
  unlock: (password: string) => invoke<void>("unlock", { password }),
  // `auto`: the unlock screen offering Hello by itself; the app refuses ("hidden") while its window is hidden.
  unlockQuick: (auto = false) => invoke<void>("unlock_quick", { auto }),
  lock: () => invoke<void>("lock"),
  quickUnlockEnable: () => invoke<void>("quick_unlock_enable"),
  quickUnlockDisable: () => invoke<void>("quick_unlock_disable"),
  changePassword: (old: string, next: string) => invoke<void>("change_password", { old, new: next }),

  listItems: (query: ItemQuery) => invoke<ItemSummary[]>("list_items", { query }),
  getItem: (id: string) => invoke<ItemView>("get_item", { id }),
  saveItem: (input: ItemInput) => invoke<string>("save_item", { input }),
  trashItem: (id: string) => invoke<void>("trash_item", { id }),
  restoreItem: (id: string) => invoke<void>("restore_item", { id }),
  purgeItem: (id: string) => invoke<void>("purge_item", { id }),
  emptyTrash: () => invoke<number>("empty_trash"),
  setFavorite: (id: string, favorite: boolean) => invoke<void>("set_favorite", { id, favorite }),
  resolveConflict: (id: string, field: string, op: string) => invoke<void>("resolve_conflict", { id, field, op }),
  copyField: (id: string, field: string) => invoke<number>("copy_field", { id, field }),
  copyText: (text: string) => invoke<number>("copy_text", { text }),
  totpCode: (id: string) => invoke<TotpCode>("totp_code", { id }),
  generatePassword: (options: PasswordOptions) =>
    invoke<{ password: string; entropyBits: number }>("generate_password", { options }),

  folders: () => invoke<FolderView[]>("folders"),
  saveFolder: (id: string | null, name: string) => invoke<string>("save_folder", { id, name }),
  deleteFolder: (id: string) => invoke<void>("delete_folder", { id }),

  devices: () => invoke<DeviceInfo[]>("devices"),
  renameDevice: (name: string) => invoke<void>("rename_device", { name }),
  revokeDevice: (id: string) => invoke<void>("revoke_device", { id }),
  inviteStart: () => invoke<InviteInfo>("invite_start"),
  inviteCancel: () => invoke<void>("invite_cancel"),
  inviteRespond: (requestId: number, approve: boolean) => invoke<void>("invite_respond", { requestId, approve }),
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  syncNow: () => invoke<number>("sync_now"),
  addPeer: (addr: string) => invoke<void>("add_peer", { addr }),

  importFile: (path: string, password: string | null, keyfile: string | null) =>
    invoke<ImportReport>("import_file", { path, password, keyfile }),
  exportBackup: (path: string, password: string) => invoke<void>("export_backup", { path, password }),
  /** Resolves to how many passkeys the plain file leaves out (only the encrypted backup carries them). */
  exportJson: (path: string) => invoke<number>("export_json", { path }),

  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  leaveVault: () => invoke<void>("leave_vault"),
  browserStatus: () => invoke<BrowserStatus>("browser_status"),
  browserInstall: () => invoke<BrowserStatus>("browser_install"),
  browserRemove: () => invoke<BrowserStatus>("browser_remove"),
  browserClientRemove: (id: string) => invoke<void>("browser_client_remove", { id }),
  brokerRespond: (requestId: number, approve: boolean) => invoke<void>("broker_respond", { requestId, approve }),
  openExtensionFolder: () => invoke<void>("open_extension_folder"),
  windowsPasskeysStatus: () => invoke<WindowsPasskeysStatus>("windows_passkeys_status"),
  windowsPasskeysSetup: () => invoke<WindowsPasskeysStatus>("windows_passkeys_setup"),
  windowsPasskeysPair: () => invoke<WindowsPasskeysStatus>("windows_passkeys_pair"),
  windowsPasskeysRemove: (forgetPairing: boolean) => invoke<WindowsPasskeysStatus>("windows_passkeys_remove", { forgetPairing }),
  windowsPasskeysOpenSettings: () => invoke<void>("windows_passkeys_open_settings"),
  androidSetupStatus: () => invoke<AndroidSetup>("android_setup_status"),
  androidOpenSetup: (target: SetupTarget) => invoke<void>("android_open_setup", { target }),
};

export const emptyItem = (kind: ItemKind = "login"): ItemInput => ({
  id: null,
  kind,
  title: "",
  username: "",
  password: "",
  urls: [],
  notes: "",
  totp: "",
  folder: null,
  favorite: false,
  card: kind === "card" ? { holder: "", brand: "", number: "", expMonth: "", expYear: "", cvv: "" } : null,
  custom: [],
});

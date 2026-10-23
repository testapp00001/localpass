//! Importing from other managers, exporting, and encrypted backups.

use std::collections::{BTreeMap, BTreeSet};

use lp_core::hlc::now_ms;
use lp_core::ids::FolderId;
use lp_core::materialize::ItemState;
use lp_core::model::{DataOp, FieldSet, FieldValue, keys, kinds};
use lp_core::passkey::{PasskeyKey, b64u};
use lp_import::{ImportCard, ImportField, ImportItem};
use serde::Serialize;
use zeroize::{Zeroize, Zeroizing};

use crate::items::{self, CardFields, CustomFieldData, ItemInput};
use crate::{Engine, EngineError, Result, backup};

/// The `passkey.*` fields of a passkey, in the order a backup carries them. Bitwarden's JSON has no
/// place for a passkey, so a passkey travels as a login-shaped item whose custom fields carry these
/// (the private key marked hidden); LocalPass recognises it again on import, other managers see a
/// login named for the site.
const PASSKEY_FIELDS: [&str; 8] = [
    keys::PASSKEY_RP_ID,
    keys::PASSKEY_RP_NAME,
    keys::PASSKEY_CREDENTIAL_ID,
    keys::PASSKEY_USER_HANDLE,
    keys::PASSKEY_USER_NAME,
    keys::PASSKEY_USER_DISPLAY_NAME,
    keys::PASSKEY_PRIVATE_KEY,
    keys::PASSKEY_CREATED_MS,
];

/// A passkey read back from a backup, checked.
struct BackedUpPasskey {
    rp_id: String,
    rp_name: String,
    credential_id: String,
    user_handle: String,
    user_name: String,
    user_display_name: String,
    private_key: Zeroizing<String>,
    created_ms: u64,
}

/// `None` when the item is not a passkey; `Some(Err)` when it claims to be one but is not usable (it
/// is then skipped with a warning, never turned into an empty login).
fn backed_up_passkey(item: &ImportItem) -> Option<std::result::Result<BackedUpPasskey, String>> {
    let field = |key: &str| {
        item.custom
            .iter()
            .find(|f| f.label == key)
            .map(|f| f.value.as_str())
    };
    let (rp_id, credential_id, private_key) = (
        field(keys::PASSKEY_RP_ID)?,
        field(keys::PASSKEY_CREDENTIAL_ID)?,
        field(keys::PASSKEY_PRIVATE_KEY)?,
    );
    Some((|| {
        let host_like = !rp_id.is_empty()
            && rp_id.len() <= 253
            && rp_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        if !host_like {
            return Err("the passkey's site is not a host name".to_string());
        }
        let bytes = |text: &str, max: usize, what: &str| {
            b64u::decode(text)
                .ok()
                .filter(|b| !b.is_empty() && b.len() <= max)
                .map(|b| b64u::encode(&b))
                .ok_or_else(|| format!("the passkey's {what} is not valid"))
        };
        let credential_id = bytes(credential_id, 1023, "credential id")?;
        let user_handle = bytes(
            field(keys::PASSKEY_USER_HANDLE).unwrap_or(""),
            64,
            "user handle",
        )?;
        let private = Zeroizing::new(
            b64u::decode(private_key)
                .map_err(|_| "the passkey's private key is not valid".to_string())?,
        );
        PasskeyKey::from_pkcs8_der(&private)
            .map_err(|_| "the passkey's private key is not valid".to_string())?;
        let text = |key: &str| field(key).unwrap_or("").to_string();
        Ok(BackedUpPasskey {
            rp_id: rp_id.to_string(),
            rp_name: text(keys::PASSKEY_RP_NAME),
            credential_id,
            user_handle,
            user_name: text(keys::PASSKEY_USER_NAME),
            user_display_name: text(keys::PASSKEY_USER_DISPLAY_NAME),
            private_key: Zeroizing::new(b64u::encode(&private)),
            created_ms: field(keys::PASSKEY_CREATED_MS)
                .and_then(|t| t.parse().ok())
                .unwrap_or_else(now_ms),
        })
    })())
}

/// The custom fields that carry a passkey through an export.
fn passkey_export_fields(item: &ItemState) -> Vec<ImportField> {
    PASSKEY_FIELDS
        .iter()
        .filter_map(|&key| {
            item.text(key).map(|value| ImportField {
                label: key.to_string(),
                value: value.to_string(),
                hidden: key == keys::PASSKEY_PRIVATE_KEY,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub format: String,
    pub imported: usize,
    pub duplicates: usize,
    pub warnings: Vec<String>,
}

fn signature(
    title: &str,
    username: &str,
    password: &str,
    url: Option<&str>,
) -> (String, String, String, String) {
    (
        title.trim().to_lowercase(),
        username.to_string(),
        password.to_string(),
        url.unwrap_or("").to_string(),
    )
}

impl Engine {
    /// Imports a Bitwarden JSON/CSV, browser CSV, KeePass .kdbx or LocalPass
    /// backup. `password` unlocks KeePass databases and backups.
    pub fn import_file(
        &self,
        bytes: &[u8],
        password: Option<&str>,
        keyfile: Option<&[u8]>,
        skip_duplicates: bool,
    ) -> Result<ImportReport> {
        let (format, mut parsed) = if backup::is_backup(bytes) {
            let pw =
                password.ok_or_else(|| EngineError::Invalid("enter the backup password".into()))?;
            let json = backup::open(bytes, pw)?;
            (
                "LocalPass backup".to_string(),
                lp_import::bitwarden::parse_json(&json).map_err(import_err)?,
            )
        } else {
            let (f, p) = lp_import::parse(bytes, password, keyfile).map_err(import_err)?;
            (f.name().to_string(), p)
        };
        let mut report = self.import_items(&mut parsed.items, skip_duplicates)?;
        report.format = format;
        report.warnings.splice(0..0, parsed.warnings);
        Ok(report)
    }

    fn import_items(&self, list: &mut [ImportItem], skip_duplicates: bool) -> Result<ImportReport> {
        let mut report = ImportReport {
            format: String::new(),
            imported: 0,
            duplicates: 0,
            warnings: Vec::new(),
        };
        let result = self.with_session(|s, ctx| {
            let mut folders: BTreeMap<String, FolderId> = s
                .state
                .folders
                .iter()
                .filter_map(|(id, f)| f.name.as_ref().map(|n| (n.to_lowercase(), *id)))
                .collect();
            let mut seen: BTreeSet<_> = s
                .state
                .items
                .values()
                .map(|i| {
                    let url = i
                        .get(lp_core::model::keys::URLS)
                        .and_then(|v| v.as_list())
                        .and_then(|l| l.first().cloned());
                    signature(
                        i.text(lp_core::model::keys::TITLE).unwrap_or(""),
                        i.text(lp_core::model::keys::USERNAME).unwrap_or(""),
                        i.text(lp_core::model::keys::PASSWORD).unwrap_or(""),
                        url.as_deref(),
                    )
                })
                .collect();

            let mut credentials: BTreeSet<String> = s
                .state
                .items
                .values()
                .filter(|i| !i.is_trashed())
                .filter_map(|i| i.text(keys::PASSKEY_CREDENTIAL_ID).map(str::to_string))
                .collect();

            for (n, item) in list.iter().enumerate() {
                let sig = signature(
                    &item.title,
                    &item.username,
                    &item.password,
                    item.urls.first().map(String::as_str),
                );
                if skip_duplicates && !seen.insert(sig) {
                    report.duplicates += 1;
                    continue;
                }
                let folder = match item
                    .folder
                    .as_deref()
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                {
                    None => None,
                    Some(name) => Some(match folders.get(&name.to_lowercase()) {
                        Some(id) => *id,
                        None => {
                            let id = FolderId::random()?;
                            let hlc = s.clock.tick(now_ms());
                            Self::write_data(
                                s,
                                ctx,
                                DataOp::FolderSet {
                                    folder: id,
                                    hlc,
                                    name: Some(name.to_string()),
                                },
                            )?;
                            folders.insert(name.to_lowercase(), id);
                            id
                        }
                    }),
                };
                if let Some(found) = backed_up_passkey(item) {
                    let passkey = match found {
                        Ok(p) => p,
                        Err(why) => {
                            report
                                .warnings
                                .push(format!("'{}': {why}, skipped it", item.title));
                            continue;
                        }
                    };
                    // Restoring the same backup twice must not leave two copies of a credential.
                    if !credentials.insert(passkey.credential_id.clone()) {
                        report.duplicates += 1;
                        continue;
                    }
                    let set = |key: &str, value: FieldValue| FieldSet {
                        key: key.to_string(),
                        value,
                        base: None,
                    };
                    let text =
                        |key: &str, value: &str| set(key, FieldValue::Text(value.to_string()));
                    let title = match item.title.trim() {
                        "" => passkey.rp_id.as_str(),
                        t => t,
                    };
                    let mut sets = vec![
                        text(keys::KIND, kinds::PASSKEY),
                        text(keys::TITLE, title),
                        set(
                            keys::URLS,
                            FieldValue::List(vec![format!("https://{}", passkey.rp_id)]),
                        ),
                        set(keys::FAVORITE, FieldValue::Bool(item.favorite)),
                        text(keys::PASSKEY_RP_ID, &passkey.rp_id),
                        text(keys::PASSKEY_CREDENTIAL_ID, &passkey.credential_id),
                        text(keys::PASSKEY_USER_HANDLE, &passkey.user_handle),
                        text(keys::PASSKEY_PRIVATE_KEY, &passkey.private_key),
                        text(keys::PASSKEY_CREATED_MS, &passkey.created_ms.to_string()),
                    ];
                    for (key, value) in [
                        (keys::PASSKEY_RP_NAME, &passkey.rp_name),
                        (keys::PASSKEY_USER_NAME, &passkey.user_name),
                        (keys::PASSKEY_USER_DISPLAY_NAME, &passkey.user_display_name),
                        // Lists show the username under the title.
                        (keys::USERNAME, &passkey.user_name),
                    ] {
                        if !value.is_empty() {
                            sets.push(text(key, value));
                        }
                    }
                    if let Some(folder) = folder {
                        sets.push(text(keys::FOLDER, &folder.to_string()));
                    }
                    let hlc = s.clock.tick(now_ms());
                    let op = DataOp::ItemSet {
                        item: lp_core::ids::ItemId::random()?,
                        hlc,
                        fields: sets,
                    };
                    match Self::write_data(s, ctx, op) {
                        Ok(()) => report.imported += 1,
                        Err(EngineError::Core(e)) => {
                            report.warnings.push(format!("'{}': {e}", item.title))
                        }
                        Err(e) => return Err(e),
                    }
                    continue;
                }
                let mut input = to_input(item, folder);
                if !input.totp.is_empty() && lp_core::totp::TotpSpec::parse(&input.totp).is_err() {
                    report.warnings.push(format!(
                        "'{}': 2FA secret not recognized, skipped it",
                        item.title
                    ));
                    input.totp.clear();
                }
                let fields = match items::input_fields(&input) {
                    Ok(f) => f,
                    Err(e) => {
                        report.warnings.push(format!("item {}: {e}", n + 1));
                        continue;
                    }
                };
                let sets: Vec<FieldSet> = fields
                    .into_iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(key, value)| FieldSet {
                        key,
                        value,
                        base: None,
                    })
                    .collect();
                let hlc = s.clock.tick(now_ms());
                let op = DataOp::ItemSet {
                    item: lp_core::ids::ItemId::random()?,
                    hlc,
                    fields: sets,
                };
                match Self::write_data(s, ctx, op) {
                    Ok(()) => report.imported += 1,
                    Err(EngineError::Core(e)) => {
                        report.warnings.push(format!("'{}': {e}", item.title))
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        });
        for item in list.iter_mut() {
            item.zeroize();
        }
        result?;
        if report.imported > 0 {
            self.after_local_write();
        }
        Ok(report)
    }

    /// All live (non-trashed) items in a portable form. Passkeys come with their private key only when
    /// `with_passkeys` is set, which only the encrypted backup does.
    pub fn export_items(&self, with_passkeys: bool) -> Result<Vec<ImportItem>> {
        self.with_session(|s, _| {
            Ok(s.state
                .items
                .values()
                .filter(|i| !i.is_trashed())
                .filter(|i| with_passkeys || i.text(keys::KIND) != Some(kinds::PASSKEY))
                .map(|i| {
                    let v = items::view(&s.state, i);
                    ImportItem {
                        folder: v
                            .folder
                            .as_deref()
                            .and_then(|f| FolderId::parse(f).ok())
                            .and_then(|f| s.state.folder_name(&f).map(str::to_string)),
                        card: v.card.map(|c| ImportCard {
                            holder: c.holder,
                            brand: c.brand,
                            number: c.number,
                            exp_month: c.exp_month,
                            exp_year: c.exp_year,
                            cvv: c.cvv,
                        }),
                        custom: if v.kind == kinds::PASSKEY {
                            passkey_export_fields(i)
                        } else {
                            v.custom
                                .into_iter()
                                .map(|c| ImportField {
                                    label: c.label,
                                    value: c.value,
                                    hidden: c.hidden,
                                })
                                .collect()
                        },
                        kind: v.kind,
                        title: v.title,
                        username: v.username,
                        password: v.password,
                        urls: v.urls,
                        notes: v.notes,
                        totp: v.totp,
                        favorite: v.favorite,
                    }
                })
                .collect())
        })
    }

    /// Unencrypted Bitwarden-compatible JSON. Treat the result as a secret. Passkeys are left out: a
    /// passkey's private key never goes into a plain file and no other manager could use it. Only the
    /// encrypted backup carries them; [`Engine::passkeys_left_out_of_plain_export`] says how many.
    pub fn export_json(&self) -> Result<String> {
        let mut items = self.export_items(false)?;
        let json = lp_import::bitwarden::to_json(&items);
        items.iter_mut().for_each(Zeroize::zeroize);
        Ok(json)
    }

    /// How many live passkeys [`Engine::export_json`] leaves out.
    pub fn passkeys_left_out_of_plain_export(&self) -> Result<usize> {
        self.with_session(|s, _| {
            Ok(s.state
                .items
                .values()
                .filter(|i| !i.is_trashed() && i.text(keys::KIND) == Some(kinds::PASSKEY))
                .count())
        })
    }

    /// Encrypted backup that can be restored with [`Engine::import_file`]. It is the only export that
    /// carries passkeys.
    pub fn export_backup(&self, password: &str) -> Result<Vec<u8>> {
        if password.chars().count() < 8 {
            return Err(EngineError::Invalid(
                "backup password must be at least 8 characters".into(),
            ));
        }
        let mut items = self.export_items(true)?;
        let mut json = lp_import::bitwarden::to_json(&items);
        items.iter_mut().for_each(Zeroize::zeroize);
        let out = backup::seal(&json, password, self.inner.config.kdf);
        json.zeroize();
        out
    }
}

fn to_input(item: &ImportItem, folder: Option<FolderId>) -> ItemInput {
    let kind = match item.kind.as_str() {
        k @ (kinds::LOGIN | kinds::NOTE | kinds::CARD | kinds::IDENTITY) => k.to_string(),
        _ => kinds::LOGIN.to_string(),
    };
    ItemInput {
        id: None,
        kind,
        title: if item.title.trim().is_empty() {
            "Untitled".into()
        } else {
            item.title.clone()
        },
        username: item.username.clone(),
        password: item.password.clone(),
        urls: item.urls.clone(),
        notes: item.notes.clone(),
        totp: item.totp.clone(),
        folder: folder.map(|f| f.to_string()),
        favorite: item.favorite,
        card: item.card.as_ref().map(|c| CardFields {
            holder: c.holder.clone(),
            brand: c.brand.clone(),
            number: c.number.clone(),
            exp_month: c.exp_month.clone(),
            exp_year: c.exp_year.clone(),
            cvv: c.cvv.clone(),
        }),
        custom: item
            .custom
            .iter()
            .map(|f| CustomFieldData {
                id: None,
                label: f.label.clone(),
                value: f.value.clone(),
                hidden: f.hidden,
            })
            .collect(),
    }
}

fn import_err(e: lp_import::ImportError) -> EngineError {
    EngineError::Invalid(e.to_string())
}

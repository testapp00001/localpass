//! Item DTOs exchanged with UIs, and conversion to/from field registers.

use std::collections::BTreeMap;

use lp_core::ids::{DeviceId, FolderId, ItemId, OpRef};
use lp_core::materialize::{ItemState, VaultState};
use lp_core::model::{CustomField, FieldSet, FieldValue, keys, kinds};
use lp_core::totp::TotpSpec;
use serde::{Deserialize, Serialize};

use crate::{EngineError, Result};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemQuery {
    pub search: Option<String>,
    pub folder: Option<String>,
    pub trash: bool,
    pub favorites: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub url: Option<String>,
    pub favorite: bool,
    pub folder: Option<String>,
    pub has_password: bool,
    pub has_totp: bool,
    pub trashed: bool,
    pub updated_ms: u64,
    pub conflicted: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardFields {
    pub holder: String,
    pub brand: String,
    pub number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub cvv: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomFieldData {
    /// Stable id; `None` when creating a new field.
    pub id: Option<String>,
    pub label: String,
    pub value: String,
    pub hidden: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ItemInput {
    /// `None` creates a new item.
    pub id: Option<String>,
    pub kind: String,
    pub title: String,
    pub username: String,
    pub password: String,
    pub urls: Vec<String>,
    pub notes: String,
    /// `otpauth://` URI or base32 secret; empty for none.
    pub totp: String,
    pub folder: Option<String>,
    pub favorite: bool,
    pub card: Option<CardFields>,
    pub custom: Vec<CustomFieldData>,
}

/// The visible side of a passkey (never the private key).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyView {
    pub rp_id: String,
    pub rp_name: String,
    pub user_name: String,
    pub user_display_name: String,
    pub created_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub username: String,
    pub password: String,
    pub urls: Vec<String>,
    pub notes: String,
    pub totp: String,
    pub folder: Option<String>,
    pub favorite: bool,
    pub trashed: bool,
    pub card: Option<CardFields>,
    pub passkey: Option<PasskeyView>,
    pub custom: Vec<CustomFieldData>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub updated_by: String,
    pub password_history: Vec<HistoryEntry>,
    pub conflicts: Vec<ConflictView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub value: String,
    pub set_ms: u64,
    pub device: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictView {
    pub field: String,
    pub current: ConflictValue,
    pub alternatives: Vec<ConflictValue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictValue {
    /// Opaque id to pass to `resolve_conflict`.
    pub op: String,
    pub value: String,
    pub set_ms: u64,
    pub device: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderView {
    pub id: String,
    pub name: String,
    pub items: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TotpCode {
    pub code: String,
    pub period: u64,
    pub remaining: u64,
}

pub fn encode_op_ref(r: &OpRef) -> String {
    format!("{}:{}", r.author.to_hex(), r.seq)
}

pub fn decode_op_ref(s: &str) -> Result<OpRef> {
    let (a, seq) = s
        .split_once(':')
        .ok_or_else(|| EngineError::Invalid("bad op reference".into()))?;
    Ok(OpRef {
        author: DeviceId::from_hex(a)?,
        seq: seq
            .parse()
            .map_err(|_| EngineError::Invalid("bad op reference".into()))?,
    })
}

pub(crate) fn device_label(state: &VaultState, id: &DeviceId) -> String {
    state
        .devices
        .get(id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| format!("Device {}", &id.fingerprint()[..4]))
}

fn text(s: &str) -> FieldValue {
    let t = s.trim();
    if t.is_empty() {
        FieldValue::Null
    } else {
        FieldValue::Text(s.to_string())
    }
}

/// Field values described by `input`. Empty strings clear fields.
pub(crate) fn input_fields(input: &ItemInput) -> Result<BTreeMap<String, FieldValue>> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(EngineError::Invalid("title is required".into()));
    }
    let kind = if input.kind.trim().is_empty() {
        kinds::LOGIN
    } else {
        input.kind.trim()
    };
    let mut m = BTreeMap::new();
    m.insert(keys::KIND.to_string(), FieldValue::Text(kind.to_string()));
    m.insert(keys::TITLE.to_string(), FieldValue::Text(title.to_string()));
    m.insert(keys::USERNAME.to_string(), text(&input.username));
    m.insert(keys::PASSWORD.to_string(), text(&input.password));
    m.insert(keys::NOTES.to_string(), text(&input.notes));
    let urls: Vec<String> = input
        .urls
        .iter()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .collect();
    m.insert(
        keys::URLS.to_string(),
        if urls.is_empty() {
            FieldValue::Null
        } else {
            FieldValue::List(urls)
        },
    );
    let totp = input.totp.trim();
    let totp_value = if totp.is_empty() {
        FieldValue::Null
    } else {
        let spec =
            TotpSpec::parse(totp).map_err(|e| EngineError::Invalid(format!("2FA secret: {e}")))?;
        FieldValue::Text(spec.to_uri())
    };
    m.insert(keys::TOTP.to_string(), totp_value);
    let folder = match input
        .folder
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        Some(f) => FieldValue::Text(FolderId::parse(f)?.to_string()),
        None => FieldValue::Null,
    };
    m.insert(keys::FOLDER.to_string(), folder);
    m.insert(keys::FAVORITE.to_string(), FieldValue::Bool(input.favorite));
    if let Some(card) = &input.card {
        m.insert(keys::CARD_HOLDER.to_string(), text(&card.holder));
        m.insert(keys::CARD_BRAND.to_string(), text(&card.brand));
        m.insert(
            keys::CARD_NUMBER.to_string(),
            text(&card.number.replace([' ', '-'], "")),
        );
        m.insert(keys::CARD_EXP_MONTH.to_string(), text(&card.exp_month));
        m.insert(keys::CARD_EXP_YEAR.to_string(), text(&card.exp_year));
        m.insert(keys::CARD_CVV.to_string(), text(&card.cvv));
    }
    for c in &input.custom {
        if c.label.trim().is_empty() && c.value.is_empty() {
            continue;
        }
        let id = match &c.id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => ItemId::random()?.to_string(),
        };
        m.insert(
            format!("{}{id}", keys::CUSTOM_PREFIX),
            FieldValue::Custom(CustomField {
                label: c.label.trim().to_string(),
                value: c.value.clone(),
                hidden: c.hidden,
            }),
        );
    }
    Ok(m)
}

/// Field sets that turn `current` into `desired` (custom fields missing from
/// `desired` are cleared). Each set records the value it replaces as its base.
pub(crate) fn diff(current: &ItemState, desired: BTreeMap<String, FieldValue>) -> Vec<FieldSet> {
    let mut sets = Vec::new();
    for (key, entries) in &current.fields {
        let is_live = entries.last().is_some_and(|e| !e.value.is_null());
        if key.starts_with(keys::CUSTOM_PREFIX) && is_live && !desired.contains_key(key) {
            sets.push(FieldSet {
                key: key.clone(),
                value: FieldValue::Null,
                base: entries.last().map(|e| e.stamp.op_ref()),
            });
        }
    }
    for (key, value) in desired {
        let winner = current.winner(&key);
        let unchanged = match winner {
            Some(w) => w.value == value,
            None => value.is_null(),
        };
        if !unchanged {
            sets.push(FieldSet {
                key,
                value,
                base: winner.map(|w| w.stamp.op_ref()),
            });
        }
    }
    sets
}

pub(crate) fn summary(item: &ItemState) -> ItemSummary {
    let kind = item.text(keys::KIND).unwrap_or(kinds::LOGIN).to_string();
    let subtitle = if kind == kinds::CARD {
        let brand = item.text(keys::CARD_BRAND).unwrap_or("Card");
        let last4: String = item
            .text(keys::CARD_NUMBER)
            .map(|n| {
                n.chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        if last4.is_empty() {
            brand.to_string()
        } else {
            format!("{brand} •••• {last4}")
        }
    } else {
        item.text(keys::USERNAME).unwrap_or("").to_string()
    };
    ItemSummary {
        id: item.id.to_string(),
        kind,
        title: item.text(keys::TITLE).unwrap_or("(untitled)").to_string(),
        subtitle,
        url: item
            .get(keys::URLS)
            .and_then(FieldValue::as_list)
            .and_then(|l| l.first().cloned()),
        favorite: item.flag(keys::FAVORITE),
        folder: item.text(keys::FOLDER).map(str::to_string),
        has_password: item.text(keys::PASSWORD).is_some(),
        has_totp: item.text(keys::TOTP).is_some(),
        trashed: item.is_trashed(),
        updated_ms: item.updated.hlc.wall_ms,
        conflicted: !item.conflicted_fields().is_empty(),
    }
}

fn value_text(v: &FieldValue) -> String {
    match v {
        FieldValue::Text(s) => s.clone(),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::List(l) => l.join("\n"),
        FieldValue::Custom(c) => c.value.clone(),
        FieldValue::Null => String::new(),
        FieldValue::Unknown => "(unsupported value)".into(),
    }
}

pub(crate) fn view(state: &VaultState, item: &ItemState) -> ItemView {
    let s = |k: &str| item.text(k).unwrap_or("").to_string();
    let kind = item.text(keys::KIND).unwrap_or(kinds::LOGIN).to_string();
    let card =
        (kind == kinds::CARD || item.fields.keys().any(|k| k.starts_with("card."))).then(|| {
            CardFields {
                holder: s(keys::CARD_HOLDER),
                brand: s(keys::CARD_BRAND),
                number: s(keys::CARD_NUMBER),
                exp_month: s(keys::CARD_EXP_MONTH),
                exp_year: s(keys::CARD_EXP_YEAR),
                cvv: s(keys::CARD_CVV),
            }
        });
    let passkey = (kind == kinds::PASSKEY).then(|| PasskeyView {
        rp_id: s(keys::PASSKEY_RP_ID),
        rp_name: s(keys::PASSKEY_RP_NAME),
        user_name: s(keys::PASSKEY_USER_NAME),
        user_display_name: s(keys::PASSKEY_USER_DISPLAY_NAME),
        created_ms: s(keys::PASSKEY_CREATED_MS).parse().unwrap_or(0),
    });
    let custom = item
        .fields
        .keys()
        .filter_map(|k| {
            let id = k.strip_prefix(keys::CUSTOM_PREFIX)?;
            match item.get(k)? {
                FieldValue::Custom(c) => Some(CustomFieldData {
                    id: Some(id.to_string()),
                    label: c.label.clone(),
                    value: c.value.clone(),
                    hidden: c.hidden,
                }),
                _ => None,
            }
        })
        .collect();
    let password_history = item
        .history(keys::PASSWORD)
        .filter_map(|e| {
            e.value.as_text().map(|v| HistoryEntry {
                value: v.to_string(),
                set_ms: e.stamp.hlc.wall_ms,
                device: device_label(state, &e.stamp.author),
            })
        })
        .take(20)
        .collect();
    let conflicts = item
        .conflicted_fields()
        .into_iter()
        // A passkey's fields are written together by one operation and are not edited by hand; showing
        // them would put the private key in the webview and invite mixing two registrations.
        .filter(|field| !field.starts_with("passkey."))
        .filter_map(|field| {
            let winner = item.winner(field)?;
            let to_value = |e: &lp_core::materialize::FieldEntry| ConflictValue {
                op: encode_op_ref(&e.stamp.op_ref()),
                value: value_text(&e.value),
                set_ms: e.stamp.hlc.wall_ms,
                device: device_label(state, &e.stamp.author),
            };
            Some(ConflictView {
                field: field.to_string(),
                current: to_value(winner),
                alternatives: item.conflicts(field).into_iter().map(to_value).collect(),
            })
        })
        .collect();
    ItemView {
        id: item.id.to_string(),
        kind,
        title: s(keys::TITLE),
        username: s(keys::USERNAME),
        password: s(keys::PASSWORD),
        urls: item
            .get(keys::URLS)
            .and_then(FieldValue::as_list)
            .map(<[String]>::to_vec)
            .unwrap_or_default(),
        notes: s(keys::NOTES),
        totp: s(keys::TOTP),
        folder: item.text(keys::FOLDER).map(str::to_string),
        favorite: item.flag(keys::FAVORITE),
        trashed: item.is_trashed(),
        card,
        passkey,
        custom,
        created_ms: item.created.hlc.wall_ms,
        updated_ms: item.updated.hlc.wall_ms,
        updated_by: device_label(state, &item.updated.author),
        password_history,
        conflicts,
    }
}

pub(crate) fn matches(item: &ItemState, q: &ItemQuery) -> bool {
    if item.is_trashed() != q.trash {
        return false;
    }
    if q.favorites && !item.flag(keys::FAVORITE) {
        return false;
    }
    if let Some(folder) = q.folder.as_deref().filter(|f| !f.is_empty())
        && item.text(keys::FOLDER) != Some(folder)
    {
        return false;
    }
    match q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => true,
        Some(needle) => {
            let needle = needle.to_lowercase();
            let hit = |k: &str| {
                item.text(k)
                    .is_some_and(|v| v.to_lowercase().contains(&needle))
            };
            hit(keys::TITLE)
                || hit(keys::USERNAME)
                || hit(keys::NOTES)
                || item
                    .get(keys::URLS)
                    .and_then(FieldValue::as_list)
                    .is_some_and(|l| l.iter().any(|u| u.to_lowercase().contains(&needle)))
        }
    }
}

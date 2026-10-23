//! Bitwarden unencrypted JSON export: reading, and writing (our export format).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{ImportCard, ImportError, ImportField, ImportItem, Parsed, Result, strip_bom};

#[derive(Deserialize)]
struct Export {
    #[serde(default)]
    encrypted: bool,
    #[serde(default)]
    folders: Vec<Folder>,
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Deserialize, Serialize)]
struct Folder {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    #[serde(rename = "type")]
    kind: u8,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    folder_id: Option<String>,
    #[serde(default)]
    login: Option<Login>,
    #[serde(default)]
    card: Option<Card>,
    #[serde(default)]
    identity: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    fields: Option<Vec<Field>>,
}

#[derive(Deserialize)]
struct Login {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    totp: Option<String>,
    #[serde(default)]
    uris: Option<Vec<Uri>>,
}

#[derive(Deserialize)]
struct Uri {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Card {
    #[serde(default)]
    cardholder_name: Option<String>,
    #[serde(default)]
    brand: Option<String>,
    #[serde(default)]
    number: Option<String>,
    #[serde(default)]
    exp_month: Option<String>,
    #[serde(default)]
    exp_year: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Deserialize)]
struct Field {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(rename = "type", default)]
    kind: u8,
}

fn s(v: &Option<String>) -> String {
    v.clone().unwrap_or_default()
}

/// "firstName" → "First name".
fn humanize(key: &str) -> String {
    let mut out = String::new();
    for (i, c) in key.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn parse_json(bytes: &[u8]) -> Result<Parsed> {
    let export: Export =
        serde_json::from_slice(strip_bom(bytes)).map_err(|e| ImportError::Json(e.to_string()))?;
    if export.encrypted {
        return Err(ImportError::Unsupported(
            "this Bitwarden export is encrypted; export again choosing the unencrypted .json format".into(),
        ));
    }
    let folders: BTreeMap<String, String> =
        export.folders.into_iter().map(|f| (f.id, f.name)).collect();
    let mut parsed = Parsed::default();
    for (n, it) in export.items.into_iter().enumerate() {
        let mut item = ImportItem {
            title: s(&it.name),
            notes: s(&it.notes),
            favorite: it.favorite,
            folder: it
                .folder_id
                .as_ref()
                .and_then(|id| folders.get(id).cloned()),
            ..Default::default()
        };
        match it.kind {
            1 => {
                item.kind = "login".into();
                if let Some(l) = &it.login {
                    item.username = s(&l.username);
                    item.password = s(&l.password);
                    item.totp = s(&l.totp);
                    item.urls = l
                        .uris
                        .iter()
                        .flatten()
                        .filter_map(|u| u.uri.clone())
                        .filter(|u| !u.trim().is_empty())
                        .collect();
                }
            }
            2 => item.kind = "note".into(),
            3 => {
                item.kind = "card".into();
                if let Some(c) = &it.card {
                    item.card = Some(ImportCard {
                        holder: s(&c.cardholder_name),
                        brand: s(&c.brand),
                        number: s(&c.number),
                        exp_month: s(&c.exp_month),
                        exp_year: s(&c.exp_year),
                        cvv: s(&c.code),
                    });
                }
            }
            4 => {
                item.kind = "identity".into();
                for (k, v) in it.identity.iter().flatten() {
                    if let Value::String(val) = v
                        && !val.trim().is_empty()
                    {
                        let hidden =
                            matches!(k.as_str(), "ssn" | "passportNumber" | "licenseNumber");
                        item.custom.push(ImportField {
                            label: humanize(k),
                            value: val.clone(),
                            hidden,
                        });
                    }
                }
            }
            other => {
                item.kind = "note".into();
                parsed.warnings.push(format!(
                    "item {} has unsupported type {other}; imported as a note",
                    n + 1
                ));
            }
        }
        for f in it.fields.iter().flatten() {
            // type 0 = text, 1 = hidden, 2 = boolean, 3 = linked (skipped)
            if f.kind == 3 {
                continue;
            }
            item.custom.push(ImportField {
                label: s(&f.name),
                value: s(&f.value),
                hidden: f.kind == 1,
            });
        }
        if item.title.trim().is_empty() {
            item.title = item
                .urls
                .first()
                .and_then(|u| crate::host_of(u))
                .unwrap_or_else(|| "Untitled".into());
        }
        parsed.items.push(item);
    }
    Ok(parsed)
}

/// Writes items as an unencrypted Bitwarden JSON export.
pub fn to_json(items: &[ImportItem]) -> String {
    let mut folder_ids: BTreeMap<String, String> = BTreeMap::new();
    for name in items.iter().filter_map(|i| i.folder.clone()) {
        let n = folder_ids.len() + 1;
        folder_ids
            .entry(name)
            .or_insert_with(|| format!("00000000-0000-4000-8000-{n:012x}"));
    }
    let folders: Vec<Value> = folder_ids
        .iter()
        .map(|(name, id)| json!({ "id": id, "name": name }))
        .collect();
    let items: Vec<Value> = items
        .iter()
        .map(|i| {
            let kind = match i.kind.as_str() {
                "note" => 2,
                "card" => 3,
                "identity" => 4,
                _ => 1,
            };
            let mut v = json!({
                "type": kind,
                "name": i.title,
                "notes": if i.notes.is_empty() { Value::Null } else { Value::String(i.notes.clone()) },
                "favorite": i.favorite,
                "folderId": i.folder.as_ref().and_then(|f| folder_ids.get(f)),
                "fields": i.custom.iter().map(|f| json!({
                    "name": f.label, "value": f.value, "type": if f.hidden { 1 } else { 0 },
                })).collect::<Vec<_>>(),
            });
            match kind {
                1 => {
                    v["login"] = json!({
                        "username": i.username,
                        "password": i.password,
                        "totp": if i.totp.is_empty() { Value::Null } else { Value::String(i.totp.clone()) },
                        "uris": i.urls.iter().map(|u| json!({ "match": null, "uri": u })).collect::<Vec<_>>(),
                    });
                }
                2 => v["secureNote"] = json!({ "type": 0 }),
                3 => {
                    let c = i.card.clone().unwrap_or_default();
                    v["card"] = json!({
                        "cardholderName": c.holder, "brand": c.brand, "number": c.number,
                        "expMonth": c.exp_month, "expYear": c.exp_year, "code": c.cvv,
                    });
                }
                _ => {}
            }
            v
        })
        .collect();
    serde_json::to_string_pretty(&json!({ "encrypted": false, "folders": folders, "items": items }))
        .expect("JSON values serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "encrypted": false,
      "folders": [{ "id": "f1", "name": "Work" }],
      "items": [
        { "type": 1, "name": "Gmail", "notes": "personal", "favorite": true, "folderId": null,
          "login": { "username": "me@gmail.com", "password": "pw1", "totp": "JBSWY3DPEHPK3PXP",
                     "uris": [{ "match": null, "uri": "https://mail.google.com" }] },
          "fields": [{ "name": "PIN", "value": "1234", "type": 1 }, { "name": "x", "value": null, "type": 3 }] },
        { "type": 2, "name": "Wifi", "notes": "pass: abc", "folderId": "f1", "secureNote": { "type": 0 } },
        { "type": 3, "name": "Visa", "card": { "cardholderName": "Me", "brand": "Visa", "number": "4111111111111111",
          "expMonth": "1", "expYear": "2030", "code": "123" } },
        { "type": 4, "name": "Me", "identity": { "firstName": "Jane", "ssn": "123-45", "email": null } },
        { "type": 1, "name": "", "login": { "uris": [{ "uri": "https://www.github.com/login" }] } }
      ]
    }"#;

    #[test]
    fn parses_all_item_types() {
        let p = parse_json(SAMPLE.as_bytes()).unwrap();
        assert_eq!(p.items.len(), 5);
        let gmail = &p.items[0];
        assert_eq!(
            (
                gmail.kind.as_str(),
                gmail.username.as_str(),
                gmail.password.as_str()
            ),
            ("login", "me@gmail.com", "pw1")
        );
        assert_eq!(gmail.urls, vec!["https://mail.google.com"]);
        assert!(gmail.favorite);
        assert_eq!(
            gmail.custom,
            vec![ImportField {
                label: "PIN".into(),
                value: "1234".into(),
                hidden: true
            }]
        );
        assert_eq!(p.items[1].folder.as_deref(), Some("Work"));
        assert_eq!(p.items[2].card.as_ref().unwrap().cvv, "123");
        let identity = &p.items[3];
        assert!(
            identity
                .custom
                .iter()
                .any(|f| f.label == "First name" && f.value == "Jane")
        );
        assert!(identity.custom.iter().any(|f| f.label == "Ssn" && f.hidden));
        assert_eq!(p.items[4].title, "github.com");
    }

    #[test]
    fn rejects_encrypted_exports() {
        assert!(matches!(
            parse_json(br#"{"encrypted": true, "items": []}"#),
            Err(ImportError::Unsupported(_))
        ));
    }

    #[test]
    fn export_round_trips() {
        let p = parse_json(SAMPLE.as_bytes()).unwrap();
        let json = to_json(&p.items);
        let back = parse_json(json.as_bytes()).unwrap();
        assert_eq!(back.items, p.items);
    }
}

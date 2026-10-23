//! KeePass / KeePassXC databases (.kdbx 3 and 4).

use keepass::db::{GroupRef, fields};
use keepass::error::{DatabaseKeyError, DatabaseOpenError};
use keepass::{Database, DatabaseKey};

use crate::{ImportError, ImportField, ImportItem, Parsed, Result};

/// KeePass 2.47+ stores TOTP in these fields (KeePassXC uses `otp`).
const TIMEOTP_PREFIX: &str = "TimeOtp-";

pub fn parse(bytes: &[u8], password: Option<&str>, keyfile: Option<&[u8]>) -> Result<Parsed> {
    let mut key = DatabaseKey::new();
    if let Some(pw) = password {
        key = key.with_password(pw);
    }
    if let Some(kf) = keyfile {
        key = key
            .with_keyfile(&mut std::io::Cursor::new(kf))
            .map_err(|e| ImportError::Keepass(format!("key file: {e}")))?;
    }
    let db = Database::parse(bytes, key).map_err(|e| match e {
        DatabaseOpenError::Key(DatabaseKeyError::IncorrectKey) => ImportError::WrongKey,
        other => ImportError::Keepass(other.to_string()),
    })?;
    let recycle_bin = db.meta.recyclebin_uuid;
    let mut parsed = Parsed::default();
    walk(db.root(), &[], recycle_bin, &mut parsed);
    Ok(parsed)
}

fn walk(group: GroupRef<'_>, path: &[String], recycle_bin: Option<uuid::Uuid>, out: &mut Parsed) {
    let folder = (!path.is_empty()).then(|| path.join("/"));
    for entry in group.entries() {
        let get = |k: &str| entry.get(k).unwrap_or("").to_string();
        let mut item = ImportItem {
            kind: "login".into(),
            title: get(fields::TITLE),
            username: get(fields::USERNAME),
            password: get(fields::PASSWORD),
            urls: Some(get(fields::URL))
                .filter(|u| !u.trim().is_empty())
                .into_iter()
                .collect(),
            notes: get(fields::NOTES),
            totp: totp_of(&entry),
            folder: folder.clone(),
            ..Default::default()
        };
        let mut extra: Vec<(&String, &keepass::db::Value<String>)> = entry
            .fields
            .iter()
            .filter(|(k, _)| {
                !fields::KNOWN_FIELDS.contains(&k.as_str())
                    && k.as_str() != fields::OTP
                    && !k.starts_with(TIMEOTP_PREFIX)
                    && !k.starts_with("KPRPC")
            })
            .collect();
        extra.sort_by_key(|(k, _)| k.as_str());
        for (k, v) in extra {
            item.custom.push(ImportField {
                label: k.clone(),
                value: v.get().clone(),
                hidden: v.is_protected(),
            });
        }
        if item.title.trim().is_empty() {
            item.title = item
                .urls
                .first()
                .and_then(|u| crate::host_of(u))
                .unwrap_or_else(|| "Untitled".into());
        }
        if item.username.is_empty() && item.password.is_empty() && item.urls.is_empty() {
            item.kind = "note".into();
        }
        out.items.push(item);
    }
    for child in group.groups() {
        if recycle_bin.is_some_and(|rb| child.id().uuid() == rb) {
            continue;
        }
        let mut child_path = path.to_vec();
        child_path.push(child.name.clone());
        walk(child, &child_path, recycle_bin, out);
    }
}

fn totp_of(entry: &keepass::db::EntryRef<'_>) -> String {
    if let Some(otp) = entry.get(fields::OTP).filter(|v| !v.trim().is_empty()) {
        return otp.to_string();
    }
    let Some(secret) = entry
        .get("TimeOtp-Secret-Base32")
        .filter(|v| !v.trim().is_empty())
    else {
        return String::new();
    };
    let algorithm = match entry.get("TimeOtp-Algorithm").unwrap_or("") {
        "HMAC-SHA-256" => "SHA256",
        "HMAC-SHA-512" => "SHA512",
        _ => "SHA1",
    };
    let digits = entry.get("TimeOtp-Length").unwrap_or("6");
    let period = entry.get("TimeOtp-Period").unwrap_or("30");
    format!(
        "otpauth://totp/KeePass?secret={}&algorithm={algorithm}&digits={digits}&period={period}",
        secret.replace(' ', "")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use keepass::db::EntryMut;

    fn sample_db(password: &str) -> Vec<u8> {
        let mut db = Database::new();
        let mut root = db.root_mut();
        root.add_entry().edit(|e: &mut EntryMut<'_>| {
            e.set_unprotected(fields::TITLE, "Router");
            e.set_unprotected(fields::USERNAME, "admin");
            e.set_protected(fields::PASSWORD, "router-pw");
            e.set_unprotected(fields::URL, "http://192.168.1.1");
            e.set_protected("PIN", "9999");
            e.set_unprotected(fields::OTP, "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP");
        });
        let mut work = root.add_group();
        work.name = "Work".into();
        work.add_entry().edit(|e: &mut EntryMut<'_>| {
            e.set_unprotected(fields::TITLE, "Jira");
            e.set_unprotected(fields::USERNAME, "me");
            e.set_protected(fields::PASSWORD, "jira-pw");
            e.set_unprotected("TimeOtp-Secret-Base32", "JBSWY3DPEHPK3PXP");
            e.set_unprotected("TimeOtp-Algorithm", "HMAC-SHA-256");
        });
        let mut out = Vec::new();
        db.save(&mut out, DatabaseKey::new().with_password(password))
            .unwrap();
        out
    }

    #[test]
    fn reads_entries_groups_custom_fields_and_totp() {
        let bytes = sample_db("kp-pass");
        assert_eq!(crate::detect(&bytes), Some(crate::Format::Keepass));
        let p = parse(&bytes, Some("kp-pass"), None).unwrap();
        assert_eq!(p.items.len(), 2);
        let router = p.items.iter().find(|i| i.title == "Router").unwrap();
        assert_eq!(router.password, "router-pw");
        assert_eq!(router.urls, vec!["http://192.168.1.1"]);
        assert_eq!(
            router.custom,
            vec![ImportField {
                label: "PIN".into(),
                value: "9999".into(),
                hidden: true
            }]
        );
        assert!(router.totp.starts_with("otpauth://"));
        assert!(router.folder.is_none());
        let jira = p.items.iter().find(|i| i.title == "Jira").unwrap();
        assert_eq!(jira.folder.as_deref(), Some("Work"));
        assert!(jira.totp.contains("algorithm=SHA256"));
        assert!(jira.custom.is_empty());
    }

    #[test]
    fn wrong_password_is_reported() {
        let bytes = sample_db("kp-pass");
        assert!(matches!(
            parse(&bytes, Some("nope"), None),
            Err(ImportError::WrongKey)
        ));
    }
}

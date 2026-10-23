//! CSV exports from browsers and other managers, matched by header names:
//! Chrome/Edge (`name,url,username,password,note`), Firefox, Safari,
//! Bitwarden CSV, LastPass, 1Password, KeePassXC and similar.

use crate::{ImportError, ImportField, ImportItem, Parsed, Result, host_of, strip_bom};

#[derive(Default)]
struct Columns {
    title: Option<usize>,
    url: Option<usize>,
    username: Option<usize>,
    password: Option<usize>,
    notes: Option<usize>,
    totp: Option<usize>,
    folder: Option<usize>,
    favorite: Option<usize>,
    kind: Option<usize>,
    fields: Option<usize>,
}

const TITLE: &[&str] = &["name", "title", "account", "item name", "description"];
const URL: &[&str] = &[
    "url",
    "login_uri",
    "uri",
    "website",
    "web site",
    "login url",
    "hostname",
    "origin_url",
];
const USERNAME: &[&str] = &[
    "username",
    "login_username",
    "user name",
    "user",
    "login",
    "email",
    "login name",
    "userid",
];
const PASSWORD: &[&str] = &["password", "login_password", "pass"];
const NOTES: &[&str] = &["notes", "note", "extra", "comments", "comment"];
const TOTP: &[&str] = &[
    "totp",
    "login_totp",
    "otpauth",
    "otp",
    "one-time password",
    "2fa",
    "otp auth",
];
const FOLDER: &[&str] = &["folder", "grouping", "group", "category"];
const FAVORITE: &[&str] = &["favorite", "fav", "favourite"];

fn find(headers: &[String], names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|n| headers.iter().position(|h| h == n))
}

pub fn parse(bytes: &[u8]) -> Result<Parsed> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(strip_bom(bytes));
    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| ImportError::Csv(e.to_string()))?
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let cols = Columns {
        title: find(&headers, TITLE),
        url: find(&headers, URL),
        username: find(&headers, USERNAME),
        password: find(&headers, PASSWORD),
        notes: find(&headers, NOTES),
        totp: find(&headers, TOTP),
        folder: find(&headers, FOLDER),
        favorite: find(&headers, FAVORITE),
        kind: headers.iter().position(|h| h == "type"),
        fields: headers.iter().position(|h| h == "fields"),
    };
    if cols.password.is_none() && cols.username.is_none() && cols.notes.is_none() {
        return Err(ImportError::Unsupported(format!(
            "CSV has no username/password/notes columns (headers: {})",
            headers.join(", ")
        )));
    }

    let mut parsed = Parsed::default();
    for (n, record) in reader.records().enumerate() {
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                parsed.warnings.push(format!("row {}: {e}", n + 2));
                continue;
            }
        };
        let get = |c: Option<usize>| {
            c.and_then(|i| record.get(i))
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        };
        let url = get(cols.url);
        // LastPass marks secure notes with the pseudo-URL "http://sn".
        let lastpass_note = url == "http://sn";
        let kind = match get(cols.kind).to_lowercase().as_str() {
            "note" | "securenote" | "secure note" => "note",
            _ if lastpass_note => "note",
            _ => "login",
        };
        let mut item = ImportItem {
            kind: kind.into(),
            title: get(cols.title),
            username: get(cols.username),
            password: cols
                .password
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .to_string(),
            urls: if url.is_empty() || lastpass_note {
                vec![]
            } else {
                vec![url]
            },
            notes: cols
                .notes
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .to_string(),
            totp: get(cols.totp),
            folder: Some(get(cols.folder)).filter(|f| !f.is_empty()),
            favorite: matches!(
                get(cols.favorite).to_lowercase().as_str(),
                "1" | "true" | "yes"
            ),
            ..Default::default()
        };
        // Bitwarden CSV packs custom fields as "name: value" lines.
        for line in get(cols.fields).lines() {
            if let Some((label, value)) = line.split_once(": ") {
                item.custom.push(ImportField {
                    label: label.into(),
                    value: value.into(),
                    hidden: false,
                });
            }
        }
        if item.title.is_empty()
            && item.username.is_empty()
            && item.password.is_empty()
            && item.urls.is_empty()
            && item.notes.is_empty()
        {
            continue;
        }
        if item.title.is_empty() {
            item.title = item
                .urls
                .first()
                .and_then(|u| host_of(u))
                .or_else(|| Some(item.username.clone()).filter(|u| !u.is_empty()))
                .unwrap_or_else(|| "Imported item".into());
        }
        parsed.items.push(item);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_csv() {
        let csv = "name,url,username,password,note\n\
                   github.com,https://github.com/login,me,gh-pw,\n\
                   ,https://www.example.com/,bob,pw2,\"multi\nline\"\n";
        let p = parse(csv.as_bytes()).unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].title, "github.com");
        assert_eq!(p.items[0].password, "gh-pw");
        assert_eq!(p.items[1].title, "example.com");
        assert_eq!(p.items[1].notes, "multi\nline");
    }

    #[test]
    fn firefox_csv() {
        let csv = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
                   \"https://accounts.firefox.com\",\"me@x.com\",\"pw\",,\"https://accounts.firefox.com\",\"{1}\",\"1\",\"1\",\"1\"\n";
        let p = parse(csv.as_bytes()).unwrap();
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].title, "accounts.firefox.com");
        assert_eq!(p.items[0].username, "me@x.com");
    }

    #[test]
    fn bitwarden_csv_with_fields_and_notes() {
        let csv = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
                   Work,1,login,Jira,,\"PIN: 42\",0,https://jira.corp,me,jpw,JBSWY3DPEHPK3PXP\n\
                   ,,note,Wifi,ssid pw,,0,,,,\n";
        let p = parse(csv.as_bytes()).unwrap();
        assert_eq!(p.items.len(), 2);
        let jira = &p.items[0];
        assert_eq!(jira.folder.as_deref(), Some("Work"));
        assert!(jira.favorite);
        assert_eq!(jira.totp, "JBSWY3DPEHPK3PXP");
        assert_eq!(jira.custom[0].label, "PIN");
        assert_eq!(p.items[1].kind, "note");
    }

    #[test]
    fn lastpass_secure_note() {
        let csv = "url,username,password,totp,extra,name,grouping,fav\n\
                   http://sn,,,,secret note,My note,Personal,0\n";
        let p = parse(csv.as_bytes()).unwrap();
        assert_eq!(p.items[0].kind, "note");
        assert!(p.items[0].urls.is_empty());
        assert_eq!(p.items[0].folder.as_deref(), Some("Personal"));
    }

    #[test]
    fn rejects_unrelated_csv() {
        assert!(parse(b"a,b,c\n1,2,3\n").is_err());
    }
}

//! Import from other password managers, and export to a portable format.
//!
//! Every importer produces plain [`ImportItem`]s; the engine turns them into
//! vault ops. Export writes Bitwarden-compatible JSON, which Bitwarden,
//! KeePassXC, 1Password and others can import, so you're never locked in.

pub mod bitwarden;
pub mod csv_import;
pub mod keepass_import;

use zeroize::Zeroize;

#[derive(Clone, Debug, Default, PartialEq, Eq, Zeroize)]
/// Call [`Zeroize::zeroize`] once an item has been imported (it holds secrets).
pub struct ImportItem {
    /// "login", "note", "card" or "identity".
    pub kind: String,
    pub title: String,
    pub username: String,
    pub password: String,
    pub urls: Vec<String>,
    pub notes: String,
    /// `otpauth://` URI or base32 secret.
    pub totp: String,
    /// Folder name; nested groups are joined with "/".
    pub folder: Option<String>,
    pub favorite: bool,
    pub card: Option<ImportCard>,
    pub custom: Vec<ImportField>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Zeroize)]
pub struct ImportCard {
    pub holder: String,
    pub brand: String,
    pub number: String,
    pub exp_month: String,
    pub exp_year: String,
    pub cvv: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Zeroize)]
pub struct ImportField {
    pub label: String,
    pub value: String,
    pub hidden: bool,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub items: Vec<ImportItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("not valid JSON: {0}")]
    Json(String),
    #[error("could not read CSV: {0}")]
    Csv(String),
    #[error("could not open KeePass database: {0}")]
    Keepass(String),
    #[error("wrong KeePass password or key file")]
    WrongKey,
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T, E = ImportError> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    BitwardenJson,
    Csv,
    Keepass,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::BitwardenJson => "Bitwarden JSON",
            Format::Csv => "CSV",
            Format::Keepass => "KeePass",
        }
    }
}

const KDBX_MAGIC: [u8; 4] = [0x03, 0xd9, 0xa2, 0x9a];

/// Guesses the format from the file contents.
pub fn detect(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&KDBX_MAGIC) {
        return Some(Format::Keepass);
    }
    let text = strip_bom(bytes);
    let first = text.iter().find(|b| !b.is_ascii_whitespace())?;
    if *first == b'{' {
        return Some(Format::BitwardenJson);
    }
    let line_end = text.iter().position(|b| *b == b'\n').unwrap_or(text.len());
    text[..line_end].contains(&b',').then_some(Format::Csv)
}

/// Parses a file. `password`/`keyfile` are only used for KeePass databases.
pub fn parse(
    bytes: &[u8],
    password: Option<&str>,
    keyfile: Option<&[u8]>,
) -> Result<(Format, Parsed)> {
    let format = detect(bytes).ok_or_else(|| {
        ImportError::Unsupported(
            "unrecognized file: expected a Bitwarden .json, a .csv or a KeePass .kdbx".into(),
        )
    })?;
    let parsed = match format {
        Format::BitwardenJson => bitwarden::parse_json(bytes)?,
        Format::Csv => csv_import::parse(bytes)?,
        Format::Keepass => keepass_import::parse(bytes, password, keyfile)?,
    };
    Ok((format, parsed))
}

pub(crate) fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// "https://www.example.com/login" → "example.com".
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#', ':']).next()?.trim();
    let host = host.rsplit('@').next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        assert_eq!(
            detect(b"\xEF\xBB\xBF  {\"items\":[]}"),
            Some(Format::BitwardenJson)
        );
        assert_eq!(detect(b"name,url,username,password\n"), Some(Format::Csv));
        assert_eq!(
            detect(&[0x03, 0xd9, 0xa2, 0x9a, 0x67, 0xfb, 0x4b, 0xb5]),
            Some(Format::Keepass)
        );
        assert_eq!(detect(b"hello"), None);
    }

    #[test]
    fn hosts() {
        assert_eq!(
            host_of("https://www.example.com/login?x=1").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("accounts.google.com").as_deref(),
            Some("accounts.google.com")
        );
        assert_eq!(host_of("http://user@host:8080/").as_deref(), Some("host"));
        assert_eq!(host_of(""), None);
    }
}

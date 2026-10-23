//! RFC 6238 TOTP codes from `otpauth://totp/...` URIs or bare base32 secrets.

use std::fmt;
use std::sync::LazyLock;

use data_encoding::{BASE32_NOPAD, Encoding, Specification};
use hmac::{Hmac, Mac};
use zeroize::Zeroizing;

use crate::{CoreError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TotpAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl TotpAlgorithm {
    fn name(self) -> &'static str {
        match self {
            TotpAlgorithm::Sha1 => "SHA1",
            TotpAlgorithm::Sha256 => "SHA256",
            TotpAlgorithm::Sha512 => "SHA512",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TotpSpec {
    secret: Zeroizing<Vec<u8>>,
    pub algorithm: TotpAlgorithm,
    pub digits: u32,
    pub period: u64,
    pub issuer: Option<String>,
    pub account: Option<String>,
}

/// Lenient base32: case-insensitive input, any trailing bits accepted.
static BASE32_LENIENT: LazyLock<Encoding> = LazyLock::new(|| {
    let mut spec = Specification::new();
    spec.symbols.push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567");
    spec.check_trailing_bits = false;
    spec.encoding().expect("valid base32 spec")
});

impl TotpSpec {
    pub fn from_secret(
        secret: Vec<u8>,
        algorithm: TotpAlgorithm,
        digits: u32,
        period: u64,
    ) -> Result<Self> {
        if secret.is_empty() {
            return Err(CoreError::Totp("secret is empty".into()));
        }
        if !(6..=8).contains(&digits) {
            return Err(CoreError::Totp("digits must be 6, 7 or 8".into()));
        }
        if !(1..=300).contains(&period) {
            return Err(CoreError::Totp(
                "period must be between 1 and 300 seconds".into(),
            ));
        }
        Ok(Self {
            secret: Zeroizing::new(secret),
            algorithm,
            digits,
            period,
            issuer: None,
            account: None,
        })
    }

    /// Accepts an `otpauth://totp/...` URI or a bare base32 secret.
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if input.len() > 10 && input[..10].eq_ignore_ascii_case("otpauth://") {
            Self::parse_uri(&input[10..])
        } else {
            Self::from_secret(decode_base32(input)?, TotpAlgorithm::Sha1, 6, 30)
        }
    }

    fn parse_uri(rest: &str) -> Result<Self> {
        let (kind, rest) = rest
            .split_once('/')
            .ok_or_else(|| CoreError::Totp("missing label".into()))?;
        match kind.to_ascii_lowercase().as_str() {
            "totp" => {}
            "hotp" => {
                return Err(CoreError::Totp(
                    "counter-based (HOTP) codes are not supported".into(),
                ));
            }
            other => return Err(CoreError::Totp(format!("unsupported OTP type '{other}'"))),
        }
        let (label, query) = rest.split_once('?').unwrap_or((rest, ""));
        let label = percent_decode(label)?;
        let (mut issuer, account) = match label.split_once(':') {
            Some((i, a)) => (Some(i.trim().to_string()), a.trim().to_string()),
            None => (None, label.trim().to_string()),
        };

        let (mut secret, mut algorithm, mut digits, mut period) =
            (None, TotpAlgorithm::Sha1, 6, 30);
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v)?;
            match k.to_ascii_lowercase().as_str() {
                "secret" => secret = Some(decode_base32(&v)?),
                "issuer" if !v.trim().is_empty() => issuer = Some(v.trim().to_string()),
                "algorithm" => {
                    algorithm = match v.to_ascii_uppercase().as_str() {
                        "SHA1" => TotpAlgorithm::Sha1,
                        "SHA256" => TotpAlgorithm::Sha256,
                        "SHA512" => TotpAlgorithm::Sha512,
                        other => {
                            return Err(CoreError::Totp(format!(
                                "unsupported algorithm '{other}'"
                            )));
                        }
                    }
                }
                "digits" => {
                    digits = v
                        .parse()
                        .map_err(|_| CoreError::Totp("bad digits".into()))?
                }
                "period" => {
                    period = v
                        .parse()
                        .map_err(|_| CoreError::Totp("bad period".into()))?
                }
                _ => {}
            }
        }
        let secret = secret.ok_or_else(|| CoreError::Totp("missing secret".into()))?;
        let mut spec = Self::from_secret(secret, algorithm, digits, period)?;
        spec.issuer = issuer.filter(|s| !s.is_empty());
        spec.account = Some(account).filter(|s| !s.is_empty());
        Ok(spec)
    }

    pub fn code_at(&self, unix_secs: u64) -> String {
        let counter = (unix_secs / self.period).to_be_bytes();
        let digest = match self.algorithm {
            TotpAlgorithm::Sha1 => mac::<sha1::Sha1>(&self.secret, &counter),
            TotpAlgorithm::Sha256 => mac::<sha2::Sha256>(&self.secret, &counter),
            TotpAlgorithm::Sha512 => mac::<sha2::Sha512>(&self.secret, &counter),
        };
        let offset = (digest[digest.len() - 1] & 0x0f) as usize;
        let bin = u32::from_be_bytes(digest[offset..offset + 4].try_into().expect("4 bytes"))
            & 0x7fff_ffff;
        let code = bin % 10u32.pow(self.digits);
        format!("{code:0width$}", width = self.digits as usize)
    }

    pub fn seconds_remaining(&self, unix_secs: u64) -> u64 {
        self.period - unix_secs % self.period
    }

    pub fn to_uri(&self) -> String {
        let account = self.account.as_deref().unwrap_or("");
        let label = match &self.issuer {
            Some(i) => format!("{}:{}", percent_encode(i), percent_encode(account)),
            None => percent_encode(account),
        };
        let mut uri = format!(
            "otpauth://totp/{label}?secret={}&algorithm={}&digits={}&period={}",
            BASE32_NOPAD.encode(&self.secret),
            self.algorithm.name(),
            self.digits,
            self.period
        );
        if let Some(i) = &self.issuer {
            uri.push_str("&issuer=");
            uri.push_str(&percent_encode(i));
        }
        uri
    }
}

impl fmt::Debug for TotpSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TotpSpec")
            .field("algorithm", &self.algorithm)
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("issuer", &self.issuer)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

fn mac<D: hmac::EagerHash>(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = <Hmac<D> as hmac::KeyInit>::new_from_slice(key).expect("HMAC accepts any key size");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

fn decode_base32(s: &str) -> Result<Vec<u8>> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.is_empty() {
        return Err(CoreError::Totp("secret is empty".into()));
    }
    BASE32_LENIENT
        .decode(cleaned.as_bytes())
        .map_err(|_| CoreError::Totp("secret is not valid base32".into()))
}

fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .ok_or_else(|| CoreError::Totp("bad percent-encoding".into()))?;
            out.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| CoreError::Totp("bad percent-encoding".into()))?,
            );
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| CoreError::Totp("label is not UTF-8".into()))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc(secret: &str, alg: TotpAlgorithm) -> TotpSpec {
        TotpSpec::from_secret(secret.as_bytes().to_vec(), alg, 8, 30).unwrap()
    }

    #[test]
    fn rfc6238_vectors() {
        let sha1 = rfc("12345678901234567890", TotpAlgorithm::Sha1);
        assert_eq!(sha1.code_at(59), "94287082");
        assert_eq!(sha1.code_at(1_111_111_109), "07081804");
        assert_eq!(sha1.code_at(1_111_111_111), "14050471");
        assert_eq!(sha1.code_at(1_234_567_890), "89005924");
        assert_eq!(sha1.code_at(2_000_000_000), "69279037");
        assert_eq!(sha1.code_at(20_000_000_000), "65353130");
        let sha256 = rfc("12345678901234567890123456789012", TotpAlgorithm::Sha256);
        assert_eq!(sha256.code_at(59), "46119246");
        let sha512 = rfc(
            "1234567890123456789012345678901234567890123456789012345678901234",
            TotpAlgorithm::Sha512,
        );
        assert_eq!(sha512.code_at(59), "90693936");
    }

    #[test]
    fn parses_otpauth_uri() {
        let spec = TotpSpec::parse(
            "otpauth://totp/ACME%20Co:john@example.com?secret=JBSWY3DPEHPK3PXP&issuer=ACME%20Co&algorithm=SHA1&digits=6&period=30",
        )
        .unwrap();
        assert_eq!(spec.issuer.as_deref(), Some("ACME Co"));
        assert_eq!(spec.account.as_deref(), Some("john@example.com"));
        assert_eq!(&spec.secret[..], b"Hello!\xDE\xAD\xBE\xEF");
        assert_eq!((spec.digits, spec.period), (6, 30));
        let again = TotpSpec::parse(&spec.to_uri()).unwrap();
        assert_eq!(again, spec);
    }

    #[test]
    fn parses_bare_secret_leniently() {
        let a = TotpSpec::parse("jbsw y3dp ehpk 3pxp").unwrap();
        let b = TotpSpec::parse("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(a.code_at(1_000), b.code_at(1_000));
        assert_eq!(a.code_at(1_000).len(), 6);
    }

    #[test]
    fn rejects_unsupported_input() {
        assert!(TotpSpec::parse("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1").is_err());
        assert!(TotpSpec::parse("otpauth://totp/x?issuer=nosecret").is_err());
        assert!(TotpSpec::parse("not base32 !!").is_err());
        assert!(TotpSpec::parse("otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&digits=12").is_err());
    }

    #[test]
    fn remaining_seconds() {
        let spec = TotpSpec::parse("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(spec.seconds_remaining(60), 30);
        assert_eq!(spec.seconds_remaining(89), 1);
    }
}

//! Which saved logins belong to a website, and which relying party ids a website may use.
//!
//! The origin always comes from the browser (never from page content). Matching is by
//! registrable domain using the public suffix list that is compiled in, so lookalikes such as
//! `example.com.evil.net` or `alice.github.io` vs `bob.github.io` never match.

use std::net::IpAddr;

use url::{Host, Url};

use crate::{CoreError, Result};

/// How well a saved URL matches a page. `Exact` (same host) ranks above `Domain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// Same registrable domain, different host (`login.example.com` for `example.com`).
    Domain,
    /// Same host.
    Exact,
}

/// A web origin we are willing to fill into or sign for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// `https`, or `http` for loopback addresses only.
    pub scheme: String,
    /// Lower-case ASCII (punycode) host without a trailing dot, or an IP literal.
    pub host: String,
    /// Explicit non-default port.
    pub port: Option<u16>,
    pub is_ip: bool,
}

impl Origin {
    /// Parses the origin of a page. Only https pages are accepted, plus http on loopback so
    /// local development works. Everything else (`file:`, `chrome:`, `about:`, plain http on
    /// the internet) is refused.
    pub fn parse(text: &str) -> Result<Self> {
        let url = Url::parse(text.trim())
            .map_err(|_| CoreError::Invalid("not a valid web address".into()))?;
        let (host, is_ip) = normalized_host(&url)
            .ok_or_else(|| CoreError::Invalid("the address has no host".into()))?;
        if !is_ip && !is_valid_hostname(&host) {
            return Err(CoreError::Invalid("not a valid host name".into()));
        }
        if url.port() == Some(0) {
            return Err(CoreError::Invalid("not a valid port".into()));
        }
        let loopback = is_loopback(&host);
        match url.scheme() {
            "https" => {}
            "http" if loopback => {}
            _ => {
                return Err(CoreError::Invalid(
                    "only https pages (and http on localhost) are supported".into(),
                ));
            }
        }
        Ok(Self {
            scheme: url.scheme().to_string(),
            host,
            port: url.port(),
            is_ip,
        })
    }

    /// `scheme://host[:port]`, the form WebAuthn puts in client data.
    pub fn serialized(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(port) => format!("{}://{host}:{port}", self.scheme),
            None => format!("{}://{host}", self.scheme),
        }
    }

    /// The registrable domain (`example.co.uk` for `www.example.co.uk`); `None` for IP
    /// addresses, single labels and bare public suffixes.
    pub fn registrable_domain(&self) -> Option<String> {
        if self.is_ip {
            return None;
        }
        psl::domain_str(&self.host).map(str::to_string)
    }
}

fn normalized_host(url: &Url) -> Option<(String, bool)> {
    match url.host()? {
        Host::Domain(d) => {
            // One trailing dot (the absolute form of a name) is dropped; more are left for
            // `is_valid_hostname` to refuse.
            let d = d.strip_suffix('.').unwrap_or(d).to_ascii_lowercase();
            (!d.is_empty()).then_some((d, false))
        }
        Host::Ipv4(ip) => Some((ip.to_string(), true)),
        Host::Ipv6(ip) => Some((ip.to_string(), true)),
    }
}

/// Whether a (lower-case, punycode) host name is well formed: at most 253 characters in labels of
/// 1 to 63 letters, digits, `-` or `_` that neither start nor end with `-`. Browsers hand us
/// hosts they have already parsed, so this is defence in depth against odd names that could
/// confuse the matching below.
fn is_valid_hostname(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// How a saved login's URL matches `origin`, if it does.
///
/// A saved URL without a scheme counts as https. A saved `http://` URL still matches the https
/// site (the usual upgrade), but a saved https URL never matches a plain-http page except on
/// loopback. A port in the saved URL must equal the page's port; without one any port matches.
pub fn match_login(saved_url: &str, origin: &Origin) -> Option<MatchKind> {
    let text = saved_url.trim();
    if text.is_empty() {
        return None;
    }
    let url = Url::parse(text)
        .ok()
        .filter(|u| u.host().is_some())
        .or_else(|| Url::parse(&format!("https://{text}")).ok())?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let (host, is_ip) = normalized_host(&url)?;
    if let Some(port) = url.port()
        && origin.port != Some(port)
    {
        return None;
    }
    if host == origin.host {
        return Some(MatchKind::Exact);
    }
    if is_ip || origin.is_ip {
        return None;
    }
    match (psl::domain_str(&host), psl::domain_str(&origin.host)) {
        (Some(a), Some(b)) if a == b => Some(MatchKind::Domain),
        _ => None,
    }
}

/// The best match over all of an item's saved URLs.
pub fn best_match<S: AsRef<str>>(saved_urls: &[S], origin: &Origin) -> Option<MatchKind> {
    saved_urls
        .iter()
        .filter_map(|u| match_login(u.as_ref(), origin))
        .max()
}

/// Whether `rp_id` may be used on `origin` (WebAuthn's rule): the origin's host is the id or a
/// subdomain of it, and the id is not a public suffix or an IP address. `localhost` is allowed
/// for local development.
pub fn validate_rp_id(rp_id: &str, origin: &Origin) -> Result<()> {
    let bad = |why: &str| {
        Err(CoreError::Invalid(format!(
            "relying party id {rp_id:?}: {why}"
        )))
    };
    if rp_id.is_empty()
        || rp_id != rp_id.to_ascii_lowercase()
        || rp_id.ends_with('.')
        || !rp_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return bad("not a valid domain");
    }
    if origin.is_ip {
        return bad("a page opened by IP address cannot use a relying party id");
    }
    if rp_id.parse::<IpAddr>().is_ok()
        || rp_id
            .rsplit('.')
            .next()
            .is_some_and(|last| last.chars().all(|c| c.is_ascii_digit()))
    {
        return bad("an IP address cannot be a relying party id");
    }
    let within = origin.host == rp_id || origin.host.ends_with(&format!(".{rp_id}"));
    if !within {
        return bad("does not belong to this site");
    }
    if rp_id == "localhost" || rp_id.ends_with(".localhost") {
        return Ok(());
    }
    if psl::domain_str(rp_id).is_none() {
        return bad("is a public suffix");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(s: &str) -> Origin {
        Origin::parse(s).unwrap()
    }

    #[test]
    fn origins_are_normalised_and_restricted() {
        let o = origin("HTTPS://Login.Example.COM:8443/path?q=1#x");
        assert_eq!(o.host, "login.example.com");
        assert_eq!(o.port, Some(8443));
        assert_eq!(o.serialized(), "https://login.example.com:8443");
        assert_eq!(origin("https://example.com:443/").port, None);
        assert_eq!(
            origin("https://bücher.example/").host,
            "xn--bcher-kva.example"
        );
        assert!(Origin::parse("http://example.com/").is_err());
        assert!(Origin::parse("http://localhost:3000/").is_ok());
        assert!(Origin::parse("http://127.0.0.1:3000/").is_ok());
        assert!(Origin::parse("http://[::1]:3000/").is_ok());
        for refused in [
            "file:///etc/passwd",
            "chrome://settings",
            "about:blank",
            "data:text/html,x",
            "javascript:alert(1)",
            "chrome-extension://abc/page.html",
            "not a url",
            "",
        ] {
            assert!(Origin::parse(refused).is_err(), "{refused}");
        }
        assert_eq!(
            origin("https://www.example.co.uk")
                .registrable_domain()
                .as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(origin("http://localhost").registrable_domain(), None);
    }

    #[test]
    fn saved_logins_match_by_registrable_domain() {
        let page = origin("https://login.example.com/signin");
        let cases: &[(&str, Option<MatchKind>)] = &[
            ("https://login.example.com", Some(MatchKind::Exact)),
            ("login.example.com", Some(MatchKind::Exact)),
            ("https://example.com", Some(MatchKind::Domain)),
            ("example.com/account", Some(MatchKind::Domain)),
            ("http://example.com", Some(MatchKind::Domain)),
            ("https://www.example.com:443", Some(MatchKind::Domain)),
            ("https://example.com:8443", None),
            ("https://example.org", None),
            ("https://example.com.evil.net", None),
            ("https://evilexample.com", None),
            ("https://notexample.com", None),
            ("ftp://example.com", None),
            ("", None),
            ("   ", None),
        ];
        for (saved, expected) in cases {
            assert_eq!(match_login(saved, &page), *expected, "{saved}");
        }
    }

    #[test]
    fn private_suffix_tenants_do_not_share_logins() {
        let alice = origin("https://alice.github.io");
        assert_eq!(
            match_login("https://alice.github.io", &alice),
            Some(MatchKind::Exact)
        );
        assert_eq!(match_login("https://bob.github.io", &alice), None);
        assert_eq!(match_login("https://github.io", &alice), None);
        assert_eq!(
            match_login(
                "https://www.example.co.uk",
                &origin("https://shop.example.co.uk")
            ),
            Some(MatchKind::Domain)
        );
        assert_eq!(
            match_login("https://other.co.uk", &origin("https://shop.example.co.uk")),
            None
        );
    }

    #[test]
    fn ip_and_localhost_need_an_exact_host() {
        let ip = origin("https://192.168.1.10");
        assert_eq!(
            match_login("https://192.168.1.10", &ip),
            Some(MatchKind::Exact)
        );
        assert_eq!(match_login("192.168.1.10:8443", &ip), None);
        assert_eq!(match_login("https://192.168.1.11", &ip), None);
        let local = origin("http://localhost:3000");
        assert_eq!(match_login("localhost", &local), Some(MatchKind::Exact));
        assert_eq!(
            match_login("localhost:3000", &local),
            Some(MatchKind::Exact)
        );
        assert_eq!(match_login("localhost:4000", &local), None);
        assert_eq!(match_login("https://example.localhost", &local), None);
    }

    #[test]
    fn best_match_prefers_exact_hosts() {
        let page = origin("https://login.example.com");
        let urls = [
            "https://example.com",
            "https://login.example.com",
            "https://other.org",
        ];
        assert_eq!(best_match(&urls, &page), Some(MatchKind::Exact));
        assert_eq!(best_match(&urls[2..], &page), None);
        assert_eq!(best_match::<&str>(&[], &page), None);
    }

    #[test]
    fn relying_party_ids_follow_the_webauthn_rule() {
        let page = origin("https://login.example.com");
        assert!(validate_rp_id("login.example.com", &page).is_ok());
        assert!(validate_rp_id("example.com", &page).is_ok());
        for refused in [
            "com",
            "other.com",
            "evil.example.com",
            "xample.com",
            "login.example.com.evil.net",
            "Example.com",
            "example.com.",
            "",
            "exa mple.com",
            "example.com:443",
            "https://example.com",
        ] {
            assert!(validate_rp_id(refused, &page).is_err(), "{refused}");
        }
        // Public suffixes can never be a relying party id, even if the origin is inside one.
        let uk = origin("https://shop.example.co.uk");
        assert!(validate_rp_id("example.co.uk", &uk).is_ok());
        assert!(validate_rp_id("co.uk", &uk).is_err());
        let tenant = origin("https://alice.github.io");
        assert!(validate_rp_id("alice.github.io", &tenant).is_ok());
        assert!(validate_rp_id("github.io", &tenant).is_err());
        // Local development.
        let local = origin("http://localhost:8080");
        assert!(validate_rp_id("localhost", &local).is_ok());
        assert!(validate_rp_id("example.com", &local).is_err());
        let ip = origin("https://192.168.1.10");
        assert!(validate_rp_id("192.168.1.10", &ip).is_err());
    }

    #[test]
    fn no_relying_party_id_is_valid_on_an_ip_address_origin() {
        // The numeric tail of an address is not a registrable domain (`1.10` for 192.168.1.10 was
        // accepted, and shared with 10.0.1.10).
        let v4 = origin("https://192.168.1.10");
        for id in [
            "1.10",
            "168.1.10",
            "10",
            "1",
            "192.168.1.10",
            "0.1.10",
            "localhost",
            "example.com",
            "192.168.1.10.",
            "",
        ] {
            assert!(validate_rp_id(id, &v4).is_err(), "{id}");
        }
        assert!(validate_rp_id("1.10", &origin("https://10.0.1.10")).is_err());
        let loopback = origin("http://127.0.0.1:3000");
        for id in ["0.0.1", "1", "127.0.0.1", "localhost"] {
            assert!(validate_rp_id(id, &loopback).is_err(), "{id}");
        }
        let v6 = origin("http://[::1]:3000");
        for id in ["1", "::1", "[::1]", "localhost"] {
            assert!(validate_rp_id(id, &v6).is_err(), "{id}");
        }
        let v6 = origin("https://[2001:db8::1]");
        for id in ["1", "db8::1", "2001:db8::1", "example.com"] {
            assert!(validate_rp_id(id, &v6).is_err(), "{id}");
        }
        // A number-only last label is not a domain either, whatever the origin.
        assert!(validate_rp_id("example.123", &origin("https://www.example.com")).is_err());
    }

    #[test]
    fn relying_party_id_edge_cases() {
        // A trailing dot in the origin is dropped; in the id it is refused.
        let dotted = origin("https://www.example.com.");
        assert!(validate_rp_id("example.com", &dotted).is_ok());
        assert!(validate_rp_id("example.com.", &dotted).is_err());
        assert!(validate_rp_id("EXAMPLE.com", &dotted).is_err());
        // Internationalised names work in their punycode form only.
        let idn = origin("https://www.bücher.example");
        assert!(validate_rp_id("xn--bcher-kva.example", &idn).is_ok());
        assert!(validate_rp_id("bücher.example", &idn).is_err());
        assert!(validate_rp_id("xn--bcher-kva.example.", &idn).is_err());
        // Lookalikes of a letter are not the letter.
        let page = origin("https://example.com");
        assert!(validate_rp_id("xn--exmple-cua.com", &page).is_err());
    }

    #[test]
    fn malformed_hosts_and_port_zero_are_refused() {
        for refused in [
            "https://a..b.com",
            "https://-bad-.com",
            "https://bad-.example.com",
            "https://-bad.example.com",
            "https://example.com:0",
            "https://example.com:0/path",
            "https://.example.com",
            "https://exa!mple.com",
        ] {
            assert!(Origin::parse(refused).is_err(), "{refused}");
        }
        let long_label = format!("https://{}.example.com", "a".repeat(64));
        assert!(Origin::parse(&long_label).is_err());
        let long_host = format!("https://{}.com", ["abcdefgh"; 40].join("."));
        assert!(Origin::parse(&long_host).is_err());
        // Ordinary hosts still parse.
        for fine in [
            "https://example.com",
            "https://my-site.example.com:8443",
            "https://my_service.example.com",
            "https://xn--bcher-kva.example",
            "https://www.example.com.",
            "https://1password.com",
            "http://localhost:3000",
            "https://[2001:db8::1]/",
            "https://192.168.1.10/",
        ] {
            assert!(Origin::parse(fine).is_ok(), "{fine}");
        }
        let max = format!("https://{}.com", "a".repeat(63));
        assert!(Origin::parse(&max).is_ok());
    }
}

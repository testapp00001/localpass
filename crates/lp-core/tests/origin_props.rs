//! Property tests for origin parsing, login matching and relying-party id validation (phase 5.1).
//! The origin comes from the browser or the OS, but it is still the line between "this is your
//! bank" and "this is not", so the rules are checked against an oracle built straight on the `url`
//! and `psl` crates, and against a list of lookalike constructions.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::{Duration, Instant};

use lp_core::origin::{MatchKind, Origin, best_match, match_login, validate_rp_id};
use proptest::collection::vec;
use proptest::prelude::*;

/// 128 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

fn label() -> impl Strategy<Value = String> {
    "[a-z0-9]([a-z0-9-]{0,10}[a-z0-9])?"
}

/// Public suffixes: ICANN, multi-label, private-section, wildcard-ish and unknown ones.
fn suffix() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("com".to_string()),
        Just("org".to_string()),
        Just("co.uk".to_string()),
        Just("com.au".to_string()),
        Just("github.io".to_string()),
        Just("blogspot.com".to_string()),
        Just("herokuapp.com".to_string()),
        Just("s3.amazonaws.com".to_string()),
        Just("appspot.com".to_string()),
        Just("tokyo.jp".to_string()),
        "[a-z]{2,8}",
    ]
}

/// A well-formed host name: zero to three labels in front of a public suffix.
fn host() -> impl Strategy<Value = String> {
    (vec(label(), 0..4), suffix()).prop_map(|(labels, suffix)| {
        let mut parts = labels;
        parts.push(suffix);
        parts.join(".")
    })
}

/// A host with at least one label in front of a suffix, so it has a registrable domain.
fn registrable_host() -> impl Strategy<Value = String> {
    (vec(label(), 1..4), suffix())
        .prop_map(|(labels, suffix)| format!("{}.{suffix}", labels.join(".")))
}

fn parse(text: &str) -> Option<Origin> {
    Origin::parse(text).ok()
}

fn https(host: &str) -> Option<Origin> {
    parse(&format!("https://{host}"))
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn origin_parse_never_panics(
        any_text in any::<String>(),
        urlish in r"(https?|HTTPS|ftp|file|data|javascript|about|chrome-extension)?:?/{0,3}[^ ]{0,60}",
        mixed in r"https?://([a-zA-Z0-9@:%._\[\]-]|\\u\{[0-9a-f]{1,4}\}|[\u{80}-\u{2fff}]){0,40}(:[0-9]{0,7})?(/.{0,10})?",
    ) {
        let _ = Origin::parse(&any_text);
        let _ = Origin::parse(&urlish);
        let _ = Origin::parse(&mixed);
        let origin = Origin::parse("https://example.com").unwrap();
        for saved in [&any_text, &urlish, &mixed] {
            let _ = match_login(saved, &origin);
        }
        let _ = best_match(&[&any_text, &urlish, &mixed], &origin);
    }

    #[test]
    fn origin_ok_is_normalised_and_round_trips(
        scheme in prop_oneof![Just("https"), Just("http"), Just("HTTPS"), Just("ftp")],
        userinfo in prop::option::of("[a-zA-Z0-9%:._~!$&'()*+,;=-]{0,12}"),
        host in prop_oneof![
            host(),
            host().prop_map(|h| h.to_uppercase()),
            host().prop_map(|h| format!("{h}.")),
            Just("localhost".to_string()),
            Just("foo.localhost".to_string()),
            any::<u32>().prop_map(|n| Ipv4Addr::from(n).to_string()),
            any::<u32>().prop_map(|n| n.to_string()),
            any::<u128>().prop_map(|n| format!("[{}]", Ipv6Addr::from(n))),
            "[a-zA-Z0-9.%-]{1,30}",
        ],
        port in prop::option::of(0u32..70000),
        tail in prop_oneof![Just(""), Just("/"), Just("/a/b?q=1#f"), Just("?x"), Just("#y")],
    ) {
        let mut text = format!("{scheme}://");
        if let Some(userinfo) = &userinfo {
            text.push_str(userinfo);
            text.push('@');
        }
        text.push_str(&host);
        if let Some(port) = port {
            text.push_str(&format!(":{port}"));
        }
        text.push_str(tail);

        if let Ok(origin) = Origin::parse(&text) {
            prop_assert!(origin.scheme == "https" || origin.scheme == "http");
            prop_assert!(origin.host.is_ascii());
            prop_assert_eq!(origin.host.to_ascii_lowercase(), origin.host.clone());
            prop_assert!(!origin.host.is_empty());
            prop_assert!(!origin.host.ends_with('.'));
            prop_assert!(!origin.host.contains(['@', '/', '\\', ' ', '%', '?', '#', '[', ']']));
            prop_assert_eq!(origin.host.contains(':'), origin.is_ip && origin.host.parse::<Ipv6Addr>().is_ok());
            prop_assert_ne!(origin.port, Some(0));
            if origin.scheme == "http" {
                let loopback = origin.host == "localhost"
                    || origin.host.ends_with(".localhost")
                    || origin.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
                prop_assert!(loopback, "plain http accepted for {}", origin.host);
            }
            if !origin.is_ip {
                prop_assert!(origin.host.len() <= 253);
                for part in origin.host.split('.') {
                    prop_assert!((1..=63).contains(&part.len()));
                    prop_assert!(!part.starts_with('-') && !part.ends_with('-'));
                }
            } else {
                prop_assert!(origin.host.parse::<std::net::IpAddr>().is_ok());
            }
            // Serializing and parsing again gives the same origin.
            prop_assert_eq!(Origin::parse(&origin.serialized()).ok(), Some(origin.clone()));
            // Neither the user info nor the path ever changed the answer.
            let plain = format!("{}://{}", origin.scheme, origin.serialized().split("://").nth(1).unwrap());
            prop_assert_eq!(Origin::parse(&plain).ok(), Some(origin));
        }
    }

    #[test]
    fn case_dots_default_ports_and_paths_do_not_change_the_origin(host in host(), path in "(/[a-z0-9]{0,6}){0,3}") {
        let Some(plain) = https(&host) else { return Ok(()); };
        for text in [
            format!("https://{}", host.to_uppercase()),
            format!("HTTPS://{host}."),
            format!("https://{host}:443"),
            format!("https://{host}{path}?q#f"),
            format!("  https://{host}/  "),
            format!("https://user:pass@{host}/"),
        ] {
            prop_assert_eq!(Origin::parse(&text).ok(), Some(plain.clone()), "{}", text);
        }
        prop_assert_eq!(plain.registrable_domain(), psl::domain_str(&plain.host).map(str::to_string));
    }

    #[test]
    fn userinfo_cannot_change_host(
        real in host(),
        userinfo in "[a-zA-Z0-9%:._~!$&'()*+,;=@-]{0,24}",
        evil in registrable_host(),
    ) {
        let Some(plain) = https(&real) else { return Ok(()); };
        // Whatever sits in front of the last `@` is user info, even when it looks like a host.
        for text in [
            format!("https://{userinfo}@{real}/"),
            format!("https://{evil}@{real}/"),
            format!("https://{evil}:80@{real}"),
            format!("https://{evil}%40{real}@{real}"),
        ] {
            if let Ok(origin) = Origin::parse(&text) {
                prop_assert_eq!(origin.host, plain.host.clone(), "{}", text);
            }
        }
        // The other way round: the host before `@` is not a host.
        if let Some(origin) = https(&format!("{real}@{evil}")) {
            prop_assert_eq!(origin.host, evil);
        }
    }

    /// The login a page may get is decided by the host name alone: same host, or the same
    /// registrable domain according to the public suffix list.
    #[test]
    fn match_login_agrees_with_psl_oracle(
        base in registrable_host(),
        saved_prefix in vec(label(), 0..3),
        page_prefix in vec(label(), 0..3),
        unrelated in registrable_host(),
        use_unrelated in any::<bool>(),
        saved_form in 0usize..4,
    ) {
        let saved_host = [saved_prefix, vec![base.clone()]].concat().join(".");
        let page_host = if use_unrelated {
            unrelated
        } else {
            [page_prefix, vec![base]].concat().join(".")
        };
        let Some(page) = https(&page_host) else { return Ok(()); };
        let Some(saved_origin) = https(&saved_host) else { return Ok(()); };
        let saved = match saved_form {
            0 => format!("https://{saved_host}"),
            1 => saved_host.clone(),
            2 => format!("http://{}/login?next=/x", saved_host.to_uppercase()),
            _ => format!("https://me@{saved_host}/path"),
        };
        let expected = if saved_origin.host == page.host {
            Some(MatchKind::Exact)
        } else {
            match (psl::domain_str(&saved_origin.host), psl::domain_str(&page.host)) {
                (Some(a), Some(b)) if a == b => Some(MatchKind::Domain),
                _ => None,
            }
        };
        prop_assert_eq!(match_login(&saved, &page), expected, "{} vs {}", saved, page_host);
        prop_assert_eq!(best_match(&["https://unrelated.invalid", saved.as_str()], &page), expected);
    }

    /// Hosts built to look like `base` to a human or to a sloppy `contains` check never match.
    #[test]
    fn lookalike_hosts_never_match(
        base in registrable_host(),
        evil in registrable_host(),
        stuck in "[a-z0-9]{1,6}",
        letter in 0usize..9,
    ) {
        let Some(page) = https(&base) else { return Ok(()); };
        // A generated host can itself be a public suffix (`a.r.appspot.com`): no domain to imitate.
        let Some(domain) = psl::domain_str(&page.host).map(str::to_string) else { return Ok(()); };
        let base_domain = Some(domain.clone());
        prop_assume!(psl::domain_str(&evil) != base_domain.as_deref());
        // A look-alike letter from another alphabet (never one that IDNA folds back to ASCII).
        let confusable = ["а", "е", "о", "р", "с", "х", "у", "і", "ӏ"][letter];
        let from = ['a', 'e', 'o', 'p', 'c', 'x', 'y', 'i', 'l'][letter];
        let swapped = domain.replacen(from, confusable, 1);
        let mut attacks = vec![
            format!("https://{base}.{evil}"),
            format!("https://{stuck}{domain}"),
            format!("https://{base}@{evil}"),
            format!("https://{base}:443@{evil}/"),
            format!("https://{evil}/{base}"),
            format!("https://{evil}?{base}"),
            format!("https://{evil}#{base}"),
            format!("https://{evil}\\{base}"),
            format!("https://{base}%2f.{evil}"),
            format!("https://{base}%00.{evil}"),
            format!("{base}.{evil}"),
            format!("{evil}/{base}"),
            format!("{evil} {base}"),
            format!("javascript://{base}/%0a//{evil}"),
        ];
        if swapped != domain {
            attacks.push(format!("https://{swapped}"));
        }
        for saved in &attacks {
            prop_assert_eq!(match_login(saved, &page), None, "{} matched {}", saved, base);
        }
        // And the same lookalikes as the PAGE against a saved login for `base`.
        for attack in &attacks {
            if let Some(origin) = parse(attack)
                && psl::domain_str(&origin.host) != base_domain.as_deref()
            {
                prop_assert_eq!(match_login(&format!("https://{base}"), &origin), None);
            }
        }
    }

    #[test]
    fn ip_literals_match_only_canonical_equal(
        a in any::<u32>(),
        b in any::<u32>(),
        same in any::<bool>(),
        wide in any::<u128>(),
        wide_other in any::<u128>(),
        domain in registrable_host(),
    ) {
        let b = if same { a } else { b };
        let page = https(&Ipv4Addr::from(b).to_string()).unwrap();
        let forms = [
            Ipv4Addr::from(a).to_string(),
            a.to_string(),
            format!("0x{a:x}"),
            format!("{}.{}", a >> 24, a & 0x00ff_ffff),
        ];
        for form in &forms {
            let expected = (a == b).then_some(MatchKind::Exact);
            prop_assert_eq!(match_login(&format!("https://{form}/"), &page), expected, "{} vs {}", form, page.host);
            prop_assert_eq!(match_login(form, &page), expected, "{} vs {}", form, page.host);
        }
        // An IP address inside a domain name, or a domain beside an IP page, is not the IP.
        prop_assert_eq!(match_login(&format!("https://{}.{domain}", Ipv4Addr::from(b)), &page), None);
        prop_assert_eq!(match_login(&format!("https://{domain}"), &page), None);
        prop_assert_eq!(match_login(&format!("https://{}", Ipv4Addr::from(a ^ 1)), &page), None);
        // An IP address saved, a domain page.
        let domain_page = https(&domain).unwrap();
        prop_assert_eq!(match_login(&format!("https://{}", Ipv4Addr::from(a)), &domain_page), None);

        // IPv6, with both spellings of the hex digits.
        let six = Ipv6Addr::from(wide);
        let page6 = https(&format!("[{}]", Ipv6Addr::from(if same { wide } else { wide_other }))).unwrap();
        let expected6 = (same || wide == wide_other).then_some(MatchKind::Exact);
        for form in [format!("[{six}]"), format!("[{}]", six.to_string().to_uppercase())] {
            prop_assert_eq!(match_login(&format!("https://{form}/x"), &page6), expected6);
        }
    }

    /// V22, pinned on purpose: a saved URL without a port matches every port; one with a port
    /// must equal the page's.
    #[test]
    fn port_rule_is_pinned(host in registrable_host(), saved_port in 1u16..=65535, page_port in 1u16..=65535) {
        let Some(bare) = https(&host) else { return Ok(()); };
        let Some(page) = https(&format!("{host}:{page_port}")) else { return Ok(()); };
        // Without a port in the saved URL every page port matches, and ranks Exact.
        prop_assert_eq!(match_login(&format!("https://{host}"), &page), Some(MatchKind::Exact));
        prop_assert_eq!(match_login(&format!("https://{host}"), &bare), Some(MatchKind::Exact));
        // With a port the page's port must be the same one.
        let saved = format!("https://{host}:{saved_port}");
        let default_port = saved_port == 443;
        let expected = (default_port || saved_port == page_port).then_some(MatchKind::Exact);
        prop_assert_eq!(match_login(&saved, &page), expected, "{} vs port {}", saved, page_port);
        if !default_port {
            prop_assert_eq!(match_login(&saved, &bare), None);
        }
    }

    #[test]
    fn empty_labels_hyphens_and_long_names_are_refused(
        a in label(), b in label(), tld in "[a-z]{2,6}", long in 64usize..120,
    ) {
        for text in [
            format!("https://{a}..{b}.{tld}"),
            format!("https://.{a}.{tld}"),
            format!("https://{a}.{tld}.."),
            format!("https://-{a}.{tld}"),
            format!("https://{a}-.{tld}"),
            format!("https://{a}.{b}-.{tld}"),
            format!("https://{}.{tld}", "a".repeat(long)),
            format!("https://{}.{tld}", vec!["abcdefghi"; 30].join(".")),
            format!("https://{a}.{tld}:0"),
            "http://localhost:0".to_string(),
        ] {
            prop_assert!(Origin::parse(&text).is_err(), "{} was accepted", text);
        }
    }

    #[test]
    fn rp_id_ok_implies_the_webauthn_rule(
        host in host(),
        pick in 0usize..6,
        random_id in "[a-zA-Z0-9.-]{0,30}",
        variant in 0usize..6,
    ) {
        let Some(origin) = https(&host) else { return Ok(()); };
        let suffixes = dotted_suffixes(&origin.host);
        let mut id = suffixes[pick % suffixes.len()].clone();
        id = match variant {
            0 => id,
            1 => id.to_uppercase(),
            2 => format!("{id}."),
            3 => format!(".{id}"),
            4 => random_id,
            _ => id.replace('a', "_"),
        };
        if validate_rp_id(&id, &origin).is_ok() {
            prop_assert!(!id.is_empty());
            prop_assert_eq!(id.to_ascii_lowercase(), id.clone());
            prop_assert!(!id.ends_with('.'));
            prop_assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'));
            prop_assert!(!origin.is_ip);
            prop_assert!(id.parse::<std::net::IpAddr>().is_err());
            let last = id.rsplit('.').next().unwrap();
            prop_assert!(!last.chars().all(|c| c.is_ascii_digit()), "numeric last label in {}", id);
            prop_assert!(origin.host == id || origin.host.ends_with(&format!(".{id}")), "{} is not a suffix of {}", id, origin.host);
            let local = id == "localhost" || id.ends_with(".localhost");
            prop_assert!(local || psl::domain_str(&id).is_some(), "{} is a public suffix", id);
        }
    }

    #[test]
    fn rp_id_must_be_a_suffix_of_the_host(host in registrable_host(), other in registrable_host()) {
        let Some(origin) = https(&host) else { return Ok(()); };
        prop_assume!(origin.host != other && !origin.host.ends_with(&format!(".{other}")));
        prop_assert!(validate_rp_id(&other, &origin).is_err());
        // Not on a label boundary.
        let cut = &origin.host[1..];
        prop_assert!(validate_rp_id(cut, &origin).is_err(), "{} for {}", cut, origin.host);
        // The registrable domain itself, and every name between it and the host, are fine.
        let Some(domain) = psl::domain_str(&origin.host).map(str::to_string) else { return Ok(()); };
        prop_assert!(validate_rp_id(&domain, &origin).is_ok(), "{} for {}", domain, origin.host);
    }

    /// V3, fixed: no relying party id is valid for a page opened by IP address.
    #[test]
    fn rp_id_is_never_valid_for_ip_origins(a in any::<u32>(), wide in any::<u128>()) {
        let v4 = Ipv4Addr::from(a).to_string();
        let origin = https(&v4).unwrap();
        for id in dotted_suffixes(&v4) {
            prop_assert!(validate_rp_id(&id, &origin).is_err(), "{} accepted for {}", id, v4);
        }
        let v6 = Ipv6Addr::from(wide).to_string();
        let origin = https(&format!("[{v6}]")).unwrap();
        for id in v6.split(':').filter(|g| !g.is_empty()).chain([v6.as_str()]) {
            prop_assert!(validate_rp_id(id, &origin).is_err(), "{} accepted for {}", id, v6);
        }
        // Loopback by IP is the same.
        let loopback = https("127.0.0.1").unwrap();
        prop_assert!(validate_rp_id("127.0.0.1", &loopback).is_err());
        prop_assert!(validate_rp_id("0.1", &loopback).is_err());
    }

    #[test]
    fn rp_id_rejects_public_suffixes(host in registrable_host()) {
        let Some(origin) = https(&host) else { return Ok(()); };
        let suffix = psl::suffix_str(&origin.host).unwrap().to_string();
        prop_assert!(validate_rp_id(&suffix, &origin).is_err(), "{} accepted as a suffix of {}", suffix, host);
        // Every shorter tail is a public suffix, a TLD, or the registrable domain.
        for id in dotted_suffixes(&origin.host) {
            let result = validate_rp_id(&id, &origin);
            let registrable = psl::domain_str(&id).is_some();
            prop_assert_eq!(result.is_ok(), registrable, "{} for {}", id, host);
        }
    }
}

/// `a.b.c` gives `c`, `b.c`, `a.b.c`.
fn dotted_suffixes(host: &str) -> Vec<String> {
    let labels: Vec<&str> = host.split('.').collect();
    (0..labels.len())
        .rev()
        .map(|from| labels[from..].join("."))
        .collect()
}

#[test]
fn rp_id_numeric_suffix_regression() {
    // V3: "1.10" and "168.1.10" were accepted for an IP origin, and "1.10" fits two of them.
    for origin in ["https://192.168.1.10", "https://10.0.1.10", "https://[::1]"] {
        let origin = Origin::parse(origin).unwrap();
        for id in [
            "1.10",
            "168.1.10",
            "10",
            "192.168.1.10",
            "1",
            "0.1",
            "::1",
            "1",
        ] {
            assert!(validate_rp_id(id, &origin).is_err(), "{id} for {origin:?}");
        }
    }
}

#[test]
fn rp_id_edge_table() {
    let page = Origin::parse("https://login.shop.example.co.uk").unwrap();
    let ok = |id: &str| validate_rp_id(id, &page).is_ok();
    assert!(ok("login.shop.example.co.uk"));
    assert!(ok("shop.example.co.uk"));
    assert!(ok("example.co.uk"));
    for bad in [
        "",
        ".",
        "co.uk",
        "uk",
        "co",
        "example.co",
        "xample.co.uk",
        "ample.co.uk",
        ".example.co.uk",
        "example.co.uk.",
        "EXAMPLE.co.uk",
        "example.co.uk ",
        " example.co.uk",
        "example..co.uk",
        "example.co.uk\0",
        "exаmple.co.uk",
        "*.example.co.uk",
        "example.co.uk/",
        "ex_ample.co.uk",
        "other.example.co.uk",
        "example.com",
        "shop.example.co.uk:443",
    ] {
        assert!(!ok(bad), "{bad:?} accepted");
    }
    // Local development.
    let local = Origin::parse("http://localhost:3000").unwrap();
    assert!(validate_rp_id("localhost", &local).is_ok());
    assert!(validate_rp_id("local", &local).is_err());
    let sub = Origin::parse("http://app.localhost").unwrap();
    assert!(validate_rp_id("app.localhost", &sub).is_ok());
    assert!(validate_rp_id("localhost", &sub).is_ok());
    // Private suffixes (the PSL's private section) are public suffixes too.
    for (host, suffix) in [
        ("https://alice.github.io", "github.io"),
        ("https://x.blogspot.com", "blogspot.com"),
        ("https://x.s3.amazonaws.com", "s3.amazonaws.com"),
    ] {
        let origin = Origin::parse(host).unwrap();
        assert!(validate_rp_id(suffix, &origin).is_err(), "{suffix}");
        assert!(validate_rp_id(&host["https://".len()..], &origin).is_ok());
    }
}

#[test]
fn known_attacks_do_not_match() {
    let page = Origin::parse("https://example.com").unwrap();
    for saved in [
        "https://example.com.evil.net",
        "https://evilexample.com",
        "https://examp1e.com",
        "https://example.com@evil.com",
        "https://evil.com/example.com",
        "https://evil.com#example.com",
        "https://evil.com\\example.com",
        "https://example.com%2f.evil.com",
        "https://exаmple.com",
        "https://example.com.",
        "https://example.co",
        "https://example.org",
        "https://github.io",
        "https://com",
        "ftp://example.com",
        "file:///example.com",
        "javascript:alert('example.com')",
        "data:text/html,example.com",
        "",
        " ",
        "https://",
        "https://:443",
    ] {
        let result = match_login(saved, &page);
        // The one name that is the same site written another way: the absolute form.
        let expected = (saved == "https://example.com.").then_some(MatchKind::Exact);
        assert_eq!(result, expected, "{saved:?}");
    }
    // Sibling private-suffix tenants stay apart.
    let alice = Origin::parse("https://alice.github.io").unwrap();
    assert_eq!(match_login("https://bob.github.io", &alice), None);
    assert_eq!(
        match_login("https://www.alice.github.io", &alice),
        Some(MatchKind::Domain)
    );
}

#[test]
fn huge_inputs_return_fast() {
    let started = Instant::now();
    let origin = Origin::parse("https://example.com").unwrap();
    let inputs = [
        format!("https://{}.com", "a".repeat(100_000)),
        format!("https://{}com", "a.".repeat(60_000)),
        format!("https://{}@example.com", "u".repeat(1_000_000)),
        format!("https://example.com/{}", "p".repeat(1_000_000)),
        "%".repeat(1_000_000),
        "\u{202e}".repeat(300_000),
        format!("https://{}", "xn--".repeat(50_000)),
        format!("https://[{}]", ":".repeat(100_000)),
        "[".repeat(200_000),
    ];
    for input in &inputs {
        let _ = Origin::parse(input);
        let _ = match_login(input, &origin);
        let _ = best_match(&[input.as_str()], &origin);
    }
    for id in [
        "a".repeat(100_000),
        "a.".repeat(50_000),
        "é".repeat(100_000),
    ] {
        assert!(validate_rp_id(&id, &origin).is_err());
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
}

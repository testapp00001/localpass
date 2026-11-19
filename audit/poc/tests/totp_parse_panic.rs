//! L-1: `TotpSpec::parse` slices the input at byte 10 without checking that byte 10 is a
//! character boundary (crates/lp-core/src/totp.rs:79). Text longer than 10 bytes whose 10th byte
//! falls inside a multi-byte character makes it panic instead of returning an error.
//!
//! This test FAILS at the audited commit (the call panics). It passes once `parse` returns `Err`.

use lp_core::totp::TotpSpec;

#[test]
fn parse_returns_an_error_instead_of_panicking_on_non_ascii_input() {
    // Nine ASCII characters, then a two-byte character: 11 bytes, and byte 10 is mid-character.
    let input = "123456789\u{e9}";
    let outcome = std::panic::catch_unwind(|| TotpSpec::parse(input).is_err());
    assert_eq!(
        outcome.ok(),
        Some(true),
        "TotpSpec::parse panicked on {input:?}; it must return an error"
    );
}

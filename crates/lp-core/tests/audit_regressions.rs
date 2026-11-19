//! Maintained audit regressions. All inputs are synthetic and require no vault.
use lp_core::totp::TotpSpec;
use proptest::prelude::*;

#[test]
fn totp_unicode_boundary_returns_error() {
    assert!(TotpSpec::parse("123456789\u{e9}").is_err());
}

proptest! {
    #[test]
    fn totp_arbitrary_unicode_never_panics(input in any::<String>()) {
        let _ = TotpSpec::parse(&input);
    }
}

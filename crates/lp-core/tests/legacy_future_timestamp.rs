//! B-1 (logic): a value written with a timestamp far in the future wins last-writer-wins for
//! good. `HlcClock::observe` (crates/lp-core/src/hlc.rs:53) rightly refuses to follow a remote
//! clock more than 24 h ahead, but `VaultState::apply` (crates/lp-core/src/materialize.rs:143)
//! still orders by that timestamp, so every later edit made with a correct clock loses.
//!
//! This test FAILS at the audited commit: the later edit does not become the current value.

use lp_core::hlc::{Hlc, HlcClock};
use lp_core::ids::{DeviceId, ItemId};
use lp_core::materialize::VaultState;
use lp_core::model::{DataOp, FieldSet, FieldValue, keys};

fn set(item: ItemId, hlc: Hlc, value: &str) -> DataOp {
    DataOp::ItemSet {
        item,
        hlc,
        fields: vec![FieldSet {
            key: keys::PASSWORD.into(),
            value: FieldValue::Text(value.into()),
            base: None,
        }],
    }
}

#[test]
#[ignore = "B-1: pending versioned Lamport ordering in phase 9.2; legacy timestamps remain migration provenance"]
fn an_edit_made_later_with_a_correct_clock_becomes_the_current_value() {
    let now_ms = 1_800_000_000_000u64;
    let item = ItemId(uuid::Uuid::from_u128(1));
    let (wrong_clock_device, good_device) = (DeviceId([1; 32]), DeviceId([2; 32]));
    let mut state = VaultState::default();

    // A device whose clock is one year ahead saves a password.
    let ahead = Hlc {
        wall_ms: now_ms + 365 * 24 * 60 * 60 * 1000,
        counter: 0,
    };
    state.apply(wrong_clock_device, 1, &set(item, ahead, "old"));

    // Another device sees that op (the clock ignores the far-future time, by design) and the
    // user then changes the password there.
    let mut clock = HlcClock::default();
    clock.observe(ahead, now_ms);
    let later = clock.tick(now_ms + 60_000);
    state.apply(good_device, 1, &set(item, later, "new"));

    assert_eq!(
        state.items[&item].text(keys::PASSWORD),
        Some("new"),
        "the user's later change was saved but the old value still wins"
    );
}

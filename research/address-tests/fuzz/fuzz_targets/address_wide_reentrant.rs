//! libFuzzer target: 64 addresses with spawn-on-drop endpoints —
//! maximizing simultaneously live registrations under reentrant spawns.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_wide_reentrant --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_wide_reentrant(data);
});

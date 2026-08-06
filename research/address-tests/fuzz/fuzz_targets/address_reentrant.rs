//! libFuzzer target: endpoints whose `Drop` re-enters the space and
//! claims a NEW registration (parked in a bag, so the spawned
//! registration persists), with random claim/release/resolve histories.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_reentrant --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_reentrant(data);
});

//! libFuzzer target: nested lease chains (each endpoint owns the next
//! lease down its chain) with random builds — including builds that
//! collide with a live chain and abort via a partial drop cascade —
//! whole-chain cascades, resolves, and len checks.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_cascade --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_cascade(data);
});

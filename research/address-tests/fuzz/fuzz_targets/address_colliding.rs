//! libFuzzer target: model-checked histories over hash-colliding addresses
//! (every key lands in one bucket).
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_colliding --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_colliding(data);
});

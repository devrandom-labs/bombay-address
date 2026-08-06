//! libFuzzer target: 1-byte-op histories over 64 addresses (maximizes
//! simultaneously live registrations).
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_wide --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_wide(data);
});

//! libFuzzer target: cascade chains driven through short STRING keys on
//! a tiny alphabet with zero-padding variants — exercising the custom
//! chunked hasher's `write` path under nested-lease chain operations.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_string_cascade --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_string_cascade(data);
});

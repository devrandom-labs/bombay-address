//! libFuzzer target: two independent address spaces, each hosting nested
//! lease chains, with operations routed by a bit — no cross-space bleed
//! under chain mechanics.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_isolation_cascade --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_isolation_cascade(data);
});

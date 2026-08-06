//! libFuzzer target: operations routed between two independent spaces;
//! each must behave as if the other did not exist.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_isolation --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_isolation(data);
});

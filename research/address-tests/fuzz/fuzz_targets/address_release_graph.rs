//! libFuzzer target: release graphs — each endpoint may hold the lease of
//! ANOTHER live address, so releasing a top-level lease cascades through
//! a tree of endpoint drops at arbitrary addresses; failed builds release
//! their held lease through the rejected endpoint's drop.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run address_release_graph --fuzz-dir \
//!   research/address-tests/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    address_tests::fuzz_entry_release_graph(data);
});

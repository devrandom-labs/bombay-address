//! libFuzzer target: cascade chains driven through constant-hash keys —
//! every address collides in one hash bucket, combining the chain
//! mechanics (build, mid-build abort cascade, whole-chain cascade) with
//! collision contention.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run addresspass_colliding_cascade --fuzz-dir \
//!   research/addresspass-autoresearch/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    addresspass_autoresearch::fuzz_entry_colliding_cascade(data);
});

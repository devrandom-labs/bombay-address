//! libFuzzer target: reentrant spawn-on-drop endpoints driven through
//! constant-hash keys — every spawn claim, drop-time claim, and release
//! runs through ONE hash bucket.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run addresspass_reentrant_colliding --fuzz-dir \
//!   research/addresspass-autoresearch/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    addresspass_autoresearch::fuzz_entry_reentrant_colliding(data);
});

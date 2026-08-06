//! libFuzzer target: cascade chains driven through composite `(u64, u64)`
//! keys — exercising the hasher's two-half chunked `write` path under
//! nested-lease chain operations.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run addresspass_wide_cascade --fuzz-dir \
//!   research/addresspass-autoresearch/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    addresspass_autoresearch::fuzz_entry_wide_cascade(data);
});

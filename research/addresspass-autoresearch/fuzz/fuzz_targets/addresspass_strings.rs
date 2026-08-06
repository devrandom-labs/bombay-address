//! libFuzzer target: model-checked histories over string addresses
//! (chunked-hasher `write` path, zero-padding collision variants).
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run addresspass_strings --fuzz-dir \
//!   research/addresspass-autoresearch/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    addresspass_autoresearch::fuzz_entry_strings(data);
});

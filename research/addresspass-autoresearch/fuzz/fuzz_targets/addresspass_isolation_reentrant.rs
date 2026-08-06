//! libFuzzer target: two independent address spaces, each hosting
//! spawn-on-drop endpoints, with operations routed by a bit — no
//! cross-space bleed under reentrant spawns.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run addresspass_isolation_reentrant --fuzz-dir \
//!   research/addresspass-autoresearch/fuzz`
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    addresspass_autoresearch::fuzz_entry_isolation_reentrant(data);
});

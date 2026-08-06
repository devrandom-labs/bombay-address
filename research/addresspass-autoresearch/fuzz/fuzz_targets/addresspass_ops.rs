//! libFuzzer target: model-checked operation histories.
//!
//! Run with cargo-fuzz where available:
//! `cargo fuzz run addresspass_ops --fuzz-dir \
//!   research/addresspass-autoresearch/fuzz`
//!
//! The campaign's stable-toolchain execution of this exact logic lives in
//! `tests/fuzz_replay.rs` (deterministic seeded mutation replay); corpus
//! and execution counts are reported in RESEARCH-REPORT.md.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    addresspass_autoresearch::fuzz_entry(data);
});

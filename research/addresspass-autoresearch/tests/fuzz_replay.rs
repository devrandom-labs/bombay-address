//! Deterministic fuzz campaign on the stable toolchain.
//!
//! cargo-fuzz/libFuzzer requires a nightly sanitizer build that this
//! repository's pinned stable toolchain does not provide, so coverage-
//! guided fuzzing is approximated honestly: a fixed-seed mutational fuzzer
//! (xorshift64*) drives the exact entry points the libFuzzer targets call,
//! over a hand-written seed corpus plus PRNG-generated seeds. Every run is
//! bit-for-bit reproducible; seeds, mutation operators, and execution
//! counts are recorded in RESEARCH-REPORT.md.
//!
//! A crash (divergence between SUT and reference model) fails the test and
//! prints the offending input for minimization.
//!
//! Excluded from Miri (execution counts are tuned for native speed).
#![cfg(not(miri))]

use addresspass_autoresearch::{
    XorShift64Star, fuzz_entry, fuzz_entry_cascade, fuzz_entry_colliding,
    fuzz_entry_colliding_cascade, fuzz_entry_isolation, fuzz_entry_isolation_cascade,
    fuzz_entry_reentrant, fuzz_entry_reentrant_colliding, fuzz_entry_release_graph,
    fuzz_entry_string_cascade, fuzz_entry_strings, fuzz_entry_wide, fuzz_entry_wide_cascade,
};

/// Hand-written seeds: structured histories exercising claim/release/
/// resolve overlap, boundary addresses (0 and 15), and empty input.
const SEEDS: &[&[u8]] = &[
    &[],
    &[0, 1],
    &[0, 1, 2, 0, 1, 0],
    // claim addr 0, resolve addr 0, release addr 0, resolve addr 0
    &[0, 7, 2, 0, 1, 0, 2, 0],
    // double claim on addr 0, then release twice
    &[0, 9, 0, 10, 1, 0, 1, 0, 3, 0],
    // boundary address 15 (0b1111 << 2 | op)
    &[60, 5, 62, 0, 61, 0, 62, 0],
    // all-op walk across every address
    &[
        0, 1, 4, 2, 8, 3, 12, 4, 16, 5, 20, 6, 24, 7, 28, 8, 32, 9, 36, 10, 40, 11, 44, 12, 48, 13,
        52, 14, 56, 15, 60, 16,
    ],
];

/// One deterministic mutation step: flip a random byte to a random value,
/// truncate, or duplicate a slice — the classic mutation operators, driven
/// by the seeded PRNG so the whole campaign replays identically.
fn mutate(rng: &mut XorShift64Star, input: &mut Vec<u8>) {
    if input.is_empty() || rng.below(4) == 0 {
        let len = 1 + rng.below(64);
        input.clear();
        for _ in 0..len {
            input.push(rng.next_u64() as u8);
        }
        return;
    }
    match rng.below(3) {
        0 => {
            let index = rng.below(input.len() as u64) as usize;
            input[index] = rng.next_u64() as u8;
        }
        1 => {
            let keep = 1 + rng.below(input.len() as u64) as usize;
            input.truncate(keep);
        }
        _ => {
            let split = rng.below(input.len() as u64) as usize;
            let tail: Vec<u8> = input[split..].to_vec();
            input.extend_from_slice(&tail);
        }
    }
}

/// Default executions per campaign for the gate; a deep soak can raise
/// this via `FUZZ_EXECUTIONS=N` (seeds stay fixed — still deterministic
/// for a given N).
fn executions() -> u64 {
    std::env::var("FUZZ_EXECUTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000)
}

fn campaign(entry: fn(&[u8]), seed: u64, executions: u64) -> u64 {
    let mut rng = XorShift64Star::new(seed);
    let mut input: Vec<u8> = Vec::new();
    let mut runs = 0_u64;
    for seed_bytes in SEEDS {
        input.clear();
        input.extend_from_slice(seed_bytes);
        entry(&input);
        runs += 1;
        // Mutate each seed in a deterministic chain.
        for _ in 0..(executions / (SEEDS.len() as u64)) {
            mutate(&mut rng, &mut input);
            let snapshot = input.clone();
            entry(&snapshot);
            runs += 1;
        }
    }
    runs
}

#[test]
fn fuzz_replay_ops_histories_never_diverge() {
    // Seed fixed for reproducibility: 0xA55E_0001 (campaign 1).
    let runs = campaign(fuzz_entry, 0xA55E_0001, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_colliding_histories_never_diverge() {
    // Seed fixed for reproducibility: 0xC011_1D1E (campaign 2).
    let runs = campaign(fuzz_entry_colliding, 0xC011_1D1E, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_string_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x5E1E_0003 (campaign 3).
    let runs = campaign(fuzz_entry_strings, 0x5E1E_0003, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_wide_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x41DE_0004 (campaign 4).
    let runs = campaign(fuzz_entry_wide, 0x41DE_0004, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_isolation_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x1501_A7E5 (campaign 5).
    let runs = campaign(fuzz_entry_isolation, 0x1501_A7E5, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_cascade_histories_never_diverge() {
    // Seed fixed for reproducibility: 0xC45C_ADE0 (campaign 6).
    let runs = campaign(fuzz_entry_cascade, 0xC45C_ADE0, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_reentrant_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x52E3_37A4 (campaign 7).
    let runs = campaign(fuzz_entry_reentrant, 0x52E3_37A4, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_release_graph_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x6A4A_1EED (campaign 8).
    let runs = campaign(fuzz_entry_release_graph, 0x6A4A_1EED, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_colliding_cascade_histories_never_diverge() {
    // Seed fixed for reproducibility: 0xC011_CA5C (campaign 9).
    let runs = campaign(fuzz_entry_colliding_cascade, 0xC011_CA5C, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_string_cascade_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x57A1_CA5C (campaign 10).
    let runs = campaign(fuzz_entry_string_cascade, 0x57A1_CA5C, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_isolation_cascade_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x1501_CA5C (campaign 11).
    let runs = campaign(fuzz_entry_isolation_cascade, 0x1501_CA5C, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_wide_cascade_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x41DE_CA5C (campaign 12).
    let runs = campaign(fuzz_entry_wide_cascade, 0x41DE_CA5C, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

#[test]
fn fuzz_replay_reentrant_colliding_histories_never_diverge() {
    // Seed fixed for reproducibility: 0x52E3_C011 (campaign 13).
    let runs = campaign(fuzz_entry_reentrant_colliding, 0x52E3_C011, executions());
    assert!(runs >= executions(), "only {runs} executions");
}

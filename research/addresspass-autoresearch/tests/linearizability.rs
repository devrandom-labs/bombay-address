//! Linearizability checking of real concurrent histories.
//!
//! Threads execute scripted, barrier-synchronized workloads while
//! recording each operation's kind, value, result, and start/finish
//! indices from a global sequence counter. A backtracking checker then
//! decides whether some sequential ordering of the recorded operations
//! (respecting real-time order) produces exactly the recorded results
//! under the documented semantics.
//!
//! Multi-address histories are checked PER ADDRESS: linearizability is a
//! local property (Herlihy & Wing, *Linearizability: A Correctness
//! Condition for Concurrent Objects*, TOPLAS 1990 — a history is
//! linearizable iff every object's subhistory is), and each address pass
//! registration is an independent object.
//!
//! The checker is validated against fabricated illegal histories so the
//! suite can actually fail (a checker that accepts everything is
//! worthless).
#![cfg(not(miri))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use addresspass::AddressSpace;
use parking_lot::Mutex;
use proptest::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpKind {
    ClaimOk(u64),
    ClaimRejected,
    Release(u64),
    ResolveHit(u64),
    ResolveMiss,
}

#[derive(Debug, Clone, Copy)]
struct Timed {
    kind: OpKind,
    start: u64,
    finish: u64,
}

/// Sequential single-address state for the checker: the live
/// registration's endpoint value, if any.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct State {
    owner: Option<u64>,
}

impl State {
    /// Apply an op; returns the state after the op, or None if the op's
    /// recorded result is impossible in `self`.
    fn apply(self, op: OpKind) -> Option<State> {
        match (op, self.owner) {
            (OpKind::ClaimOk(v), None) => Some(State { owner: Some(v) }),
            (OpKind::ClaimOk(_), Some(_)) => None,
            (OpKind::ClaimRejected, Some(_)) => Some(self),
            (OpKind::ClaimRejected, None) => None,
            (OpKind::Release(v), Some(o)) if o == v => Some(State { owner: None }),
            (OpKind::Release(_), _) => None,
            (OpKind::ResolveHit(v), Some(o)) if o == v => Some(self),
            (OpKind::ResolveHit(_), _) => None,
            (OpKind::ResolveMiss, None) => Some(self),
            (OpKind::ResolveMiss, Some(_)) => None,
        }
    }
}

/// Backtracking linearizability check over `ops` (any order), respecting
/// real-time precedence, memoized on (completed-op bitmask, state).
fn is_linearizable(ops: &[Timed]) -> bool {
    let n = ops.len();
    assert!(n < 64, "bitmask checker caps at 63 ops");
    use std::collections::HashMap;
    let mut memo: HashMap<(u64, Option<u64>), bool> = HashMap::new();

    fn search(
        ops: &[Timed],
        done: u64,
        state: State,
        memo: &mut std::collections::HashMap<(u64, Option<u64>), bool>,
    ) -> bool {
        if done == (1 << ops.len()) - 1 {
            return true;
        }
        if let Some(&cached) = memo.get(&(done, state.owner)) {
            return cached;
        }
        let result = (0..ops.len()).any(|i| {
            if done & (1 << i) != 0 {
                return false;
            }
            // Op i may be linearized next iff every op that FINISHED
            // before i STARTED (strict real-time precedence) is done.
            let precedence_ok = (0..ops.len()).all(|j| {
                j == i || done & (1 << j) != 0 || ops[j].finish > ops[i].start
            });
            precedence_ok
                && state
                    .apply(ops[i].kind)
                    .is_some_and(|next| search(ops, done | (1 << i), next, memo))
        });
        memo.insert((done, state.owner), result);
        result
    }

    search(ops, 0, State { owner: None }, &mut memo)
}

/// One scripted operation for the concurrent workloads. Scripts fix each
/// thread's op sequence; the interleaving is left to the scheduler.
#[derive(Debug, Clone, Copy)]
enum ScriptOp {
    Claim(u8),
    Resolve(u8),
    /// Release the oldest lease this thread still holds, if any.
    ReleaseOwned,
}

/// Run one thread per script against a shared space, all released onto
/// the workload by one barrier. Returns per-address timed logs.
fn run_scripted(scripts: &[Vec<ScriptOp>]) -> Vec<Vec<Timed>> {
    const MAX_ADDRESSES: usize = 8;
    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let clock = Arc::new(AtomicU64::new(1));
    let logs: Arc<Vec<Mutex<Vec<Timed>>>> =
        Arc::new((0..MAX_ADDRESSES).map(|_| Mutex::new(Vec::new())).collect());
    let barrier = Arc::new(Barrier::new(scripts.len()));

    let handles: Vec<_> = scripts
        .iter()
        .enumerate()
        .map(|(thread, script)| {
            let space = Arc::clone(&space);
            let clock = Arc::clone(&clock);
            let logs = Arc::clone(&logs);
            let barrier = Arc::clone(&barrier);
            let script = script.clone();
            std::thread::spawn(move || {
                let mut owned: Vec<(u64, addresspass::Lease<u64, u64>)> = Vec::new();
                barrier.wait();
                for (round, op) in script.iter().enumerate() {
                    match *op {
                        ScriptOp::Claim(address) => {
                            let address = u64::from(address) % MAX_ADDRESSES as u64;
                            // Unique endpoint per (thread, round).
                            let endpoint = (thread * 1_000 + round + 1) as u64;
                            let start = clock.fetch_add(1, Ordering::SeqCst);
                            let outcome = space.claim(address, endpoint);
                            let finish = clock.fetch_add(1, Ordering::SeqCst);
                            match outcome {
                                Ok(lease) => {
                                    logs[address as usize].lock().push(Timed {
                                        kind: OpKind::ClaimOk(endpoint),
                                        start,
                                        finish,
                                    });
                                    owned.push((endpoint, lease));
                                }
                                Err(_) => logs[address as usize].lock().push(Timed {
                                    kind: OpKind::ClaimRejected,
                                    start,
                                    finish,
                                }),
                            }
                        }
                        ScriptOp::Resolve(address) => {
                            let address = u64::from(address) % MAX_ADDRESSES as u64;
                            let start = clock.fetch_add(1, Ordering::SeqCst);
                            let resolved = space.resolve(&address);
                            let finish = clock.fetch_add(1, Ordering::SeqCst);
                            let kind = match resolved {
                                Some(v) => OpKind::ResolveHit(v),
                                None => OpKind::ResolveMiss,
                            };
                            logs[address as usize].lock().push(Timed { kind, start, finish });
                        }
                        ScriptOp::ReleaseOwned => {
                            if !owned.is_empty() {
                                let (endpoint, lease) = owned.remove(0);
                                let address = *lease.address();
                                let start = clock.fetch_add(1, Ordering::SeqCst);
                                lease.release();
                                let finish = clock.fetch_add(1, Ordering::SeqCst);
                                logs[address as usize].lock().push(Timed {
                                    kind: OpKind::Release(endpoint),
                                    start,
                                    finish,
                                });
                            }
                        }
                    }
                }
                // Drain this thread's leases inside the timed region.
                for (endpoint, lease) in owned {
                    let address = *lease.address();
                    let start = clock.fetch_add(1, Ordering::SeqCst);
                    lease.release();
                    let finish = clock.fetch_add(1, Ordering::SeqCst);
                    logs[address as usize].lock().push(Timed {
                        kind: OpKind::Release(endpoint),
                        start,
                        finish,
                    });
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert!(space.is_empty());
    let mut result = Vec::new();
    for log in logs.iter() {
        let mut log = log.lock().clone();
        log.sort_by_key(|op| (op.start, op.finish));
        result.push(log);
    }
    result
}

fn check_all_addresses(logs: &[Vec<Timed>]) {
    for (address, log) in logs.iter().enumerate() {
        if log.is_empty() {
            continue;
        }
        assert!(
            is_linearizable(log),
            "address {address}: recorded history has no legal sequential \
             ordering:\n{log:#?}"
        );
    }
}

#[test]
fn hot_address_history_is_linearizable() {
    // 4 threads × 5 rounds on ONE address (fixed scripts for
    // determinism of the workload; the interleaving is the scheduler's).
    let scripts: Vec<Vec<ScriptOp>> = (0..4)
        .map(|_| {
            (0..5)
                .flat_map(|_| [ScriptOp::Claim(0), ScriptOp::Resolve(0), ScriptOp::ReleaseOwned])
                .collect()
        })
        .collect();
    let logs = run_scripted(&scripts);
    assert!(logs[0].len() < 64, "{} ops recorded", logs[0].len());
    check_all_addresses(&logs);
}

#[test]
fn three_address_history_is_linearizable_per_address() {
    let scripts: Vec<Vec<ScriptOp>> = (0..6)
        .map(|thread| {
            (0..4)
                .flat_map(|round| {
                    let address = ((thread + round) % 3) as u8;
                    [
                        ScriptOp::Claim(address),
                        ScriptOp::Resolve((address + 1) % 3),
                        ScriptOp::ReleaseOwned,
                        ScriptOp::Resolve(address),
                    ]
                })
                .collect()
        })
        .collect();
    let logs = run_scripted(&scripts);
    check_all_addresses(&logs);
}

/// Replacement churn on ONE hot address: many threads claim, resolve,
/// release, and RE-CLAIM the same address across rounds — the ownership
/// handoff between threads exercises the release→reclaim race the
/// checker must order. Every recorded history must be linearizable.
#[test]
fn replacement_churn_on_one_address_is_linearizable() {
    // 6 threads × 3 rounds of claim/resolve/release on address 0: the
    // hot-address log stays under the checker's 63-op bitmask cap
    // (6 × 3 × 3 = 54 ops).
    let scripts: Vec<Vec<ScriptOp>> = (0..6)
        .map(|_| {
            (0..3)
                .flat_map(|_| [ScriptOp::Claim(0), ScriptOp::Resolve(0), ScriptOp::ReleaseOwned])
                .collect()
        })
        .collect();
    let logs = run_scripted(&scripts);
    assert!(logs[0].len() < 64, "{} ops recorded", logs[0].len());
    check_all_addresses(&logs);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    /// Randomized concurrent linearizability: proptest fixes the scripts,
    /// the scheduler picks the interleaving; every recorded per-address
    /// history must be linearizable.
    #[test]
    fn random_scripts_are_linearizable(
        scripts in prop::collection::vec(
            prop::collection::vec(
                prop_oneof![
                    3 => (0u8..3).prop_map(ScriptOp::Claim),
                    4 => (0u8..3).prop_map(ScriptOp::Resolve),
                    2 => Just(ScriptOp::ReleaseOwned),
                ],
                1..10
            ),
            2..5
        )
    ) {
        let logs = run_scripted(&scripts);
        check_all_addresses(&logs);
    }
}

/// The checker must REJECT illegal histories — otherwise the tests above
/// could never fail. Each fabricated history violates exactly one rule.
#[test]
fn checker_rejects_illegal_histories() {
    // Two overlapping successful claims with no release between them.
    let double_claim = [
        Timed { kind: OpKind::ClaimOk(1), start: 1, finish: 4 },
        Timed { kind: OpKind::ClaimOk(2), start: 2, finish: 3 },
    ];
    assert!(!is_linearizable(&double_claim));

    // A resolve observing a value never successfully claimed.
    let phantom_resolve = [
        Timed { kind: OpKind::ClaimOk(1), start: 1, finish: 2 },
        Timed { kind: OpKind::ResolveHit(9), start: 3, finish: 4 },
    ];
    assert!(!is_linearizable(&phantom_resolve));

    // A resolve missing while an un-released claim strictly precedes it
    // in real time.
    let stale_miss = [
        Timed { kind: OpKind::ClaimOk(1), start: 1, finish: 2 },
        Timed { kind: OpKind::ResolveMiss, start: 3, finish: 4 },
    ];
    assert!(!is_linearizable(&stale_miss));

    // A rejected claim on an address that was released in real time
    // before the claim started.
    let wrong_reject = [
        Timed { kind: OpKind::ClaimOk(1), start: 1, finish: 2 },
        Timed { kind: OpKind::Release(1), start: 3, finish: 4 },
        Timed { kind: OpKind::ClaimRejected, start: 5, finish: 6 },
    ];
    assert!(!is_linearizable(&wrong_reject));

    // And the legal versions of each shape ARE accepted (checker is not
    // just saying no to everything).
    let legal = [
        Timed { kind: OpKind::ClaimOk(1), start: 1, finish: 2 },
        Timed { kind: OpKind::ResolveHit(1), start: 3, finish: 4 },
        Timed { kind: OpKind::Release(1), start: 5, finish: 6 },
        Timed { kind: OpKind::ClaimOk(2), start: 7, finish: 8 },
        Timed { kind: OpKind::ResolveMiss, start: 1, finish: 2 },
    ];
    assert!(is_linearizable(&legal));
}

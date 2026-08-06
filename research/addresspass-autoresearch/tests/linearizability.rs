//! Linearizability checking of real concurrent histories.
//!
//! Threads execute a fixed, barrier-synchronized workload on ONE hot
//! address while recording each operation's kind, value, result, and
//! start/finish indices from a global sequence counter. A backtracking
//! checker then decides whether some sequential ordering of the recorded
//! operations (respecting real-time order: op A finishes before op B
//! starts) produces exactly the recorded results under the documented
//! semantics.
//!
//! The checker is validated against a fabricated illegal history so the
//! test suite can actually fail (a checker that accepts everything is
//! worthless).
#![cfg(not(miri))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use addresspass::AddressSpace;
use parking_lot::Mutex;

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

/// Sequential single-address state for the checker. `owner` is the live
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
            // Real-time order: op i cannot be linearized before any
            // already-completed op that it overlaps... the standard rule:
            // op i may go next iff every op that FINISHED before i STARTED
            // is already done.
            let precedence_ok = (0..ops.len()).all(|j| {
                j == i
                    || done & (1 << j) != 0
                    || ops[j].finish > ops[i].start
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

/// Execute the real concurrent workload: 4 threads × 5 rounds on one
/// address, recording timed operations.
fn record_history() -> Vec<Timed> {
    const THREADS: usize = 4;
    const ROUNDS: u64 = 5;

    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let clock = Arc::new(AtomicU64::new(1));
    let log = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS as u64)
        .map(|thread| {
            let space = Arc::clone(&space);
            let clock = Arc::clone(&clock);
            let log = Arc::clone(&log);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for round in 0..ROUNDS {
                    let endpoint = thread * 100 + round + 1;
                    let start = clock.fetch_add(1, Ordering::SeqCst);
                    let outcome = space.claim(0, endpoint);
                    match outcome {
                        Ok(lease) => {
                            let finish = clock.fetch_add(1, Ordering::SeqCst);
                            log.lock().push(Timed {
                                kind: OpKind::ClaimOk(endpoint),
                                start,
                                finish,
                            });
                            // Resolve while we own the address.
                            let start = clock.fetch_add(1, Ordering::SeqCst);
                            let resolved = space.resolve(&0);
                            let finish = clock.fetch_add(1, Ordering::SeqCst);
                            let kind = match resolved {
                                Some(v) => OpKind::ResolveHit(v),
                                None => OpKind::ResolveMiss,
                            };
                            log.lock().push(Timed { kind, start, finish });
                            let start = clock.fetch_add(1, Ordering::SeqCst);
                            lease.release();
                            let finish = clock.fetch_add(1, Ordering::SeqCst);
                            log.lock().push(Timed {
                                kind: OpKind::Release(endpoint),
                                start,
                                finish,
                            });
                        }
                        Err(_) => {
                            let finish = clock.fetch_add(1, Ordering::SeqCst);
                            log.lock().push(Timed {
                                kind: OpKind::ClaimRejected,
                                start,
                                finish,
                            });
                        }
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert!(space.is_empty());
    log.lock().clone()
}

#[test]
fn recorded_concurrent_history_is_linearizable() {
    let mut history = record_history();
    assert!(!history.is_empty());
    assert!(history.len() < 64, "{} ops recorded", history.len());
    // Deterministic order for the checker (recording order is irrelevant
    // to the verdict, but fix it for reproducibility).
    history.sort_by_key(|op| (op.start, op.finish));
    assert!(
        is_linearizable(&history),
        "recorded history has no legal sequential ordering:\n{history:#?}"
    );
}

/// The checker must REJECT illegal histories — otherwise the test above
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

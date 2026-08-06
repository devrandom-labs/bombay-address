//! FINDING-001 probe: caller `Hash`/`Eq` code runs under the table lock.
//!
//! The crate documents that endpoint `Clone`/`Drop` never run under the
//! lock (re-entrancy safe). The same guarantee does NOT extend to the
//! address's `Hash`/`Eq`: `claim` hashes and compares the address while
//! holding the write guard, so an address whose `Hash` re-enters the same
//! address space self-deadlocks (parking_lot locks are not re-entrant).
//!
//! The active assertion expresses the correct expected behavior: a claim
//! with a re-entrant `Hash` completes. It currently fails (the claim never
//! returns), so the test is ignored pending a production decision.
#![cfg(not(miri))]

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use addresspass::AddressSpace;

/// An address whose `Hash` and `Eq` re-enter the address space they are
/// registered in — the same re-entrancy pattern the crate explicitly
/// supports for endpoint `Clone`/`Drop`.
struct ReentrantKey {
    id: u64,
    space: Arc<AddressSpace<ReentrantKey, u64>>,
}

impl PartialEq for ReentrantKey {
    fn eq(&self, other: &Self) -> bool {
        // Re-entrant read: takes the table's read guard.
        let _ = self.space.len();
        self.id == other.id
    }
}
impl Eq for ReentrantKey {}

impl Hash for ReentrantKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Re-entrant read: takes the table's read guard.
        let _ = self.space.len();
        state.write_u64(self.id);
    }
}

impl Clone for ReentrantKey {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            space: Arc::clone(&self.space),
        }
    }
}

#[test]
#[ignore = "FINDING-001: claim hashes/compares the address under the write guard; a re-entrant Hash self-deadlocks"]
fn claim_with_reentrant_hash_completes() {
    let space = Arc::new(AddressSpace::<ReentrantKey, u64>::new());
    let key = ReentrantKey {
        id: 1,
        space: Arc::clone(&space),
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = space.claim(key, 7);
        let _ = tx.send(result.is_ok());
    });
    let completed = rx.recv_timeout(Duration::from_secs(5));
    assert!(
        matches!(completed, Ok(true)),
        "claim with a re-entrant Hash did not complete within 5s: \
         the address is hashed under the write guard and the re-entrant \
         read deadlocks (FINDING-001)"
    );
}

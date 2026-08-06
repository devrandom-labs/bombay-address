//! A typed concurrent address space with generation-safe ownership.
//!
//! # Address key contracts
//!
//! Address keys (`A: Eq + Hash + Clone`) must satisfy these invariants on
//! every call that passes through the space:
//!
//! - **Deterministic.** `Hash` and `Eq` must produce the same output for
//!   the same input every time they are called.
//! - **Consistent.** `Hash` and `Eq` must agree: if `a == b`, then
//!   `a.hash(…)` and `b.hash(…)` must write the same bytes to the
//!   hasher (the `Hash` trait requirement).
//! - **No panic.** `Hash` and `Eq` must never panic. A panicking `Hash`
//!   or `Eq` during `claim`, `resolve`, or `release` may leave the
//!   address space in an inconsistent state.
//! - **No re-entrancy.** `Hash` and `Eq` must not call any method of the
//!   *same* `AddressSpace` from which they were invoked. The hash-table
//!   probe runs under the table's write or read guard, and
//!   `parking_lot` locks are not re-entrant: a re-entrant `Hash` or
//!   `Eq` will deadlock. (Endpoint `Clone` and `Drop` are intentionally
//!   run outside the lock and can safely re-enter the space.)
//! - **Identity-preserving `Clone`.** `A: Clone` must preserve `Eq` and
//!   `Hash` identity: for every address value `a`,
//!   `a.clone() == a` and `a.clone()` hashes to the same value as `a`.
//!   The reference documentation calls this the *logical Clone contract*
//!   for key types.

mod table;

use core::hash::Hash;
#[cfg(loom)]
use loom::sync::atomic::{AtomicU64, Ordering};
#[cfg(loom)]
use loom::sync::{Arc, RwLock};
#[cfg(not(loom))]
use parking_lot::RwLock;
#[cfg(not(loom))]
use std::sync::Arc;
#[cfg(not(loom))]
use std::sync::atomic::{AtomicU64, Ordering};

use table::OpenTable;

/// A failed attempt to claim an address that already has a live owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInUse<A>(pub A);

struct Inner<A, E> {
    next_generation: AtomicU64,
    entries: RwLock<OpenTable<A, E>>,
}

impl<A, E> Inner<A, E> {
    #[cfg(loom)]
    fn read_entries(&self) -> loom::sync::RwLockReadGuard<'_, OpenTable<A, E>> {
        // loom's RwLock::read() always returns Ok — the lock-acquisition
        // loop never fails. Unwrap is honest: loom never poisons.
        self.entries.read().unwrap()
    }

    #[cfg(loom)]
    fn write_entries(&self) -> loom::sync::RwLockWriteGuard<'_, OpenTable<A, E>> {
        self.entries.write().unwrap()
    }

    #[cfg(not(loom))]
    fn read_entries(&self) -> parking_lot::RwLockReadGuard<'_, OpenTable<A, E>> {
        self.entries.read()
    }

    #[cfg(not(loom))]
    fn write_entries(&self) -> parking_lot::RwLockWriteGuard<'_, OpenTable<A, E>> {
        self.entries.write()
    }
}

/// A shared mapping from addresses to typed endpoints.
pub struct AddressSpace<A, E> {
    inner: Arc<Inner<A, E>>,
}

impl<A, E> Clone for AddressSpace<A, E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<A, E> Default for AddressSpace<A, E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A, E> AddressSpace<A, E> {
    /// Construct an empty address space.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                next_generation: AtomicU64::new(1),
                entries: RwLock::new(OpenTable::new()),
            }),
        }
    }

    /// Return the number of live registrations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.read_entries().len()
    }

    /// Return whether the address space is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<A: Eq + Hash, E> AddressSpace<A, E> {
    /// Reclaim excess backing-table capacity (rehash the map into a
    /// smaller allocation where the implementation can do so). This may
    /// block concurrent address-space operations while it runs.
    ///
    /// Preserves every live registration, endpoint, and generation. Safe
    /// on a new or already-empty space; idempotent; may be called while
    /// registrations are live. Ordinary `Lease::release` never shrinks
    /// automatically.
    pub fn shrink_to_fit(&self) {
        self.inner.write_entries().shrink_to_fit();
    }
}

impl<A, E> AddressSpace<A, E>
where
    A: Eq + Hash + Clone,
{
    /// Exclusively claim `address` for `endpoint`.
    ///
    /// # Errors
    /// Returns [`AddressInUse`] when an owner is already registered.
    ///
    /// # Key contracts
    ///
    /// The address key is `Clone`d before the write guard is taken, so
    /// caller `Clone` code never runs under the lock. The clone must
    /// preserve `Eq`/`Hash` identity (see [module-level
    /// docs](index.html#address-key-contracts)). The original key is
    /// given to the returned [`Lease`]; the clone is stored in the table.
    pub fn claim(&self, address: A, endpoint: E) -> Result<Lease<A, E>, AddressInUse<A>> {
        // The key copy for the table happens before the write guard is
        // taken: caller `A: Clone` code must not run under the lock.
        let key = address.clone();
        let mut entries = self.inner.write_entries();
        if entries.get(&address).is_some() {
            return Err(AddressInUse(address));
        }
        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        entries.insert(key, generation, endpoint);
        Ok(Lease {
            inner: self.inner.clone(),
            address,
            generation,
            released: false,
        })
    }
}

impl<A, E> AddressSpace<A, E>
where
    A: Eq + Hash,
    E: Clone,
{
    /// Resolve a snapshot of the endpoint currently registered at `address`.
    ///
    /// The endpoint's `Clone` runs after the read guard is dropped: taking
    /// the shared handle under the guard is refcount arithmetic only, so a
    /// re-entrant `Clone` that claims or releases in the address space
    /// cannot deadlock against the lock.
    #[must_use]
    pub fn resolve(&self, address: &A) -> Option<E> {
        let endpoint = {
            let guard = self.inner.read_entries();
            guard.get(address).map(|entry| Arc::clone(&entry.endpoint))
        };
        endpoint.as_deref().cloned()
    }
}

/// Exclusive ownership of one exact address registration generation.
pub struct Lease<A, E>
where
    A: Eq + Hash,
{
    inner: Arc<Inner<A, E>>,
    address: A,
    generation: u64,
    released: bool,
}

impl<A, E> Lease<A, E>
where
    A: Eq + Hash,
{
    /// Inspect the owned address.
    #[must_use]
    pub fn address(&self) -> &A {
        &self.address
    }

    /// Release this registration immediately.
    ///
    /// # Key contracts
    ///
    /// The address key is re-hashed during the table removal. The key's
    /// `Hash` and `Eq` must not panic; a panicking hash or equality
    /// comparison during release may permanently leave the registration
    /// in place (the lease is consumed, but the table entry is never
    /// removed).
    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        // The removed address and endpoint handle are dropped after the
        // write guard is released, so re-entrant `Drop` implementations
        // cannot deadlock against the lock.
        let removed = {
            let mut entries = self.inner.write_entries();
            entries.remove_if(&self.address, self.generation)
        };
        drop(removed);
    }
}

impl<A, E> Drop for Lease<A, E>
where
    A: Eq + Hash,
{
    fn drop(&mut self) {
        self.release_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::{AddressInUse, AddressSpace};

    #[test]
    fn lease_owns_and_releases_one_registration() {
        let space = AddressSpace::new();
        let lease = space.claim(7, "first").unwrap();
        assert_eq!(space.resolve(&7), Some("first"));
        assert!(matches!(space.claim(7, "second"), Err(AddressInUse(7))));
        drop(lease);
        assert_eq!(space.resolve(&7), None);
        let replacement = space.claim(7, "second").unwrap();
        assert_eq!(space.resolve(&7), Some("second"));
        drop(replacement);
    }
}

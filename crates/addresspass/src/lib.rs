//! A typed concurrent address space with generation-safe ownership.

mod table;

use core::hash::Hash;
#[cfg(loom)]
use loom::sync::atomic::{AtomicU64, Ordering};
#[cfg(loom)]
use loom::sync::{Arc, RwLock};
use std::sync::PoisonError;
#[cfg(not(loom))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(loom))]
use std::sync::{Arc, RwLock};

use table::OpenTable;

fn recover<T>(poisoned: PoisonError<T>) -> T {
    poisoned.into_inner()
}

/// A failed attempt to claim an address that already has a live owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInUse<A>(pub A);

struct Inner<A, E> {
    next_generation: AtomicU64,
    entries: RwLock<OpenTable<A, E>>,
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
        self.inner.entries.read().unwrap_or_else(recover).len()
    }

    /// Return whether the address space is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
    pub fn claim(&self, address: A, endpoint: E) -> Result<Lease<A, E>, AddressInUse<A>> {
        let mut entries = self.inner.entries.write().unwrap_or_else(recover);
        if entries.get(&address).is_some() {
            return Err(AddressInUse(address));
        }
        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        entries.insert(address.clone(), generation, endpoint);
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
    #[must_use]
    pub fn resolve(&self, address: &A) -> Option<E> {
        self.inner
            .entries
            .read()
            .unwrap_or_else(recover)
            .get(address)
            .map(|entry| entry.endpoint.clone())
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
    pub fn release(mut self) {
        self.release_inner();
    }

    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut entries = self.inner.entries.write().unwrap_or_else(recover);
        entries.remove_if(&self.address, self.generation);
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

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
use loom::sync::{Arc, RwLock};
#[cfg(not(loom))]
use parking_lot::RwLock;
#[cfg(not(loom))]
use std::sync::Arc;
use std::{fmt, hash::Hasher, num::NonZeroU64};

use table::OpenTable;

/// A failed attempt to claim an address that already has a live owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInUse<A>(pub A);

/// The reason an address claim failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError<A> {
    /// An owner is already registered at the returned address.
    AddressInUse(A),
    /// This address space has issued every available registration identity.
    ///
    /// Exhaustion is permanent: identities are never wrapped or reused.
    RegistrationIdsExhausted(A),
}

struct RegistrationScopeMarker {
    _private: u8,
}

/// Opaque identity of one address-space scope within the current process.
///
/// Clones of an [`AddressSpace`] have the same scope. Independently created
/// spaces have different scopes. This value is process-local: it is not a
/// durable identifier, an authentication credential, or registration
/// authority.
#[derive(Clone)]
pub struct RegistrationScopeId(Arc<RegistrationScopeMarker>);

impl PartialEq for RegistrationScopeId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RegistrationScopeId {}

impl core::hash::Hash for RegistrationScopeId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.0).hash(state);
    }
}

impl fmt::Debug for RegistrationScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RegistrationScopeId")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

/// Opaque identity of one exact registration within the current process.
///
/// The identity is independent of the address and endpoint types. It grants no
/// ownership, resolution, or release authority and is meaningful only for
/// process-local equality and correlation.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RegistrationId {
    scope: RegistrationScopeId,
    generation: NonZeroU64,
}

impl RegistrationId {
    /// Return the address-space scope in which this registration was created.
    #[must_use]
    pub fn scope_id(&self) -> &RegistrationScopeId {
        &self.scope
    }
}

impl fmt::Debug for RegistrationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistrationId")
            .field("scope", &self.scope)
            .field("generation", &self.generation)
            .finish()
    }
}

struct Inner<A, E> {
    scope: RegistrationScopeId,
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
                scope: RegistrationScopeId(Arc::new(RegistrationScopeMarker { _private: 0 })),
                entries: RwLock::new(OpenTable::new()),
            }),
        }
    }

    /// Return this address space's opaque, process-local registration scope.
    #[must_use]
    pub fn registration_scope_id(&self) -> RegistrationScopeId {
        self.inner.scope.clone()
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
    /// # Panics
    /// Panics after this address space has issued all `u64::MAX` registration
    /// identities. Use [`AddressSpace::try_claim`] to handle this physically
    /// unreachable boundary without panicking. Identities never wrap or reuse.
    ///
    /// # Key contracts
    ///
    /// The address key is `Clone`d before the write guard is taken, so
    /// caller `Clone` code never runs under the lock. The clone must
    /// preserve `Eq`/`Hash` identity (see [module-level
    /// docs](index.html#address-key-contracts)). The original key is
    /// given to the returned [`Lease`]; the clone is stored in the table.
    pub fn claim(&self, address: A, endpoint: E) -> Result<Lease<A, E>, AddressInUse<A>> {
        match self.try_claim(address, endpoint) {
            Ok(lease) => Ok(lease),
            Err(ClaimError::AddressInUse(address)) => Err(AddressInUse(address)),
            Err(ClaimError::RegistrationIdsExhausted(_)) => {
                panic!("registration identities exhausted")
            }
        }
    }

    /// Exclusively claim `address`, including explicit identity-exhaustion
    /// handling.
    ///
    /// This is equivalent to [`AddressSpace::claim`] during ordinary
    /// operation. Unlike `claim`, it returns
    /// [`ClaimError::RegistrationIdsExhausted`] after the final identity has
    /// been issued. Exhaustion is permanent.
    ///
    /// # Errors
    /// Returns [`ClaimError::AddressInUse`] when an owner is already
    /// registered. Returns [`ClaimError::RegistrationIdsExhausted`] after the
    /// address space has issued all `u64::MAX` registration identities.
    pub fn try_claim(&self, address: A, endpoint: E) -> Result<Lease<A, E>, ClaimError<A>> {
        // The key copy for the table happens before the write guard is
        // taken: caller `A: Clone` code must not run under the lock.
        let key = address.clone();
        let mut entries = self.inner.write_entries();
        if entries.get(&address).is_some() {
            return Err(ClaimError::AddressInUse(address));
        }
        let Some(generation) = entries.take_generation() else {
            return Err(ClaimError::RegistrationIdsExhausted(address));
        };
        entries.insert(key, generation.get(), endpoint);
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
    generation: NonZeroU64,
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

    /// Return the opaque, process-local identity of this exact registration.
    ///
    /// The returned value grants no ownership or release authority and does
    /// not keep the registration or endpoint alive.
    #[must_use]
    pub fn registration_id(&self) -> RegistrationId {
        RegistrationId {
            scope: self.inner.scope.clone(),
            generation: self.generation,
        }
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
            entries.remove_if(&self.address, self.generation.get())
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
    use super::{AddressSpace, ClaimError};
    use std::collections::HashSet;

    #[test]
    fn lease_owns_and_releases_one_registration() {
        let space = AddressSpace::new();
        let lease = space.claim(7, "first").unwrap();
        assert_eq!(space.resolve(&7), Some("first"));
        assert!(matches!(
            space.try_claim(7, "second"),
            Err(ClaimError::AddressInUse(7))
        ));
        drop(lease);
        assert_eq!(space.resolve(&7), None);
        let replacement = space.claim(7, "second").unwrap();
        assert_eq!(space.resolve(&7), Some("second"));
        drop(replacement);
    }

    #[test]
    fn registration_identity_distinguishes_replacements() {
        let space = AddressSpace::new();
        let first = space.claim("worker", 1).unwrap();
        let first_id = first.registration_id();
        first.release();

        let second = space.claim("worker", 2).unwrap();
        let second_id = second.registration_id();
        assert_ne!(first_id, second_id);
        assert_eq!(first_id.scope_id(), second_id.scope_id());

        let ids = HashSet::from([first_id, second_id]);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn registration_identity_is_scoped_to_the_address_space() {
        let first = AddressSpace::new();
        let first_peer = first.clone();
        let second = AddressSpace::new();

        assert_eq!(
            first.registration_scope_id(),
            first_peer.registration_scope_id()
        );
        assert_ne!(
            first.registration_scope_id(),
            second.registration_scope_id()
        );

        let first_id = first.claim((1_u8, 2_u8), ()).unwrap().registration_id();
        let second_id = second.claim((1_u8, 2_u8), ()).unwrap().registration_id();
        assert_ne!(first_id, second_id);
    }

    #[test]
    fn retaining_an_identity_grants_no_registration_lifetime() {
        let space = AddressSpace::new();
        let lease = space
            .claim(String::from("worker"), String::from("mailbox"))
            .unwrap();
        let identity = lease.registration_id();
        drop(lease);

        assert_eq!(space.resolve(&String::from("worker")), None);
        let replacement = space
            .claim(String::from("worker"), String::from("replacement"))
            .unwrap();
        assert_ne!(identity, replacement.registration_id());
    }

    #[test]
    fn exhausted_space_rejects_claims_without_reusing_an_identity() {
        let space = AddressSpace::new();
        space.inner.write_entries().set_next_generation(u64::MAX);

        let last = space.claim(1_u64, "last").unwrap();
        let last_id = last.registration_id();
        last.release();

        assert!(matches!(
            space.try_claim(2, "never inserted"),
            Err(ClaimError::RegistrationIdsExhausted(2))
        ));
        assert!(space.is_empty());
        assert_eq!(last_id.scope_id(), &space.registration_scope_id());
    }
}

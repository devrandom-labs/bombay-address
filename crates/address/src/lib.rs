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
//!   or `Eq` during acquisition, publication, resolution, or release may
//!   leave the address space in an inconsistent state.
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

/// A read-only capability for one resolved endpoint snapshot.
///
/// The snapshot remains valid after its registration is released or its
/// address is reused. Its storage and reclamation mechanism are intentionally
/// opaque; consumers can access the endpoint through [`core::ops::Deref`] or
/// [`AsRef`] but cannot reconstruct registration authority from it.
///
/// Resolved endpoints are read-only capabilities. The wrapper does not grant
/// mutable access, construction, destructuring, or registration authority.
/// As with any shared Rust reference, an endpoint may still expose deliberate
/// interior-mutability operations through `&self`.
///
/// ```compile_fail
/// use bombay_address::AddressSpace;
///
/// let space = AddressSpace::new();
/// let _lease = space.claim("worker", String::from("endpoint")).unwrap();
/// let mut endpoint = space.resolve(&"worker").unwrap();
/// endpoint.push_str("-mutated");
/// ```
///
/// Its private field prevents consumers from constructing or destructuring it:
///
/// ```compile_fail
/// use bombay_address::Resolved;
///
/// let endpoint = Resolved(String::from("forged"));
/// let Resolved(inner) = endpoint;
/// ```
///
/// The endpoint cannot be moved out through the shared dereference:
///
/// ```compile_fail
/// use bombay_address::AddressSpace;
///
/// let space = AddressSpace::new();
/// let _lease = space.claim("worker", String::from("endpoint")).unwrap();
/// let endpoint = space.resolve(&"worker").unwrap();
/// let inner: String = *endpoint;
/// ```
///
/// Resolution does not confer release authority:
///
/// ```compile_fail
/// use bombay_address::AddressSpace;
///
/// let space = AddressSpace::new();
/// let _lease = space.claim("worker", String::from("endpoint")).unwrap();
/// let endpoint = space.resolve(&"worker").unwrap();
/// endpoint.release();
/// ```
///
/// `Send` and `Sync` are inherited from `E`; the wrapper does not manufacture
/// either property for an endpoint that lacks it:
///
/// ```compile_fail
/// use bombay_address::Resolved;
/// use std::rc::Rc;
///
/// fn assert_send<T: Send>() {}
/// assert_send::<Resolved<Rc<()>>>();
/// ```
///
/// ```compile_fail
/// use bombay_address::Resolved;
/// use std::cell::Cell;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<Resolved<Cell<()>>>();
/// ```
pub struct Resolved<E>(E);

impl<E: Clone> Clone for Resolved<E> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<E> core::ops::Deref for Resolved<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<E> AsRef<E> for Resolved<E> {
    fn as_ref(&self) -> &E {
        self
    }
}

impl<E: fmt::Debug> fmt::Debug for Resolved<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Resolved").field(&self.0).finish()
    }
}

impl<E: PartialEq> PartialEq for Resolved<E> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<E: PartialEq> PartialEq<E> for Resolved<E> {
    fn eq(&self, other: &E) -> bool {
        self.as_ref() == other
    }
}

impl<E: Eq> Eq for Resolved<E> {}

/// A failed attempt to claim an address that already has a live owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressInUse<A>(pub A);

/// The reason an address claim or reservation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError<A> {
    /// The returned address is already claimed or reserved.
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

    /// Return the number of occupied addresses, including unpublished reservations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.read_entries().len()
    }

    /// Return whether the address space has no claims or reservations.
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
    /// Preserves every claim, reservation, endpoint, and generation. Safe
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
    /// Returns [`AddressInUse`] when the address is already claimed or reserved.
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
    /// Returns [`ClaimError::AddressInUse`] when the address is already
    /// claimed or reserved. Returns [`ClaimError::RegistrationIdsExhausted`]
    /// after the address space has issued all `u64::MAX` identities.
    pub fn try_claim(&self, address: A, endpoint: E) -> Result<Lease<A, E>, ClaimError<A>> {
        self.acquire(address, Some(endpoint))
    }

    /// Reserve `address` exclusively without publishing an endpoint.
    ///
    /// The reservation blocks both claims and reservations at this address,
    /// counts toward [`Self::len`], and remains absent from [`Self::resolve`].
    /// Dropping it frees the address; [`Reservation::publish`] consumes it and
    /// returns a lease for the same registration identity.
    ///
    /// The key is cloned before taking the table lock, with the same key
    /// contracts as [`Self::claim`]. Errors return the original address.
    ///
    /// # Errors
    /// Returns [`ClaimError::AddressInUse`] if the address is occupied (even
    /// after identity exhaustion), or [`ClaimError::RegistrationIdsExhausted`]
    /// if it is free but all `u64::MAX` identities have been issued.
    pub fn try_reserve(&self, address: A) -> Result<Reservation<A, E>, ClaimError<A>> {
        self.acquire(address, None).map(Reservation)
    }

    fn acquire(&self, address: A, endpoint: Option<E>) -> Result<Lease<A, E>, ClaimError<A>> {
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
        entries.insert(key, generation.get(), endpoint.map(Arc::new));
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
    /// Resolve an opaque snapshot of the endpoint currently registered at
    /// `address`. Returns `None` for an unpublished reservation.
    ///
    /// The returned handle is an exact snapshot of the resolved registration:
    /// it remains valid after the lease is released or the address is reused.
    /// Capturing the registered endpoint handle while holding the read guard
    /// runs no caller code. The endpoint's [`Clone`] implementation then
    /// defines the snapshot outside the table lock, so custom clone and drop
    /// code may safely re-enter the address space.
    #[must_use]
    pub fn resolve(&self, address: &A) -> Option<Resolved<E>> {
        let guard = self.inner.read_entries();
        let endpoint = guard.get(address).and_then(|entry| entry.endpoint.clone());
        drop(guard);
        endpoint.as_deref().cloned().map(Resolved)
    }
}

/// Exclusive, unpublished ownership of one address registration generation.
///
/// A reservation is affine: it cannot be cloned, and publication consumes it.
/// Dropping it releases exactly its generation. It keeps the address space
/// alive even if all [`AddressSpace`] handles are dropped.
///
/// ```
/// use bombay_address::AddressSpace;
/// let space = AddressSpace::new();
/// let reserved = space.try_reserve("worker").unwrap();
/// let identity = reserved.registration_id();
/// assert!(space.resolve(&"worker").is_none());
/// let lease = reserved.publish("ready");
/// assert_eq!(lease.registration_id(), identity);
/// assert_eq!(space.resolve(&"worker").as_deref(), Some(&"ready"));
/// drop(lease);
/// assert!(space.is_empty());
/// ```
///
/// ```compile_fail
/// use bombay_address::AddressSpace;
/// let space = AddressSpace::<_, ()>::new();
/// let reserved = space.try_reserve(1).unwrap();
/// let duplicate = reserved.clone();
/// ```
///
/// ```compile_fail
/// use bombay_address::AddressSpace;
/// let space = AddressSpace::new();
/// let reserved = space.try_reserve(1).unwrap();
/// let first = reserved.publish(10);
/// let second = reserved.publish(20);
/// ```
#[must_use = "dropping the reservation immediately releases the address"]
pub struct Reservation<A: Eq + Hash, E>(Lease<A, E>);

impl<A: Eq + Hash, E> Reservation<A, E> {
    /// Inspect the reserved address.
    #[must_use]
    pub fn address(&self) -> &A {
        self.0.address()
    }

    /// Return the identity that publication will preserve.
    ///
    /// This value grants no ownership or release authority.
    #[must_use]
    pub fn registration_id(&self) -> RegistrationId {
        self.0.registration_id()
    }

    /// Publish `endpoint` and transfer ownership to a lease of this generation.
    ///
    /// Publication is atomic with respect to resolution. It performs no second
    /// collision check and allocates no registration identity. It therefore
    /// succeeds even when the address space's identities are exhausted.
    /// The address is neither cloned nor reinserted; its `Hash` and `Eq` must
    /// obey the [key contracts](index.html#address-key-contracts).
    #[must_use = "dropping the lease immediately releases the published endpoint"]
    pub fn publish(self, endpoint: E) -> Lease<A, E> {
        let endpoint = Arc::new(endpoint);
        self.0
            .inner
            .write_entries()
            .publish(&self.0.address, endpoint);
        self.0
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
    use super::{AddressSpace, ClaimError, Resolved};
    use std::collections::HashSet;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn resolved_inherits_positive_auto_traits_from_endpoint() {
        assert_send_sync::<Resolved<String>>();
    }

    #[test]
    fn resolved_preserves_explicit_endpoint_interior_mutability() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let state = Arc::new(AtomicUsize::new(1));
        let space = AddressSpace::new();
        let _lease = space.claim("worker", Arc::clone(&state)).unwrap();
        let endpoint = space.resolve(&"worker").unwrap();

        endpoint.store(2, Ordering::SeqCst);
        assert_eq!(state.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn lease_owns_and_releases_one_registration() {
        let space = AddressSpace::new();
        let lease = space.claim(7, "first").unwrap();
        assert_eq!(space.resolve(&7).as_deref().copied(), Some("first"));
        assert!(matches!(
            space.try_claim(7, "second"),
            Err(ClaimError::AddressInUse(7))
        ));
        drop(lease);
        assert_eq!(space.resolve(&7), None);
        let replacement = space.claim(7, "second").unwrap();
        assert_eq!(space.resolve(&7).as_deref().copied(), Some("second"));
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
    fn resolved_snapshot_survives_release_and_address_reuse() {
        let space = AddressSpace::new();
        let first = space.claim(7, String::from("first")).unwrap();
        let snapshot = space.resolve(&7).unwrap();

        first.release();
        let second = space.claim(7, String::from("second")).unwrap();

        assert_eq!(snapshot.as_str(), "first");
        assert_eq!(space.resolve(&7).unwrap().as_str(), "second");
        drop(second);
    }

    #[test]
    fn reservations_publish_after_exhaustion_without_allocating_an_identity() {
        let space = AddressSpace::new();
        space
            .inner
            .write_entries()
            .set_next_generation(u64::MAX - 1);
        let first = space.try_reserve(String::from("first")).unwrap();
        let last = space.try_reserve(String::from("last")).unwrap();
        let identity = last.registration_id();
        assert!(matches!(
            space.try_reserve(String::from("last")),
            Err(ClaimError::AddressInUse(_))
        ));
        assert!(matches!(
            space.try_claim(String::from("last"), 1),
            Err(ClaimError::AddressInUse(_))
        ));
        let address = String::from("exhausted");
        let pointer = address.as_ptr();
        let Err(ClaimError::RegistrationIdsExhausted(returned)) = space.try_reserve(address) else {
            panic!("identities exhausted");
        };
        assert_eq!(returned.as_ptr(), pointer);
        drop(first);
        let last = last.publish(99);
        assert_eq!(last.registration_id(), identity);
        assert_eq!(space.resolve(last.address()).as_deref(), Some(&99));
        last.release();
        assert!(matches!(
            space.try_reserve(String::from("last")),
            Err(ClaimError::RegistrationIdsExhausted(_))
        ));
        assert!(space.is_empty());
    }

    #[test]
    fn stale_reservation_and_lease_drops_preserve_later_generations() {
        // Public affine ownership cannot create a stale owner. Retire entries
        // internally to exercise the defensive generation gate itself.
        let space = AddressSpace::new();
        let stale = space.try_reserve(1).unwrap();
        drop(
            space
                .inner
                .write_entries()
                .remove_if(&1, stale.0.generation.get()),
        );
        let current = space.try_reserve(1).unwrap();
        drop(stale);
        assert_eq!(space.len(), 1);
        let stale = current.publish(10);
        drop(
            space
                .inner
                .write_entries()
                .remove_if(&1, stale.generation.get()),
        );
        let current = space.try_reserve(1).unwrap().publish(20);
        drop(stale);
        assert_eq!(space.resolve(&1).as_deref(), Some(&20));
        drop(current);
        assert!(space.is_empty());
    }

    #[test]
    fn failed_acquisitions_and_publication_do_not_consume_generations() {
        let space = AddressSpace::new();
        let first = space.try_reserve(1).unwrap();
        assert!(space.try_reserve(1).is_err());
        assert!(space.try_claim(1, ()).is_err());
        let first = first.publish(());
        let second = space.try_reserve(2).unwrap();
        assert_eq!(first.generation.get(), 1);
        assert_eq!(second.0.generation.get(), 2);
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

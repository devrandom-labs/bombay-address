# Bombay Address

`bombay-address` is a typed concurrent address space with exclusive,
generation-safe ownership. The published package is `bombay-address`; its Rust
library name is `bombay_address`.

```toml
[dependencies]
bombay-address = "0.3"
```

```rust
use bombay_address::AddressSpace;

let space = AddressSpace::new();
let lease = space.claim("worker-7", "mailbox")?;

assert_eq!(space.resolve(&"worker-7").as_deref().copied(), Some("mailbox"));
drop(lease);
assert_eq!(space.resolve(&"worker-7"), None);
# Ok::<(), bombay_address::AddressInUse<&str>>(())
```

`AddressSpace::resolve` returns an opaque `Resolved<E>` capability. It avoids
exposing Address's storage or reclamation mechanism while preserving the
endpoint-defined `Clone` semantics that construct each non-owning snapshot.

The `Lease` is the authority to release one exact registration generation.
Dropping an old lease cannot remove a newer owner. Resolved endpoints are typed
snapshots, and user `Clone`/`Drop` code runs outside the table lock.

`Lease::registration_id()` returns an opaque identity for that exact local
registration. It is independent of the address type, process-local, and grants
no ownership, release authority, authentication, or durable identity.

## Reserve before publishing

```rust
use bombay_address::AddressSpace;

let space = AddressSpace::new();
let reserved = space.try_reserve("worker-7")?;
let identity = reserved.registration_id();
assert!(space.resolve(&"worker-7").is_none());

let lease = reserved.publish("mailbox");
assert_eq!(lease.registration_id(), identity);
assert_eq!(space.resolve(&"worker-7").as_deref(), Some(&"mailbox"));
drop(lease);
assert!(space.is_empty());
# Ok::<(), bombay_address::ClaimError<&str>>(())
```

A `Reservation` blocks claims and reservations at its address. It cannot be
cloned; dropping it releases its exact generation. `publish` consumes it and
returns the existing `Lease` type without a second collision check, address
clone, table insertion, or generation allocation. Publication can succeed even
after registration identities are exhausted. `len()` counts both reservations
and published registrations; only published endpoints can be resolved.

`try_reserve` and `try_claim` return `ClaimError` with the original address.
An occupied address returns `AddressInUse` even after identity exhaustion.

## Verification

```bash
nix flake check -L
nix build .#coverage -L
```

The bounded models, property tests, stress tests, fuzz targets, replay corpus,
and findings record live under `research/address-tests/`.

See the [guide](https://docs.page/devrandom-labs/bombay-address) and
[API reference](https://docs.rs/bombay-address).

Licensed under Apache-2.0 or MIT, at your option.

# Bombay Address

`bombay-address` is a typed concurrent address space with exclusive,
generation-safe ownership. The published package is `bombay-address`; its Rust
library name is `address`.

```toml
[dependencies]
bombay-address = "0.1"
```

```rust
use bombay_address::AddressSpace;

let space = AddressSpace::new();
let lease = space.claim("worker-7", "mailbox")?;

assert_eq!(space.resolve(&"worker-7"), Some("mailbox"));
drop(lease);
assert_eq!(space.resolve(&"worker-7"), None);
# Ok::<(), bombay_address::AddressInUse<&str>>(())
```

The `Lease` is the authority to release one exact registration generation.
Dropping an old lease cannot remove a newer owner. Resolved endpoints are typed
snapshots, and user `Clone`/`Drop` code runs outside the table lock.

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

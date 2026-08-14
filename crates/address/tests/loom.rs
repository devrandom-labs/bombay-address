#![cfg(loom)]

use bombay_address::AddressSpace;
use loom::sync::Arc;
use loom::thread;

#[test]
fn resolution_never_observes_a_partially_published_registration() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let writer = space.clone();
        let reader = space.clone();

        let claim = thread::spawn(move || {
            let lease = writer.claim(1_u64, 99_u64).unwrap();
            assert_eq!(writer.resolve(&1).as_deref().copied(), Some(99));
            drop(lease);
        });
        let resolve = thread::spawn(move || {
            if let Some(endpoint) = reader.resolve(&1) {
                assert_eq!(endpoint, 99);
            }
        });

        claim.join().unwrap();
        resolve.join().unwrap();
    });
}

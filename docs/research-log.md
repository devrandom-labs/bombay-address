# Research log

Record every experiment with its hypothesis, primary sources, implementation,
correctness results, benchmark distribution, allocation/memory results, and
decision. Preserve rejected experiments so later sessions do not repeat them.

## Initial baseline

Safe `std::sync::RwLock<std::collections::HashMap>` with cloned endpoints and
generation-bearing RAII leases. This establishes semantics; no performance
claim has been made.

## Starting primary sources

- Shalev and Shavit, *Split-Ordered Lists: Lock-Free Extensible Hash Tables*:
  https://people.csail.mit.edu/shanir/publications/Split-Ordered_Lists.pdf
- Click, *A Lock-Free Wait-Free Hash Table*:
  https://web.stanford.edu/class/ee380/Abstracts/070221_LockFreeHash.pdf
- Abseil Swiss Tables design notes:
  https://abseil.io/about/design/swisstables
- Prokopec et al., *Cache-Aware Lock-Free Concurrent Hash Tries*:
  https://arxiv.org/abs/1709.06056
- Kelly et al., *Concurrent Robin Hood Hashing*:
  https://arxiv.org/abs/1809.04339
- Crossbeam epoch reclamation documentation:
  https://docs.rs/crossbeam/latest/crossbeam/epoch/


# Addresspass adversarial test-only autoresearch

Add tests and test infrastructure only under
`research/addresspass-autoresearch/**`. Production, manifests, existing tests,
docs, `.auto`, and the launcher are immutable. Never fix, optimize, refactor,
or tweak production.

Aggressively attack atomic collision exclusion, resolve/claim/release races,
stale-generation release, replacement isolation, endpoint lifetime and clone
reentrancy, hash collision behavior, generation exhaustion boundaries,
poison/recovery assumptions, allocation retention, and linearizability. Use an
independent state-machine model, exhaustive small histories, proptest, fuzzing,
deterministic stress, bounded real-protocol Loom, and Miri ownership/drop tests.

For every defect, minimize it, add a deterministic
`#[ignore = "FINDING-NNN: reason"]` regression, and add `## FINDING-NNN` to
`RESEARCH-REPORT.md` with expected/actual behavior, severity, version, seed,
and exact command. Passing tests remain active. Never fix the finding. Report
all run bounds and interrupted verification honestly.

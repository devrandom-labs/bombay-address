# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/devrandom-labs/bombay-address/compare/bombay-address-v0.1.1...bombay-address-v0.2.0) - 2026-08-14

### Other

- *(resolve)* return opaque endpoint snapshots ([#8](https://github.com/devrandom-labs/bombay-address/pull/8))

### Changed

- Make `AddressSpace::resolve` return the opaque, read-only `Resolved`
  capability instead of the endpoint value directly, preventing mutation and
  keeping the internal reclamation mechanism private.

### Fixed

- Correct stale table documentation and test naming that referred to the
  replaced SplitMix hasher.

## [0.1.1](https://github.com/devrandom-labs/bombay-address/compare/bombay-address-v0.1.0...bombay-address-v0.1.1) - 2026-08-07

### Added

- *(address)* expose exact registration identities ([#6](https://github.com/devrandom-labs/bombay-address/pull/6))

### Other

- release v0.1.0 ([#5](https://github.com/devrandom-labs/bombay-address/pull/5))

## [0.1.0](https://github.com/devrandom-labs/bombay-address/releases/tag/bombay-address-v0.1.0) - 2026-08-06

### Other

- prepare bombay-address for publication
- establish address-space research baseline

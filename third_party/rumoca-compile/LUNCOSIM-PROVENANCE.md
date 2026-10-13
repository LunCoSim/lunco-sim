# Rumoca compile source provenance

This is `crates/rumoca-compile` from the maintained LunCoSim Rumoca fork at
`83641b8cf7247815d0ab80b3262113cb8622be5c` (version 0.10.2, Apache-2.0),
the same dependency revision used by the tutorials checkout.
The manifest expands inherited package/dependency/lint declarations; sibling
Rumoca dependencies retain the exact same Git revision.

The maintained revision supplies the session-revision package-definition cache
and its edit/removal/foreign-cache validation test unchanged. There is no local
cache implementation or test counter.

The local correction carries the MLS base-class lookup context. An `extends`
name is resolved before expanding that class's own inherited contents, while
enclosing inherited scopes remain available. Recursive base lookup therefore
does not explore permutations of sibling extends clauses. Mutable and snapshot
readers share this implementation. The focused inline multiple-base fixture
counts scope reads, checks enclosing inheritance and rejects unavailable bases
and cycles; existing declaration/import/type-prefix fixtures cover identity and
invalidation.
Cached class interfaces are immutable shared `Arc` values. Mutable and snapshot
lookups clone the handle rather than every member and nested-class header on
each scope visit. The existing per-file fingerprint cache still replaces an
edited interface, while retained snapshots keep their admitted facts. The
per-file invalidation fixture checks handle reuse, replacement and isolation.
An unused internal wildcard import is removed.

The root Git patch consumes this package directly. Its development profile keeps
the upstream dependency's level-3 code generation. Dependency updates must retain
the base-class lookup context and source invalidation contract.

Trailing whitespace in the upstream package README is normalized.

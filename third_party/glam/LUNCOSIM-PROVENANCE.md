# glam source provenance

This is the published `glam` 0.32.1 source (MIT OR Apache-2.0), with its
licences retained. It supplies the existing Bevy 0.19 / Parry 0.27 math types.
The root crates.io patch consumes this source directly.
The original archive has SHA256
`f70749695b063ecbf6b62949ccccde2e733ec3ecbbd71d467dca4e5c6c97cca0`.

Aligned boolean masks retain their native tuple serialization in both scalar
and SIMD builds. The public mask types exist in both configurations;
`BVec3A`/`BVec4A` serialization cannot disappear when physics enables scalar
enhanced determinism. `BVec4A` serializes all four distinct lanes, preserving
its fourth lane on roundtrip. Native scalar and SIMD serde tests cover valid
roundtrips and malformed tuple lengths; the mixed-mask truth table checks every
lane combination. Tuple visitors use the actual aligned mask's type name. The
imported package is a workspace member so these native tests use the repository
target and profile. Its explicit development profile retains level-3 native
math code generation and development invariant checks; Cargo's dependency
wildcard excludes workspace members.

Functional changes are confined to `src/features/impl_serde.rs`; imported
documentation and templates have trailing whitespace normalized. No vector layout, floating-point
algorithm, scalar precision or reflection capability changes. Dependency
updates must retain these serialization contracts and compile Bevy reflection
with scalar math enabled.

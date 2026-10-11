# Parry f64 source provenance

This is the published `parry3d-f64` 0.27.0 crate, under Apache-2.0, with its
licence retained. The original crate archive has SHA256
`c1f331c2ecfa5c34dfe379eff7efb824e5e8cfcc933d9624d74e7f867ea696dd`.
Cargo consumes this local source through the root crates.io patch, retaining
Avian 0.7 / Bevy 0.19 / glam 0.32 type compatibility.

`src/query/gjk/gjk.rs` uses the maintained implementation published in
`parry3d-f64` 0.31.1. Native distance bounds classify stalled simplices;
degenerate support directions cannot discard overlapping axisymmetric shapes.
Native ray casting refines impact distance from witnesses and scales its
rounding-error test with support magnitude. See upstream
[GJK corrections](https://github.com/dimforge/parry/blob/master/CHANGELOG.md#0302).
The 0.31.1 archive has SHA256
`ffb9e04261f1783d8f66f36767b2b73132a63c9b30b12a5ed47e1faa240f7591`.

The `enhanced-determinism` feature enables `glamx/scalar-math`, matching the
upstream 0.29 correction. It retains libm and ordered maps while removing
architecture-dependent vector association. No scalar type or scene precision
changes. Native query adapters retain failures and reject incomplete unbounded
queries before continuous-contact writeback.

Dependency updates must pass native overlapping-cylinder, finite-feature and
witness tests, plus the production drive, continuous-contact, landing-impact
and rover-leg gates. Cross-platform replay remains a separate verification gate.

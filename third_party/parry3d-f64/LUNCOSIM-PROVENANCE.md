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

The native nonlinear motion evaluator retains the supplied pose exactly at time
zero and uses the analytic pivot displacement for rotating motion. Pure
translation retains the original rotation and does not compose cancelling world
translations. The native directional initial-overlap handler bounds relative
point speed with upward-rounded L1 norms and both shapes' bounding radii about
their actual local pivots. It omits contact sampling only when that bound proves
the existing native thickness criterion unreachable over the remaining interval.
This is a local generic kernel optimisation, not an upstream release backport.
Positive-time impacts and stop-at-penetration casts retain native geometry.
Zero thickness, uncertain bounds and motion above the threshold retain sampling.
Neither the CCD threshold nor the contact/convergence tolerance changes.
The same native relative-point displacement bound is exported for continuous
constraint consumers. It includes upward-rounded translation and both angular
pivot radii and a native pose/witness arithmetic margin at the actual coordinate
and geometry scale. The continuous owner may omit convex-leaf query geometry
when this complete bound fits its existing residual tolerance: every support gap,
including the unknown initial contact axis, changes by no more than that bound.
Changed trajectories re-establish the proof; larger or uncertain bounds keep
native queries. A large-coordinate fixture reproduced a rounded pose increment
exceeding a velocity-only bound; the arithmetic margin retains its query.
`NonlinearShapeCastMode::initial_overlap_motion_is_inert` owns the strict native
directional-impact criterion. Both the native initial-penetration handler and
the continuous owner consume it. A continuous consumer must prove initial
intersection before using it; a real finite triangle point strictly inside a
cylinder supplies that proof with an arithmetic margin. Initially separated
shapes retain future-impact geometry. Prepared material inequalities still
retain their existing position tolerance. One-, multi-thread and invalid-bound
fixtures exercise the unchanged native criterion and geometry boundaries.
This is distinct from the native initial-overlap thickness test.

The native cylinder point query chooses the nearest cap or side consistently
when distances tie. Equal cap distances cannot select a farther side surface.
This local correction retains native point projection and signed-distance
semantics, including rigid transforms; it is not an upstream release backport.

Native CSO support points retain their generating direction. At an exact
single-point touching simplex, EPA returns those material witnesses and that
support normal. A raw point without support provenance cannot supply a normal;
EPA reports no result. The reproduced non-origin corner-contact fixture failed
with fabricated zero witnesses before this local correction. Both dimension
implementations consume the same support provenance, including CSO translation.

The existing parameterised native contact entry point accepts reusable EPA
scratch beside its simplex. Each native query resets current geometry while
retaining buffer capacity. The continuous-contact owner shares this scratch;
queries with varied geometry and both pair orientations must match fresh native
queries exactly. No previous contact or polytope becomes a new query's geometry.

Dependency updates must pass native overlapping-cylinder, finite-feature and
witness tests, plus the production drive, continuous-contact, landing-impact
and rover-leg gates. Cross-platform replay remains a separate verification gate.

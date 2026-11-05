# Parameter initialization, offline capture and slope diagnostics

The compiler keeps an enclosing component reference as a dependency when passing a
parameter into a nested modifier. The simulator retains compiled-artifact reuse;
it does not flatten the dependency into an old literal, copy values into children,
or force recompilation. The canonical Rumoca Git dependency is pinned to
`87b117a0c28c021977f4c0fbe483e0d4c938887c`. Its commit is local and unpublished.
A one-command Git transport override fetched that local repository while retaining
the canonical HTTPS source and exact revision in the manifest and lockfile.

The recording clock now finishes already-admitted fixed steps across a causal
hold before accepting the next image. Explicitly paused/held camera shots remain
valid. Runtime timing changes already present on main are preserved.

The slope diagnostic converts its geometric normal back into the terrain frame
using the existing normal transform, before comparing it with local +Y. Rotating
a celestial frame must not change hillside slope. Production material and physics
are unchanged. Diagnostic colour gain preserves contrast under filmic tonemapping;
optional, derivative-filtered degree bands show equal slope, not equal elevation.
The reflected material settings own both gain and band spacing. A coarse display
mesh is still not a substitute for the authoritative TerrainField query. Corrected
module documentation no longer claims an unbound normal map is being sampled.

## Validation

- Integrated Rumoca: 33 modifier tests and five initialization tests pass.
- Integrated simulator: 16 artifact/cache tests and five API capture tests pass,
  including reused nested initialization and both automatic-cycle/held-shot cases.
- Headful native GPU fixture: production -> slope -> LOD depth -> production,
  with depth, normal and motion prepasses; all four pixel phases pass, with a clean
  GPU log and normal owned-session shutdown.
- The visible film session additionally verifies repeated 60 -> 1 -> 2 -> 60 metre
  initialization and complete per-image fixed-step cadence. It uses the separately
  pinned B8 debug executable; it is not claimed to be a fresh build of merged main.

## Explicit local landing exception

Rumoca's `cargo xtask verify gate --rev 87b117a0c --coverage` passed format,
all-feature Clippy and lint, then failed five architecture checks inherited from
`a11b4b2e`: FMI Clone policy, ignored-test hygiene, session.rs size, the finalized
JIT source check and ir-solve totality assertion ceiling. The relevant failing
sources were unchanged by this fix. Coverage was not reached; GATE_OK is not claimed.
The user explicitly approved this scoped local merge with the five failures
recorded. No full-cohort, coverage, cross-platform or live 100 fps claim is made.
No dependency or simulator commit was pushed.

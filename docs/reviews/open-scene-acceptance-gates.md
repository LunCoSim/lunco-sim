# Open scene acceptance gates

Status: the Twin replacement and cancellation boundaries pass focused production
checks. Whole-runtime acceptance remains open until the integrated sequential
sweep and the outstanding physical-contact, reference-authoring and determinism
gates pass. A passing lifecycle test is not proof that every scene is usable.

## Reproduce

Build the production binary with:

```sh
cargo build -p lunco-luncosim --bin luncosim --features python -j 4
LUNCOSIM_BIN="$PWD/target/debug/luncosim" \
EDITOR_API_PORT=4800 RENDER_API_PORT=4305 \
./scripts/run_scene_tests.sh --no-build -j 1
```

The runner discovers tests through the production binary and runs one process
at a time: 36,000 ticks, one compute thread, zero jitter, a 420-second startup
budget and a 900-second scene budget. Render and editor wrappers own their
bounds and API session shutdown. Runtime overlay I/O is disabled except in the
separate manifest-backed persistence test.

## Verified focused boundaries

Production evidence from 2026-10-16:

| Boundary | Observed result |
|---|---|
| Menu replacement | 11 groups / 68 checks pass. |
| Late loading and initialization failure recovery | 6 groups / 32 checks pass. |
| Twin retirement, replacement, reopening and rejected replacement | 95 groups / 384 checks pass. |
| Pending articulated physics admission cancellation | 4 groups / 32 checks pass; replacement occurs with real pending joints and a world hold. |
| Native input and reload | Four authored checks pass through injected window input. |
| Property editor | Six checks pass, including actual gizmo transform editing, blank-space preview pan and document changes. |
| Route interaction | 38 checks pass through native window input. |
| Battery graphics | Three production GPU frames pass; the static fixture disables its native joints and uses real USD activation metadata. |
| Differential variants | Both physical differential fixtures pass. |
| Solar energy cycle | 49 checks pass, including physical travel, loaded discharge, charging and settled antenna target tracking. |
| Wi-Fi | Eight checks pass with an authored lunar frame. |
| Physical rover controls | 33 checks pass over 37 simulated seconds after the native geometry repair. |
| Unpowered landing gear | 48 checks pass at 8.67 simulated seconds, including limits, contact penetration and convergence. |

The full lander mission also passed a focused production run: 209.117 simulated
seconds / 12,547 ticks, including touchdown, deployment, three rover waypoints,
quiet propulsion after handoff, and rejected stale control. Its lesson passes.
Assembly workflow, save/clean workspace state, attachment, undo/redo, battery
mounts, base building, selection/AI, document audit and referenced-part editing
also pass focused runs. Shared physics changes still require integrated coverage.

## Outstanding verification

| Boundary | Current evidence and owning follow-up |
|---|---|
| Document command admission | Cold authoring and warm restoration pass through `test_route_runtime_persistence.py`. The eleven-check route history gate passes atomic reference edits, undo, exact-document close cancellation, stale-discard rejection, dome precision, terminal command rejection, unmounted reference preparation and discard cancellation on close. Six native command-owner tests pass. The production startup initialization fixture also passes after native dependency admission. |
| Determinism reference | All ten profiles match exactly in twenty independent production runs with every behavioral assertion and the altered-row discriminator enabled. The maintained reference is updated; replay with every comparison required to pass remains open. |
| Startup fault classification | `test_scene_runner_startup_fault.py` passes: the owned production process returns exit 1 during preparation with `modelica-initialization-failed`, without panic or timeout. |
| Full suite | The initial complete sweep found 163 gates: 97 pass, 46 fail, 19 without a verdict and one timeout. Many focused corrections now pass; those initial counts are not current integrated acceptance. Repeat the full sequential sweep after the native changes. |

The reusable lander authors native CCD on its four thin landing-body owners.
The full mission passes after that change. Native physics tests pass 92 cases,
including the two exact contact regressions, degenerate triangle projection,
and scene-owned geometry release.

Live mutation persistence uses exact stage/document leases and the shared
prepared/live reader for composed prim identity. The route-variant scenario
passes its nine runtime assertions after the document-tool correction to its
route-only program anchor.
The broader retirement gate passes 95 groups / 384 checks after the lease cutover.

The native mount-replacement seam also passes: stage authority follows the exact
active mount and ends before deferred despawn. The rebuilt command owner still
requires focused checks and the integrated sequential sweep before acceptance.

## Architectural owners

Scene-derived resources register initialization and teardown together through
`AppSceneExt::init_scene_resource` / `SceneResource`. Physics contact geometry,
joint-order and island-mass caches, bounds, holds and pending USD projection
work follow this boundary. Application configuration has an explicit retained
lifetime. Weak-reference coverage verifies collider geometry is released
without waiting for a replacement physics tick.

Native Avian joint retirement wakes every affected island. Remove observers
validate the destination archetype before reactivating a disabled joint, so
hierarchy destruction cannot recreate a joint after its bodies retire.

The fixed-step Modelica adapter uses Rumoca's Model Exchange host for
initialization and retained input/observation state. `lunco-modelica-solver`
supplies the fixed RK4 numerical plugin and continuous extension. Rumoca's
batch trace importer does not provide the retained live-session interface.
Rover command selection is centralized in authored `RoverCommandAuthority`.

USD Save writes and marks the document clean at `lunco-usd-commands`, then
publishes `DocumentSaved`; workspace mirroring consumes that event. Typed USD
queries distinguish authored opinions from requested schema defaults. Native
composition owns prim existence and reference traversal.

Gizmo picking and transform evaluation consume the same Bevy pointer state.
The vendored integration records its maintained fork revision; the geometry
engine remains pinned to that revision. Production input injection does not
move the operating-system pointer.

The passive antenna owns geometry and its endpoint. The active wrapper owns
its two rigid bodies, mass properties, controller and native joints. Active
slewing and passive attachment gates pass under real travel.

## Validation and resource handling

The registered native lifecycle and projection owners passed 133 low-level
tests before the latest numerical-contact repair. Reuse that evidence only
where subsequent code changes do not invalidate it. Capture the final native
and integrated production results separately.

Disk recovery used checkout-local `cargo clean`, including package-scoped
cleans, while preserving source, sibling worktrees and shared caches. The first
full-clean evidence archive is outside `target`; restored logs retain their
original timestamps and are not fresh validation. Every controllable runtime
uses an explicit owned API port and verifies process/port release after Exit.

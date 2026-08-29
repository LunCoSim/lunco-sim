# 46 — BigSpace maintenance notes

> Status: Active · Audience: maintainers diagnosing precision or frame bugs.

Keep this page operational. The complete contract is in
[45 — BigSpace and reference-frame contract](45-big-space-correct-usage.md).

## Ownership map

| Concern | Owner | Canonical boundary |
|---|---|---|
| Semantic astronomical frame | `lunco-celestial` | `ReferenceFrame`, `FrameTree` |
| Grid lookup | `lunco-celestial` | `ReferenceFrameIndex` |
| f64 pose composition | `lunco-core` | `ActiveFramePoseQuery`, frame helpers |
| Cell/local split | `big_space` | `Grid::translation_to_grid` |
| Camera origin | `lunco-core` + `lunco-usd-bevy` | persistent `OriginAnchor`; viewport projects the selected camera pose into its `WorldGrid` cell |
| Terrain blueprint coordinates | `lunco-render-bevy` | `blueprint_origin` plus the active-frame origin/inverse rotation restore authored flat-grid coordinates |
| Physics pose bridge | `lunco-usd-avian` | explicitly bound `ActivePhysicsFrame` bridge |
| Scene ownership | `lunco-core` / `lunco-usd` | `SceneMountState`, typed transitions |

## Correct data flow

```text
USD or ephemeris fact
        ↓
typed f64 semantic pose
        ↓
source/target frame conversion
        ↓
destination Grid::translation_to_grid
        ↓
atomic ChildOf + CellCoord + Transform
        ↓
BigSpace propagation / Avian bridge / renderer
```

The reverse path returns to the semantic f64 pose before any cross-frame
calculation. A cell/local pair is private representation state and must not
cross a network, API, Modelica, Rhai, or user-facing model boundary.

## Review checklist

When a camera, body, rover, trajectory, or line jitters:

1. Identify its semantic `ReferenceFrame` and its owning `Grid`.
2. Check that the frame index has exactly one declaration.
3. Verify the producer computed an f64 pose in that frame.
4. Verify the final split used the destination grid's
   `translation_to_grid` exactly once.
5. Verify the entity is attached atomically and has no competing transform
   writer.
6. For physics, verify that the application/scene mount explicitly bound
   `ActivePhysicsFrame` to the intended `WorldGrid` or site grid, then verify
   the bridge path, not the
   rendered `GlobalTransform`.
7. For a terrain-attached periodic line or grid, verify the shader receives the
   current `WorldGrid` cell offset plus the active frame's render-space origin
   and inverse rotation before computing the pattern; render-relative global X/Z
   alone is not an authored coordinate.
8. For a scene replacement, verify the old root was invalidated before
   deferred despawn and that projection only accepts the active root.
9. For physics admission, verify the bridge's frame diagnostic and
   `PhysicsHolds::FRAME_CONTRACT` are clear before `StepSimulation` can run.
10. For site lighting and solar poses, verify there is no more than one
   `SiteAnchor`; ambiguous authoring must produce a diagnostic and no selected
   site frame.
11. For a repeated presentation solve, verify equal `Transform` and
   `CellCoord` results are not assigned; otherwise Bevy change detection will
   legitimately wake BigSpace propagation on every solve.
12. Check shared interaction easing, mounted/path camera followers, and the
    persistent camera-origin tracker as camera pose owners; they must obey the
    same value-idempotent commit rule.

Do not add a guard that corrects an invalid pose after the fact. Fix the
producer, frame declaration, or ownership boundary that made the invalid state
possible.

## Focused verification

```sh
scripts/run_rust_tests.sh -p lunco-core --lib -j 8
scripts/run_rust_tests.sh -p lunco-celestial -j 8
scripts/run_rust_tests.sh -p lunco-usd-avian -j 8
RUSTC_WRAPPER= cargo build -p lunco-luncosim --bin luncosim -j 8
```

For a production smoke, launch the built binary with an explicit API port.
Use a head-full launch for visual acceptance; use `--no-ui` only for a
headless deterministic test. Read `/api/ready`, record any terminal fault,
then send the typed API `Exit` command and verify the process and port are
gone.

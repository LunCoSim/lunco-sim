# 51 — Cinematic Camera Paths

Status: design. Supersedes nothing; extends [35-animate-perspective](35-animate-perspective.md),
whose build-order step 1 (camera-track cuts) is already shipped.

Goal: fly a camera around the moonbase crater on an authored path, edited live the way
rover waypoints are edited, played back as an **animation** (not a behaviour tree), and
activated like any other camera.

---

## 1. What already exists

The substrate is largely built. This design mostly *composes* it.

| Capability | Where | State |
|---|---|---|
| `UsdOp::SetTimeSample` / `RemoveTimeSample` | `lunco-usd/src/document.rs:303,325` | Real, journaled, undoable, tested |
| marker → plan → sample funnel (`UsdAnimated` → `AnimationPlan` → `sample_usd_animation`) | `lunco-usd-bevy/src/lib.rs:425,439,2381` | Real; drives xform + visibility + material |
| Rotation channels incl. `xformOp:orient` slerp | `lunco-usd-bevy/src/lib.rs:2771` (`local_rotation_at`) | Real |
| `UsdGeomCamera` → ECS (`focalLength`, `clippingRange`, `projection`) | `lunco-usd-bevy/src/camera.rs` | Real; `focusDistance` unread (no DOF) |
| Data-driven cuts (`token lunco:activeCamera.timeSamples`) | `lunco-usd-bevy/src/camera_track.rs` | Real (doc 35 step 1) |
| Transport (`ControlAnimation`, `Playback`, `AnimationPreview` domain) | `lunco-time/src/domain.rs:86,584,608` | Real |
| Transport UI (play/pause/scrub/rate) | `lunco-sandbox-edit/src/ui/inspector.rs:820` | Real, ~45 lines |
| `SetActiveCamera { name }` + `reconcile_scene_viewport` | `lunco-usd-bevy/src/camera_switch.rs` | Real, sole viewport authority |
| Ground pick (DEM oracle + collider, render→world) | `lunco-sandbox-edit/src/ui/checkpoint_click.rs:216` (`pick_ground_world`) | Real, directly reusable |
| `catmull_rom_path` (shared by autopilot + ribbon) | `lunco-autopilot` | Real, has `closed` flag |
| Cursor-mode gating, `CancelIntent` → `#[Command]` | `lunco-core/src/lib.rs:~440-512` | Real |
| Working precedent scene | `assets/scenes/sandbox/lander_cinematic.usda` | Real, runs today |

**Gaps** that bear on this work:

- **No interpolation layer in-repo.** Interp is delegated to the `openusd` crate: USD
  Linear default, Held for non-lerpable tokens. No bezier, no tangents, no per-key
  interp control. `lunco:interp` exists only in doc 35 prose.
- **No timeline UI** beyond the inspector slider. Doc 35 steps 2–7 unimplemented.
- **No keyframe-authoring command** reachable from API/MCP/rhai — `SetTimeSample` is
  only reachable by constructing the `UsdOp` directly.
- **Arbitrary attributes are not projected** to ECS; only the fixed channel set is sampled.
- **Editor undo is ECS-only** (`UndoStack`) while typed undo lives on `DocumentHost` —
  Ctrl+Z does not undo waypoint edits today, and will not undo camera-path edits either
  unless routed through the document.

## 2. The crater problem — read this first

**There is no addressable crater in moonbase.** Craters are a procedural terrain layer,
not prims:

```usda
def Xform "Craters" ( prepend apiSchemas = ["LunCoTerrainLayerAPI"] )
{
    uniform token lunco:layer = "craters"
    float lunco:layer:density   = 1.5     # per hectare
    float lunco:layer:sizeMode   = 22.0   # modal diameter, m
    float lunco:layer:depthRatio = 0.3
    int   lunco:layer:seed       = 12345
}
```

Stamps are analytic and seeded (`make_crater_layer` in `lunco-terrain-surface/src/terrain.rs`);
nothing exposes "crater #7 is at (x,z) with radius r". So "orbit *the* crater" has no
referent yet. Three ways out, in order of preference:

1. **Pick a centre by eye** and author it as the path's `lunco:path:center`. Zero new code.
   Modal crater diameter is 22 m — small. For an orbit that reads cinematically you likely
   want a *large* stamp, which the procedural layer may not have produced anywhere good.
2. **Author an explicit hero crater** via the terrain edit tools (doc: far-field + edit),
   giving it a known centre/radius. Best result, moderate work, and it makes the shot
   reproducible across terrain reseeds.
3. Expose stamp positions from the terrain layer as queryable data. Most work; only worth
   it if something else needs it.

Recommendation: **(2)**, falling back to (1) for a first look. A seed-dependent centre is a
scene that breaks silently when the seed changes.

## 3. Where the scene lives

Moonbase is **a separate repo**, not this one: `/home/rod/Documents/lunco/moonbase`
(twin folder `twin/`, manifest `twin.toml` → `default_scene = "moonbase_scene.usda"`).
The copy under `main/dist/sandbox/assets/twins/moonbase/` is a stale deploy — do not edit it.

Derive exactly as `lander_cinematic.usda` derives from `lander_test.usda`:

```usda
#usda 1.0
(
    defaultPrim = "MoonbaseScene"      # keep the base's defaultPrim name
    upAxis = "Y"
    metersPerUnit = 1.0
    timeCodesPerSecond = 1             # 1 timecode == 1 second; keys read as seconds
    startTimeCode = 0
    endTimeCode = 60
    subLayers = [ @moonbase_scene.usda@ ]
)

over "MoonbaseScene" ( doc = "Cinematic crater orbit." )
{
    def Camera "CraterOrbit" { ... }   # baked timeSamples land here
    def Scope "CameraTrack" { token lunco:activeCamera.timeSamples = { 0: "CraterOrbit" } }
}
```

New file: `/home/rod/Documents/lunco/moonbase/twin/moonbase_cinematic.usda`.

Two notes. Moonbase authors **no `def Camera`** at all today (camera state rides on the
`Avatar` prim via `lunco:cameraMode`/`cameraYaw`/`cameraPitch`), so this scene introduces
the first one. And `timeCodesPerSecond` defaults to 24 — set it to 1 as the precedent does,
so keys read as seconds and line up with transport.

To surface it in the web UI: `LC_TWIN_EXTRA="moonbase_cine=moonbase_cinematic.usda=/home/rod/Documents/lunco/moonbase/twin"`
at build, or edit `dist/sandbox/scenes.json` post-deploy. Desktop picks it up via `--scene`.

## 4. Core decision — knot prims, baked to timeSamples

Two representations, and the split is the whole design.

**Authoring form**: a `def Scope "CameraPath"` with child `def Xform "Knot_*"` prims. Each
knot carries a full captured pose plus shot metadata.

**Playback form**: dense `xformOp:translate` + `xformOp:orient` `timeSamples` on the
`def Camera`, produced by a bake step.

### Why not author timeSamples directly?

Because **there is no interpolation layer**. USD Linear between sparse keys gives you
constant-velocity segments with hard direction changes at every key — a robotic dolly, not
a crane move. Euler channels lerp per-axis, which is worse. The repo already owns the fix:
`catmull_rom_path` (shared by the autopilot and the waypoint ribbon, so the drawn curve *is*
the driven curve). Baking dense samples through Catmull-Rom + an ease curve buys smooth
motion with **zero new interpolation code** and no change to the sampler.

Bake `xformOp:orient` (quaternion), not `rotateXYZ` — the sampler already slerps `orient`
(`local_rotation_at`), which sidesteps gimbal and per-axis Euler lerp artifacts.

### Why not the waypoint storage pattern?

The shipped waypoint editor bakes coordinates into a BT XML string in one attribute
(`lunco:behavior`, `target="x;y;z"`). Its own design doc (`docs/waypoints-in-usd-design.md`)
specifies prim-backed storage and calls this drift out as still-open. **Do not copy it.**
An XML blob is opaque to the inspector, to `over` composition, to selection, and to
per-knot gizmos. Camera paths get real prims from the start.

### Schema sketch

```usda
def Scope "CameraPath" ( prepend apiSchemas = ["LunCoCameraPathAPI"] )
{
    uniform token lunco:path:mode      = "orbit"   # "orbit" | "spline"
    uniform token lunco:path:target    = "CraterOrbit"  # camera prim to bake into
    double        lunco:path:duration  = 60.0     # seconds
    bool          lunco:path:closed    = true
    uniform token lunco:path:easing    = "inout"  # "linear" | "in" | "out" | "inout"
    double        lunco:path:sampleHz  = 12.0     # bake density

    # orbit mode: knots are generated, not authored
    double3 lunco:path:center          = (120, 1946, -80)
    double  lunco:path:radius          = 90.0
    double  lunco:path:height          = 35.0
    double  lunco:path:revolutions     = 1.0

    # spline mode: authored knots
    def Xform "Knot_0" ( prepend apiSchemas = ["LunCoCameraKnotAPI"] )
    {
        double3 xformOp:translate = (58, 1978, 58)
        quatd   xformOp:orient    = (0.92, -0.21, 0.33, 0.07)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient"]
        float   lunco:knot:focalLength = 24.0
        double  lunco:knot:dwell       = 0.0   # hold here, seconds
        rel     lunco:knot:lookAt              # optional: aim at a prim, overrides orient
    }
}
```

Both modes converge on one bake. Orbit mode generates knots on a circle about `center`;
spline mode uses the authored ones. `lunco:path:mode` is the only branch.

Aim resolution at bake time, in priority order: `lunco:knot:lookAt` rel → `lunco:path:center`
(orbit mode always frames its centre) → the captured `orient`. Baking the aim means no new
runtime aiming system — but note the tradeoff: a `lookAt` at a *moving* prim can't be baked
correctly. For moving targets the existing **camera mount** path is already the right answer
(`camera_mount.rs` re-aims every frame), and `lander_cinematic.usda` shows exactly that split:
hand-keyframed free cameras for static geometry, mounted cams for movers. Keep that split.

### Bake step

Mirrors the waypoint compile (`compile_behavior_xml`) but prim→prim rather than XML→spec:

```
CameraPath prims  ──plan──▶ CameraPathPlan (RAM memo, signature-hashed)
                  ──bake──▶ SetTimeSample ops on the target Camera prim
```

Bake emits through `ApplyUsdOp`, so it is journaled, undoable, persisted, and replicated
for free — the same funnel every other edit uses. Steal three hard-won lessons from
`compile_behavior_xml` verbatim:

- **Signature-hash the source** and skip the rebuild when unchanged. That system re-runs
  every frame the rover moves; rebuilding unconditionally reset every `WaitNode` timer so
  `dwell` could never elapse. A camera bake that re-fires per frame will thrash the journal.
- **Refuse to compile a dangling target** rather than baking `(0,0,0)` and flying to origin.
- **Clear the plan on stage reload** (`clear_animation_plans_on_stage_reload` is the model).

Bake is idempotent: re-baking replaces the target's timeSamples wholesale. Because
`RemoveTimeSample` inverts only to a coarse source snapshot (documented asymmetry in
`document.rs`), prefer one clean replace over incremental key surgery.

## 5. Live editing — reuse the waypoint machinery

The user-facing ask ("like the avatar, change trajectory in realtime, inspired by rover
waypoints"). The waypoint explorer surfaced a reusable kit; take it wholesale:

| Pattern | Source | Use |
|---|---|---|
| `CursorModeActive` + mirrored `*ToolActive` gate | `sync_waypoint_tool_active` (L93) | Stand down sibling click observers — every global `Pointer<Click>` observer sees the same click; `propagate(false)` stops bubbling, not siblings |
| `CancelIntent` → `#[Command] CancelCameraPathEdit` | `CancelWaypointEdit` (L129/L156) | Esc layers innermost-first; rebindable, scriptable |
| Armed placement: `Resource(Option<Pending>)` + observer `take()`s next click | `WaypointPlacement` (L415) | Move / insert-after |
| `pick_ground_world` (DEM + collider, nearer hit, `render_to_world`) | L216 | Ground reference for knot altitude |
| Real geometry, not gizmos/overlays | `sync_waypoint_path_mesh` (L1351) | Path ribbon; screen-space strokes have no depth and read as buggy |
| Stable string keys, not indices | `WaypointVisual::coord_key` | Deleting knot 1 must not respawn the rest |
| egui `set_cursor_icon`, not window `CursorIcon` | `handle_waypoint_placement_mode` (L112) | Avoids re-asserting every frame to beat bevy_egui |

**The primary gesture should be pose capture, not ground click.** Fly the free camera to a
framing you like, press a key, and that knot records position + orientation + focal length.
This is how DCC cinematic tools work, it matches "change the trajectory in realtime", and it
is the only gesture that captures *aim* — a ground click gives you a point on the floor, which
is not a shot. Keep ground-click as the secondary gesture for roughing out a ground track,
with knot altitude = ground + offset.

**Frame handling — know which of the two paths you are on.** This repo's most-repeated bug is
authoring a render-frame value as if it were grid-absolute (waypoints, connectivity occluders,
the wheel raycaster). But the two gestures need *different* treatment, and conflating them is
its own bug:

- **Pose capture (the camera itself)** → use `lunco_core::coords::world_pose(cam_entity, ..)`
  → `(DVec3, DQuat)`. It walks the grid hierarchy and returns **grid-absolute directly**. Do
  *not* route this through `render_to_world` — there is nothing to convert, and doing so would
  double-apply the origin offset. Note a camera's `GlobalTransform` **is** render-frame, so
  reading that instead is exactly the trap.
- **Ground pick (a raycast hit)** → the hit comes out in render space, so it *must* go through
  `render_to_world` (`checkpoint_click.rs:190`), as `pick_ground_world` does.

So: capture reads `world_pose`; ground-click converts. Same tool, two frames, one rule each.

## 6. Playback and activation

No new machinery. The bake makes the camera prim carry xform timeSamples → `prim_is_animated`
tags it `UsdAnimated` → `plan_usd_animation` memoises an `AnimationPlan` → `bind_animated_to_preview`
grows `Playback.start/end` from the clip span → `sample_usd_animation` drives the transform
against the `AnimationPreview` domain. The inspector's existing transport plays/pauses/scrubs it.

Activation, pick one:

- `SetActiveCamera { name: "CraterOrbit" }` — command, API, rhai `set_camera("CraterOrbit")`, or `KeyC` cycle.
- `token lunco:activeCamera.timeSamples` on a `def Scope "CameraTrack"` — cuts as data, scrubbing with the transport.

This is why the answer is **animation, not a behaviour tree**, and the codebase already agrees:
a BT is a *decision* structure re-evaluated against world state at tick rate — it re-plans, it
can fail, and doc "BT one-shot leaf" records that Sequence auto-resets children so "one-shot"
leaves re-fire at tick rate. A camera move is a *deterministic function of time* that must scrub
backward, which `Playback::replay`/`step_playhead` support and a BT fundamentally cannot.

## 7. Build order

Each phase is independently demoable.

**Phase 0 — hand-authored, zero code.** Write `moonbase_cinematic.usda` by hand with a
`def Camera "CraterOrbit"` carrying ~8 hand-typed `xformOp:translate`/`orient` keys on a circle,
plus a `CameraTrack`. Proves composition, transport, and activation end-to-end against moonbase
today, and tells us how bad linear interp actually looks — which calibrates how much the bake
is worth. Also forces the crater-centre decision (§2) immediately.

**Phase 1 — bake, headless.** `LunCoCameraPathAPI` schema + orbit generator + Catmull-Rom/ease
bake → `SetTimeSample` via `ApplyUsdOp`. Testable with no UI: author a path prim, run the bake,
assert timeSamples on the target. Registering the schema means `generatedSchema.usda` +
`plugInfo` Types, **not** `schema.usda` — that file is inert (memory: schema.usda is inert,
generated is real; there is no sync test).

**Phase 2 — live capture tool.** Pose-capture keybind, `CameraPathToolActive` gate,
`CancelCameraPathEdit`, knot visuals + path ribbon, right-click menu (move / insert-after /
delete / dwell / focal length). Re-bake on change, signature-gated.

**Phase 3 — polish.** Ease-curve UI, timeline lanes (doc 35 steps 2–7), per-knot interp.

## 8. Open questions

- **Crater centre** — hero crater (recommended) vs eyeballed point? Blocks Phase 0. §2.
- **Where does the scene live** — moonbase twin repo (recommended, it's a moonbase scene) or
  `usd/assets/scenes/sandbox/`? Cross-repo edits mean two commits.
- **Undo** — camera-path edits go through `ApplyUsdOp` so document undo works, but editor
  Ctrl+Z is wired to the ECS-only `UndoStack`. Same split that leaves waypoint edits
  un-undoable. Fix here or leave consistent-but-wrong?
- **`SetTimeSample` has no API/MCP/rhai surface.** Phase 1 is hard to test via the API skill
  without one. Worth adding a thin command?
- **Big-space at scale.** A 90 m orbit is well inside one cell, so this is fine — but knots are
  authored grid-absolute and a long traveling shot would need cell-aware handling.

## 8a. VERIFIED — Phase 0 run, and the bug it found

Ran `sandbox --scene /home/rod/Documents/lunco/moonbase/twin/moonbase_cinematic.usda --api 3001`.

**Works:**
- Composition. `twin://moonbase/moonbase_cinematic.usda` mounts doc-backed, sublayers the base,
  the whole moonbase (terrain, DEM, rovers, structures) spawns under `/MoonbaseScene`.
- Camera spawns: `[usd-bevy] /MoonbaseScene/CraterOrbit Camera → inactive SceneCamera (perspective)`.
- Activation: `[camera] viewport → 1198v0`. The `CameraTrack` cut works.
- **The Euler derivation in §4 is correct.** The t=0 screenshot frames the habitat and solar
  tower from a high three-quarter angle, exactly as designed. yaw = θ, pitch = −atan(H/R).
- Site elevation Y=1981 is correct — above terrain, not underground.

**Broken: the camera never moves.** Screenshots at t=0 / t=15 / t=30 are pixel-identical but
for the FPS counter.

**Root cause — `camera_mount` hijacks any animated top-level camera.** Not a scene bug; an
engine bug.

1. `resolve_camera_mounts` claims every `SceneCamera` whose `ChildOf` is **not a `Grid`**
   (`camera_mount.rs:48-60`). `CraterOrbit` is a child of the `/MoonbaseScene` root Xform,
   which is not a Grid — so it is rigged as a follower "mounted" on the static scene root:
   `[camera] 1198v0 mounted on 1188v0 → grid-direct follower`.
2. `MountedCamera.offset` snapshots the transform **once** at resolve (`offset: *tf`, line 84).
3. `sample_usd_animation` runs in **`Update`** (lib.rs:242). `follow_mounted_cameras` runs in
   **`PostUpdate`** before propagation (lib.rs:190-194). PostUpdate is after Update, so the
   follower **overwrites the sampler's write every single frame**, restoring the frozen
   snapshot. The scene root never moves ⇒ the camera is pinned forever.

Two systems own `Transform`, the later one wins unconditionally, and nothing warns.

**The module's own doc comment states the correct intent** — "leave grid-direct cameras
(top-level scene cameras, the avatar eye) untouched." The intent is right; the *predicate* is
wrong. `parent is not a Grid` is not a test for "mounted on a mover" — it misclassifies a
camera under a static scene-root Xform as a vehicle mount.

**This means `lander_cinematic.usda`'s flagship keyframed `OrbitView` dolly — doc 35's whole
example — is almost certainly frozen too.** Same structure: a `def Camera` under a scene-root
`over`. Worth confirming; if so, "camera animation works" has never actually been true.

### Fix options

- **A — fix the predicate (recommended).** Mount only when an ancestor between the camera and
  its grid is a genuine mover (rigid body / animated prim). A camera under a static root
  becomes grid-direct, `resolve` marks it done, and animation owns `Transform` uncontested.
  Smallest change that matches the module's stated intent.
- **B — yield to animation.** Add `Without<UsdAnimated>` to the resolver/follower. One line,
  but it makes "animated" and "mounted" mutually exclusive forever — an animated camera riding
  a rover becomes impossible.
- **C — compose instead of fight (the real DRY fix).** See §8b.

## 8a-bis. Fix verified, and the tools built on it

`camera_mount.rs` now refuses to rig an animated camera (`Without<UsdAnimated>` on both the
resolver and the follower). The `mounted on` log line is gone and the camera moves.

**Numeric proof** (probe cameras captured at four playhead times, positions from
`AddCameraHere`'s grid-absolute capture):

| t | x | y | z | radius from (18,−20) | bearing |
|---|---|---|---|---|---|
| 0 | 18.00 | 1981.00 | 70.00 | 90.00 | 0° |
| 15 | 108.00 | 1981.00 | −20.00 | 90.00 | 90° |
| 30 | 18.00 | 1981.00 | −110.00 | 90.00 | 180° |
| 45 | −72.00 | 1981.00 | −20.00 | 90.00 | 270° |

Exactly the authored circle — radius 90.00, height 1981.00, 90°/15 s. This one table validates
three things at once: the sampler drives the camera, `world_pose` capture really is
grid-absolute (a render-frame read would have put y near −19), and the trajectory overlay's
data source is sound.

Built (all reachable from rhai/API/MCP, not just buttons):

- **`AddCameraHere`** (`lunco-sandbox-edit/src/ui/cinematic.rs`) — capture the live view as a
  `def Camera` via `ApplyUsdOp` into `LayerId::root()`. Names itself `View_N`, skipping taken
  names (`AddPrim` rejects rather than merges).
- **`ControlAnimation.looping`** (`lunco-time/src/domain.rs`) — the field was honoured by
  `step_playhead` but unreachable; now a verb. Restart = `{playing, seek_secs: start}`.
- **🎬 Cinematic panel** — docked in Build mode: capture, transport (restart/play/loop/scrub/
  rate), path toggle.
- **`draw_camera_paths`** — trajectory overlay.

### Two lessons worth keeping

**The transport must be docked, not floated.** A first cut put it in an `egui::Area` pill to
survive View mode's empty dock. That works, but the dock is where it belongs; the pill was
solving a problem (View mode has no panels) that the user does not actually have while
authoring, since authoring happens in Build.

**A path overlay is unreadable from the camera flying it.** Drawn from `CraterOrbit`, the orbit
passes through the eye and projects as a line off both edges of frame — it looks broken and is
not. Read a path from a *different* camera. This is an argument for the §10 look-through
toggle: authoring wants two viewpoints (the shot, and the shot's shape from outside), and only
one of them is the camera itself.

## 8b. Why cameras have a "special path" — and what DRY actually means here

Worth being precise, because the answer is not "delete the special case".

**Cameras do NOT have a separate transform path.** Transform composition, `lunco:cameraLookAt`
aim, and `UsdAnimated` tagging all happen in the *generic* `sync_usd_visuals` path — the
camera-specific part is a three-line `if prim_type == Some("Camera")` branch (lib.rs:1104) for
the lookAt convenience, sitting right beside the generic tagging at lib.rs:1154. That tagging
fires correctly for our camera. Nothing is duplicated there.

Two things genuinely are camera-only:

1. **`instantiate_camera_prim`** — attaches `Camera` / `Projection` / `Exposure`. This is
   legitimate specialisation, exactly like mesh instantiation being mesh-only. Not a DRY
   violation: it is the one place that knows what a camera *is*.
2. **`camera_mount`** — the actual problem, but it exists for a real constraint: big_space
   requires `FloatingOrigin` on a grid-direct entity, so a camera literally parented under a
   moving rover can never host the active-view origin at full precision. Deleting it
   reintroduces the nested-camera precision caveat it was built to remove.

**So the DRY violation isn't the special path — it's that one field (`Transform`) has two
owners and two meanings.** Before `resolve_camera_mounts`, a camera's `Transform` is
*mount-local*. After it reparents the camera to the grid, the same field means *grid-local
world pose*. The sampler keeps writing it with the first meaning; the follower overwrites it
with the second. One field, two semantics, no arbitration — that is the actual defect, and it
is why the symptom is silent.

The unification worth making is conceptual, not textual. Mount and animation answer *different
questions*:

- animation → "what is my **local** transform at time t?" (plain USD semantics)
- mount → "what **frame** is that local transform expressed in?"

They compose; they do not compete:

```
world(t) = mount_world × local(t)
```

Under a static scene root, `mount_world` = identity and this degenerates to pure animation —
no branch, no special case. On a moving rover with an animated offset, both work at once: a
keyframed camera move *riding* a vehicle, which is impossible today and is exactly the shot a
cinematic toolkit wants. The current code is a frozen special case of this formula: it
snapshots `local` once at resolve instead of reading it live.

Implementing it means giving the animated local its own home (the sampler's output) and letting
the follower compose into `Transform` + `CellCoord` as the single writer. **One writer per
field is the DRY invariant that matters here** — not collapsing camera code into mesh code.

Option A (fix the predicate) is the cheap correct step; option C (compose) is where it should
land, and A is a strict subset of C's behaviour, so A is not throwaway work.

## 8c. VERIFIED: which "USD standard spline" actually applies

Two different things wear the name. Only one can carry a camera path, and it is not the one
people mean by "USD splines".

### `Ts` splines (attribute-level) — CANNOT do this

OpenUSD 25.x added splines on attributes: `double radius.spline = { bezier, 1: 10; ... }`.
They are **scalar-only**. From the OpenUSD glossary (fetched, not remembered):

> "A spline provides a curve that defines a **scalar value** that varies over time… a spline
> also contains the type of the value (**double, float, or half**)"

So `double3 xformOp:translate` **cannot** be a spline — the standard forbids it, and there is no
per-component xformOp to spline instead. `Ts` is right for scalar channels (`focalLength`,
`focusDistance`); it can never express a position path.

Independently: our Rust `openusd` fork (0.5.0, tracking mxpv/openusd) has **zero** spline
support — `grep -ri spline --include=*.rs` returns 0 across the whole fork. So even the scalar
case would need implementing upstream first.

### `UsdGeomBasisCurves` — the USD-standard spline for a path THROUGH SPACE

A camera path is a curve in space, and USD already has that primitive, with exactly the bases
wanted:

```usda
def BasisCurves "CraterPath" {
    uniform token type = "cubic"
    uniform token basis = "bezier"        # or "catmullRom" — passes THROUGH its points
    uniform token wrap = "periodic"       # a closed orbit, for free
    int[] curveVertexCounts = [12]
    point3f[] points = [(18,1981,70), ...]   # the editable control points
}
```

`wrap = "periodic"` closes the loop — exactly what an orbit shot wants, with no seam special
case. And because it is real geometry, the path is **its own trajectory visualisation**: a prim
in the scene and in usdview, not a debug gizmo that exists only in our viewport.

Neither the fork nor lunco has a typed `BasisCurves` wrapper (`grep` = 0 in both), but that does
**not** block it: the fork parses arbitrary USD and lunco reads attributes generically
(`read_vec3_f64` and friends), so a spec-conformant `def BasisCurves` can be authored and read
today. What we implement is the *evaluator*, not the format.

**Decision.** Path = `UsdGeomBasisCurves` (standard, portable, self-visualising). Smoothness =
the standard basis, evaluated by a live driver (per the chosen live-driver approach). Scalar
channels stay open to `Ts` splines if the fork ever gains them.

**The remaining trade-off, honestly:** `points` is an *array*, not prims — so the existing
selection gizmo cannot drag a control point the way it drags a knot prim. Editing needs a
per-point handle gizmo writing back into the array via `ApplyUsdOp`. That is the cost of using
the standard representation instead of inventing knot prims; the payoff is a path that is real
USD and renders itself.

## 8d. Fixed-step position, smoothed render — the established pattern

Requested, and the repo already does exactly this for the follow camera, so the path driver
copies it rather than inventing a cadence:

- `spring_arm_system` runs in **`FixedPostUpdate`** ("so its slerp/lerp uses the fixed cadence",
  `lunco-avatar/src/lib.rs:1169`), reading the body's fixed-step time domain.
- `spring_arm_paused_system` is the **render-rate twin** — because "`FixedPostUpdate` stops when
  the sim pauses" (:1211), which is the same freeze that bit the animation clock (§8a-bis).
- The Transform is **eased between** fixed writes for render (:2840).

So: evaluate the curve at the path clock's time in `FixedPostUpdate`; ease the camera Transform
toward it at render rate. Note the paused-twin problem disappears if the path clock hangs on a
wall-rooted parent — but the *cadence* still wants to be fixed, because cadence ≠ clock.

## 8e. OUTSTANDING — next session, in dependency order

Everything below is unbuilt. Ordered so each unblocks the next.

1. **Verify the gizmo fix** (written, compiles, NOT run). `gizmo::confine_scene_cameras_to_viewport`
   tags window scene cameras with `WorkbenchViewportCamera` before `sync_gizmo_camera` picks the
   gizmo camera. Symptom it targets: selection AABB visible, transform gizmo absent — because Bevy
   `Gizmos` draw world-space lines through ANY camera while transform-gizmo resolves the pointer
   against the tagged camera's viewport, and a cut to a USD `def Camera` bound it to an unclamped
   full-window viewport. **Cannot be verified over the API**: `ListEntities` returns 0 for this scene
   (empty `ApiEntityRegistry`) so `SelectEntity` has no id, and no command switches perspective. It
   needs a human click in Build mode. Fix this first — point dragging is the same gizmo.
2. **Drag control points.** `points` is an ARRAY, not prims, so the selection gizmo cannot touch it —
   the cost of the standard representation (§8c). Shape: handle entities projected from the array →
   existing `transform_gizmo_bevy` → write back via `ApplyUsdOp` (`SetAttribute` on `points`) so edits
   stay undoable / journaled / saved. Do NOT mutate the array in ECS — that escapes all four.
3. **Total animation length.** `lunco:path:duration` already exists and drives `Playback.end`; this is
   a panel field + `ApplyUsdOp` write-back. Cheapest of the four.
4. **Speed between points.** NOT a slider — this is the arc-length gap (§9.7). `u` is uniform in the
   CURVE PARAMETER, so point spacing silently *is* speed and the camera surges wherever points cluster.
   Needs arc-length resampling, then per-segment timing on top (an ease/dwell track shaped like the aim
   track — same held-key pattern, so reuse it).
5. **Terrain at full quality from the cinematic camera, no realtime LOD reload.** Not located yet:
   `grep -rn 'lod_bias|max_lod|split_distance' crates/lunco-usd-terrain/src/` returns nothing, so start
   by finding what actually selects LOD and which camera it keys off (suspect: the ACTIVE camera, which
   is now a flying path camera ⇒ constant re-streaming, i.e. the popping). Wanted: pin max detail and
   freeze re-selection while a shot plays. See [[project_terrain_streaming_architecture]].

Housekeeping: test-debris cameras (`View_1`, `View_2`, `S0`–`S19`, `Probe_t*`) are recorded in
`~/Documents/lunco/moonbase/twin/history/journal.json` and will replay on load. The `.usda` on disk is
clean.

## 9. Missing features, ranked

Phase 0 needs **nothing** — it is authored USD against shipped machinery. This is what the
*real* ask (fly, drop knots, activate) needs. Ordered by "what blocks the next demo".

### Blocking

1. **`focalLength` is not an animatable channel.** The sampler drives a *fixed* channel set:
   xform, visibility, `displayColor`/`displayOpacity`, `diffuseColor`/`opacity`, `activeCamera`.
   Arbitrary attributes are explicitly not projected. So `float focalLength.timeSamples` would
   author fine and *silently do nothing* — no zoom, no dolly-zoom, no focal-length ramp. For a
   cinematic toolkit that is a real hole. Fix: add `focalLength` (and `clippingRange`) to the
   camera's animated channels, writing `Projection` rather than `Transform`. Small, contained.
   *Verify first — inferred from the channel table, not from a run.*
2. **No keyframe-authoring surface outside raw `UsdOp`.** `SetTimeSample` has no command, API,
   MCP, or rhai entry point. This blocks the capture tool, blocks scripted shots, and blocks
   *testing any of this headlessly* via the API skill. Smallest item here and it unblocks the
   most — do it first.
3. **No knot schema + bake.** Without it every path is a linear polygon (§4).
4. **No pose-capture gesture.** §5.

### Important

5. **No timeline UI** beyond the inspector's slider — you can scrub, but you cannot *see* keys,
   let alone drag them. Doc 35 steps 2–7.
6. **Undo is split.** Editor Ctrl+Z drives the ECS-only `UndoStack`; typed undo lives on
   `DocumentHost`. Waypoint edits are already un-undoable because of this. An editor whose
   Ctrl+Z does nothing is not an editor — this is a correctness bug for the tool, not polish.
7. **No arc-length reparameterisation.** Catmull-Rom evaluated on the raw parameter moves
   *faster through sparse knots and slower through dense ones*. So knot spacing silently
   becomes speed control, and the camera surges whenever you add a knot to fix framing. Bake
   must resample by arc length, then apply ease on top. Cheap at bake time, invisible if missed
   until the shot looks drunk.
8. **No ease curves.** `lunco:path:easing` is in the §4 sketch; nothing implements it.

### Nice to have

9. `focusDistance` is read but unused — no depth of field.
10. Ortho `aperture` → `ScalingMode` is a known partial.
11. No shutter/motion-blur channel.

## 10. Editing UX — what "convenient" means here

Prior art worth copying: Blender (align-camera-to-view, follow-path + track-to constraints),
Unreal Sequencer (spatial path and temporal track as *separate* editors), Maya (motion path +
aim constraint). The consistent lesson across all three:

> **Separate WHERE from WHEN from WHAT-IT-LOOKS-AT.** Three independent edits, three
> independent controls. Tools that fuse them (one blob of keys you nudge in 3D) are the ones
> people bounce off.

Mapped onto this repo:

| Concern | Control | Backing |
|---|---|---|
| WHERE | knot `Xform` prims, dragged in the viewport | existing gizmo (`SelRoot` frame) |
| WHEN | timeline lane; per-knot `dwell` / path `duration` | `AnimationPreview` + transport |
| WHAT-IT-LOOKS-AT | a separate `AimTarget` Xform prim the path aims at | `lunco:knot:lookAt` rel |

The **aim target is the highest convenience-per-line item on this list.** One draggable prim
that every knot aims at means re-framing the whole shot is *one* drag, instead of re-orienting
twelve knots by hand. It is the single reason Blender's Track-To constraint is how everyone
actually shoots. It costs a rel + a bake-time branch that already exists in the §4 priority order.

### The core loop to optimise for

Everything else is secondary to making this fast:

```
look through the camera  →  scrub to a time  →  fly/nudge until the framing is right  →  key it
```

Which implies, in priority order:

1. **Look-through-camera toggle.** Non-negotiable. You cannot frame a shot you cannot see;
   `SetActiveCamera` already does the work, this just needs a binding and a way back.
2. **Pose capture** (fly free-cam, press a key → knot with position + orientation + focal
   length). Blender's Ctrl+Alt+Numpad0. The lowest-friction authoring gesture that exists.
3. **Draggable knot gizmos.** Reuse the selection gizmo. Knots are real prims precisely so
   they can be selected and dragged like anything else — this is the payoff for rejecting the
   waypoint XML-blob storage.
4. **Path drawn as real geometry** (ribbon, signature-hashed rebuild), plus a small frustum
   ghost at each knot so you can read the shot without scrubbing. The waypoint code already
   learned that screen-space overlay strokes have no depth and read as buggy.
5. **Auto-key on nudge while scrubbed.** Move the camera at t=12 → the key at t=12 updates.
   This is what makes it feel live rather than like form-filling. Needs undo (§9.6) to be safe.
6. **Timeline with draggable keys.** Retiming without re-flying.

### Explicitly not

- **No drag-in-empty-space for position.** A knot dragged with no depth reference lands
  somewhere arbitrary. Drag on the ground plane (`pick_ground_world`) + a separate altitude
  handle, or capture from a real camera pose.
- **No fusing the aim into the knot rotation as the primary control.** Bake it, yes; but if
  hand-rotating each knot is the only way to aim, the tool is already lost.

## 11. Verify before building

Claims below are from code reading, not execution:

- `catmull_rom_path`'s exact signature/module path and whether it's exported outside `lunco-autopilot`.
- That `xformOp:orient` slerp works end-to-end through `local_rotation_at` for a `def Camera`
  (`lander_cinematic.usda` uses `rotateXYZ`, so the orient path is unproven *for cameras*).
- Whether a `def Camera` under `over "MoonbaseScene"` spawns inactive and is reachable by leaf
  name from `SetActiveCamera` (it matches full path *or* leaf).
- Moonbase's DEM site elevation ≈1946 m — every authored Y must account for it, or the camera
  spawns underground.

# 51 — Cinematic Camera Paths

> Status: Active · Audience: contributors building camera paths, cinematics, and recording
>
> Extends [`35-animate-perspective`](35-animate-perspective.md), which owns the
> future multi-lane timeline editor. This document owns the runtime camera-path
> contract.

## Contract

A camera path is a `UsdGeomBasisCurves` prim with a relationship to the camera it
drives. The curve is the authored trajectory; the camera is the rendered result.
Spatial paths do not use `xformOp:translate.timeSamples`: USD attribute splines
(`Ts`) are scalar-only, while `BasisCurves` is the standard USD representation for
a curve through space and can be inspected by other USD tools.

```usda
def BasisCurves "ShotPath"
{
    uniform token type  = "cubic"
    uniform token basis = "catmullRom"    # "bezier" or "catmullRom"
    uniform token wrap  = "nonperiodic"   # or "periodic"
    int[] curveVertexCounts = [30]
    point3f[] points = [ ... ]

    rel    lunco:path:camera = </Scene/ShotCam>
    rel    lunco:path:lookAt = </Scene/Lander>  # optional whole-path target
    double lunco:path:duration = 58
    token  lunco:path:clock = "sim"            # or "real"
}
```

The runtime accepts `point3f[]` and `point3d[]`, validates curve type, basis,
wrap, point count, duration, and finite values, and refuses malformed paths. A
non-periodic cubic Catmull-Rom path needs at least four control points; a periodic
one needs at least three. The evaluator supports linear, Bezier, and Catmull-Rom
bases according to the `BasisCurves` shape.

## Runtime ownership

`lunco-usd-bevy/src/camera_path.rs` resolves a valid path into a `CameraPath` and
creates two domains:

1. A gate domain starts held at creation. It is an engine-readiness hold, not a
   user pause.
2. A path playback domain carries the playhead, range, rate, and loop state.

`CameraPathTransport` is the explicit typed command for `Play`, `Pause`, and
`Rewind`. It is available through the HTTP/API/MCP command surface and Rhai. `Play`
releases the gate; `Pause` changes user playback state; `Rewind` seeks to the
authored range start. Do not use `Playback.mode` as an implicit startup gate.

The path clock is selected by `lunco:path:clock`:

- `sim` (default) follows the simulation clock and pauses with it;
- `real` follows the interaction/wall clock and can play while simulation is
  paused.

The path is an analytic function of its resolved time, so it is sampled once per
render frame. It does not integrate state or interpolate between fixed-step
samples. This keeps a captured frame a pure function of the path clock and avoids
mixing a fixed-step accumulator with a render-frame clock.

The driver writes a grid-absolute target and keeps the camera grid-direct. The
camera receives `CameraPathDriven` and `CinematicCameraLock`; the path removes the
mounted follower and the avatar camera systems honour the lock. This gives one
writer ownership of the camera pose and preserves big-space precision. A path
must not write a parent-local AU-scale translation or compete with the avatar
camera stack.

## Aim

Position and aim are separate concerns. The optional path-level `lunco:path:lookAt`
relationship is the default target, but a shot can author an aim track:

```usda
double[] lunco:path:aim:times = [0, 20, 40]
token[]  lunco:path:aim:modes = ["target", "target", "manual"]
rel      lunco:path:aim:targets = [</Scene/Habitat>, </Scene/Lander>]
double   lunco:path:aim:blendDuration = 0.5
```

Each mode is held from its key until the next key:

- `target` resolves the corresponding relationship each frame;
- `tangent` aims along the curve direction;
- `manual` writes position only and leaves rotation to the operator.

An optional blend duration blends into the next aim key. A missing aim track uses
the path-level target when present, otherwise the tangent. A moving target remains
live because the relationship is resolved during driving, not baked into each
control point.

## Authoring surface

The current editor supports the low-friction capture workflow:

1. Fly the active window camera to the desired framing.
2. Invoke `AddCameraHere` from the Cinematic panel, API, MCP, or Rhai.
3. The command reads `world_pose`, not the camera's floating-origin
   `GlobalTransform`.
4. It authors a `def Camera` and its transform through `ApplyUsdOp` in the root
   layer, so the edit is saved and journaled rather than being an ECS-only spawn.

The docked Cinematic panel also exposes path visibility and transport controls.
The trajectory overlay samples the same evaluator as the runtime driver, draws
control points and aim arrows, and shows the current playhead. It hides a path
when the user is looking through that path's own camera because the trajectory is
not legible from the eye it passes through.

Keep authoring controls separated:

| Concern | Current or intended control |
|---|---|
| Where | `BasisCurves.points`, edited through USD operations |
| When | the path's `duration`, playback domain, and future timeline lane |
| What it looks at | `lookAt` or the aim track |

The path is an animation, not a behaviour tree. It is deterministic in time and
must scrub backward; behaviour trees are for decisions, guards, retries, and
world-state reactions.

## Remaining work

These are current gaps, not a record of completed investigations:

1. Add point-handle editing. `points` is an array, so the existing prim selection
   gizmo cannot edit individual control points. Handles must write the complete
   array through `ApplyUsdOp`; ECS-only mutation is not an authoring path.
2. Add arc-length reparameterisation and explicit timing when a shot needs
   constant speed through unevenly spaced points. The current parameter is
   uniform in curve space, so point spacing affects speed.
3. Add camera-specific animated channels such as focal length when a shot needs a
   zoom or dolly-zoom. The generic animation sampler's supported channel set is
   not evidence that every camera attribute is projected.
4. Add the multi-lane timeline and retiming UI under
   [`35-animate-perspective`](35-animate-perspective.md). Keep spatial curve data
   in `BasisCurves`; use the timeline for temporal edits and camera cuts.
5. Add explicit easing only where the authored contract needs it. Do not add a
   second path representation or a compatibility alias for the existing curve.

Every new edit must remain a USD operation, survive save/reload, and be checked by
the production `target/debug/luncosim` binary with an explicit API port. A parser
or evaluator unit test alone does not prove camera ownership, grid-frame handling,
transport, or visible motion.

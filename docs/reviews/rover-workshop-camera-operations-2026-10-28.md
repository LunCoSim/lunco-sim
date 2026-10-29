# Rover workshop camera operations

Scope: Summer Space School rovers and their test fixtures. Camera power is an
explicit on/off command, independent of viewport selection. Depth and noise
are presentation effects, not additional physical sensors or CPU/storage
loads. Existing capture traffic continues through the recorder and radio.

Owners and verification:

- `lunco-render-recovery` retains GPU diagnostics and publishes runtime
  failures to Recent Events. There is no floating recovery banner. The
  resource/lifecycle seam verifies warning, terminal error and scene retirement.
- `CameraPayload`, `SchoolComputer` and the EPS own camera power admission;
  the existing powered mast owns slew and actuator electrical demand. Twin
  UI switches science and hazard groups. Camera off draws zero under the
  authored study coefficients. Production Rhai checks power, data, shedding
  and recovery against the solved electrical network.
- USD camera projection and the viewport reconciler own explicit main-view
  selection. Sensor cameras use the same selection path. Camera navigation
  groups by mounted vehicle and supports filtering; exact identities remain
  the action targets, including when several rovers have identically named eyes.
- `lunco-capture` owns independent view configuration. `lunco-render` carries
  validated image-effect intent; `lunco-render-bevy` binds the native opaque
  depth and image post-process. Standard-library WESL owns depth display and
  procedural noise. Live typed commands and shader reload supply the editing
  workflow. Pixel evidence must show distinct depth and controllable noise.
- Twin USD owns rover appearance and visible route/waypoint facts. Shared
  rover references supply the same appearance to operational and test scenes.
  Physics, mass and collision contracts remain at their existing owners.
- The standard HUI vessel HUD prioritizes large speed, attitude, heading and
  link status. Detailed telemetry stays in the telemetry browser or workshop
  card instead of a dense default paragraph.
- Twin Rhai owns photography challenges: distinct viewpoints, target framing,
  powered capture, retained evidence and a finite completion verdict. Repeated
  captures from one pose must not complete a multi-angle task.

Lifecycle: view configuration, camera selection, UI drafts and challenge
evidence are scene scoped. Renderer effects do not write simulation state.
Invalid sources, ranges and values reject at their typed owners. GPU faults
keep their bounded presentation-stop semantics.

Photo visibility uses the existing Raycast owner with an explicit boolean
`exclude_sensors` filter. Avian sensor colliders remain observable by default;
photography ignores those invisible trigger volumes while solid obstructions
remain authoritative. The authored runtime-contract scene covers both rays and
malformed filter rejection. The camera capture owner rejects rendering failures
through the shared structured diagnostics rather than saving invalid evidence.

Surface ray mode delegates enclosing-solid semantics to Avian; the existing
TerrainRaycast provider separately checks measured relief. Photography composes
those owners instead of treating the coarse enclosing globe as filled terrain.
No new terrain marcher or camera-specific physics path is introduced.

Verification uses the production `luncosim` binary:

- `scripts/api/test_camera_views.py --port 4232`: 29 authored assertions plus
  pixel checks for near-red/far-green depth, fixed noise, live reset and a
  world-fixed Sun while the selected main camera tilts; owned Exit verified.
- `authored_runtime_contracts`: 20 assertions, including filtered trigger rays,
  enclosing-surface rays and invalid-filter rejection.
- `lunco-viz` decimation: four pure regressions, including stable completed
  buckets during append and scroll.
- Twin gates: workshop 33 assertions, mission systems 17, antenna mount 17 and
  missing mount negative 1; Lunar Shift 23; both Apollo briefing/route verdicts;
  five lesson startup events and the mission presentation audit.
- A windowed rehearsal drove between three survey poses and saved context,
  detail and third-side images at 18.6, 10.8 and 12.4 m. The completion event
  required three retained images, cameras off and a drained recorder. Solved
  science power was 8 W on and 0 W off; mast slew added electrical demand.

Depth is opaque rendered axial depth. Noise is illustrative. The solar and
load coefficients are authored teaching assumptions, not calibrated hardware
claims. Simultaneous application sessions prevent an isolated FPS claim here.

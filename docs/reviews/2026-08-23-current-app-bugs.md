# Current application bug audit — 2026-08-23

This is the source-backed root-cause map for the fourteen reported application
bugs, updated with the implementation and verification status of each group.
The review covers the engine checkout and the local Summer Space School Twin.

## Scope and evidence standard

The target is production behaviour: authored USD composition, projection,
runtime consumers, and the visible application path. A parser result, a green
unit test, or a binary build alone is not closure. Each implementation group
must add or reuse an integration-style test and then be checked through the
production `target/debug/luncosim` binary with an explicit API port where the
behaviour is interactive.

The Summer Space School Twin retains unrelated user changes in its working tree
(`scripts/test_mission_contracts.sh`, `sim/rovers/lunokhod2.usda`,
`sim/scenarios/README.md`, and `sim/scenarios/tests/lunokhod2_physics.rhai`).
Those changes are preserved.

## Findings

### APP-01 / APP-11 — solved battery flow is not the HUD contract

The engine HUD producer (`lunco-luncosim/src/engine_exposure.rs`) finds state of
charge, but its power lookup is limited to `battery_net_power` / `net_power_w`.
The Summer School `Electrical` boundary previously exposed battery capacity by
connecting `outputs:battery_capacity_ah` to `Battery.inputs:capacity`; that is a
nameplate parameter, not solved charge/discharge flow. The dirty Twin change
already moves that boundary toward the correct producer outputs, but the engine
and HUD still need one canonical display contract for charge, discharge, and
load power.

**Migration:** keep the existing USD/Modelica public-output projection as the
authoritative exposure path. The HUD consumes authored telemetry channels from
the shared `SignalRegistry`, whose ownership and exposure metadata come from
the authored USD topology and `ModelicaSignalLayout`; it does not resolve
electrical, hydraulic, thermal, or other output names in Rust. The existing
`Parameter.target` indirection is now authorable through the standard
`LunCoTelemetryAPI` target relationship, so one assembly can publish multiple
operator channels without a second registry. Battery SOC is an authored
percent output, and solved net/charge/discharge values are authored watt
channels. Publish one generic, authored-signal summary line and remove the
capacity-as-power interpretation and the split value/detail presentation. A
missing public signal remains unavailable rather than being inferred from an
unrelated value.

### APP-02 — compositor ownership is only partially ordered

`EguiAboveBevyUi` correctly places egui chrome above runtime-authored HUI, but
individual modal/overlay systems still use independently chosen egui orders and
are not all part of one application overlay schedule. Help, update, tutorial,
modal-host, networking, celestial, and waypoint surfaces can therefore be
painted in a surprising order even though each one claims to be foreground.

**Migration:** define one public application overlay render set and order all
modal/interactive overlays through it. Keep world labels and route annotations
below workbench chrome, and keep blocking dialogs/help/tutorial surfaces above
all ordinary UI. Preserve egui-over-Bevy-HUI as the compositor boundary.

### APP-03 — Help reports a local stamp, not a public GitHub build

`BuildIdentity` currently contains only version and a short local git stamp.
The Help menu renders that string, while the public source/release repository
and exact source-build link are absent. The updater repository is intentionally
machine-only and must not be used as the human build link.

**Migration:** stamp the canonical public repository and source revision into
the host identity, prefer the CI revision when present, and render a clickable
exact GitHub commit/release link in Help. A dirty or non-Git local build must be
labelled honestly; it must not masquerade as a published build.

### APP-04 / APP-05 — Unicode glyphs were reintroduced and native icon output
is incomplete

The titlebar already uses vector-painted controls because missing glyphs become
tofu. Other current menus, help, tutorial, and Modelica controls still use
emoji/Unicode glyphs as visual icons. That is the regression after the earlier
tofu cleanup: the fallback font is not the root fix. Separately,
`lunco-luncosim/build.rs` rasterizes an SVG for `winit::Window::set_window_icon`,
but no native executable resource is embedded; packaging the generated desktop
assets cannot give a Windows PE executable its file icon.

**Migration:** centralize the small UI icon vocabulary as vector-painted egui
icons and replace icon-as-text usages, retaining actual authored user text only
where it is semantic content. Embed the canonical Windows icon in the PE
resource at build time and keep Linux/macOS launcher/package metadata sourced
from the same canonical artwork. Add a build/package assertion for the native
artifact, not just a runtime-window assertion.

### APP-06 — Lunokhod 2 terrain path needs runtime lifecycle proof

The `lunokhod2` terrain variant composes a real cached DEM directory and a valid
2 km / 512-sample request. The production binary now loads the variant, answers
finite `TerrainHeight` queries, and reports the composed terrain attributes
including the DEM cache, 512 target resolution, and collider ring. The existing
generation ownership, cancellation, and watchdog path therefore remains the
authoritative lifecycle mechanism; no timeout terrain fallback was added.

**Status:** verified through the production headless scene/API path. A rendered
visual capture is still an open acceptance item.

### APP-07 — autopilot start can outrun terrain placement

The scene has a terrain readiness gate and a one-time body placement pass, but
autopilot engagement is independently allowed while the initial physical pose is
still being admitted. Starting movement on the first available command can apply
velocity before the body has received its authoritative terrain-fit transaction.

**Implementation:** `drive_autopilots` now observes the generic
`PhysicsStatePending` admission marker and emits no control while authored pose,
joints, and initial velocity are being admitted. The marker is removed by the
physics admission owner, so the next fixed tick is the first control write.
The focused authority test covers both sides of that boundary. No delay or
rover/terrain special case was added.

### APP-08 — Summer School’s route authoring contract is inconsistent

The production route renderer now deliberately draws green waypoint-to-waypoint
connections plus a blue rover-to-next-waypoint leg. The Summer School
`traverse.usda` scene explicitly authors no `Route` or `Mission`, while
`ss2_follow_route.rhai` describes and queries `/Traverse/Route/W*` as if those
prim paths existed, then spawns unrelated runtime beads. With no vessel route,
the renderer has no targets and therefore correctly draws no connection or
rover leg.

**Implementation:** Apollo 15 and Chang'e-4 now author `Route` prims and their
`Mission` BTXML source in each teaching terrain variant. Both mission trees
begin with W0, making the first green segment part of the authored target list.
The tutorial scripts no longer spawn runtime bead entities; the renderer reads
the same composed USD route used by the mission. The target resolver uses the
shared component path matcher and no longer contains a Route-name instance
escape. Production reload composed `/Traverse/Route/W0` and
`/Traverse/Rover/Mission` successfully. The route-editor unit test also verifies
that a first blue leg waits for the live rover pose instead of synthesising a
fake segment.

### APP-09 — antenna ownership is duplicated across composed assets

The rocker-bogie base already owns the single mounted antenna and scales its
dish. The Summer School scene comments document the previous duplicate antenna
problem and now overlays radio behaviour onto that antenna. The symptom after a
tutorial scene is therefore a lifecycle/composition regression to verify: stale
scene descendants or a second reference can leave the old unscaled visual alive
after the new scene is mounted.

**Status:** verified through the production tutorial-to-Lunokhod2 transition:
the composed scene contains exactly one `/Traverse/Rover/Antenna`, with the
canonical mast cylinder dimensions and grey authored display color. Existing
generic scene teardown and composition ownership remain unchanged; no duplicate
or alias was added.

### APP-10 — autopilot state is encoded as danger colour with a static label

The HUD producer changes `autopilot_color` to the error colour when autopilot is
engaged, while the HUI label remains the static `AUTOPILOT`. This makes a normal
active state look like a fault and leaves the user without an explicit active
state.

**Implementation:** the HUD now publishes an explicit `AUTOPILOT ON`/`AUTOPILOT`
label and uses the normal active/accent token while engaged. Red is reserved for
fault/refusal presentation. This is driven by the same authored telemetry/HUD
projection as the other vehicle status fields.

### APP-12 — a presentation contract error is treated as tutorial termination

`TutorialTargetUnavailable` is raised when an authored help anchor is absent.
`lunco-tutorial` converts it to `TUTORIAL_FAILED` and immediately triggers
`SkipTutorial`, which clears the host, overlays, and owned scene. A missing panel
is recoverable presentation state, not evidence that the lesson simulation is
invalid.

**Implementation:** the lesson host and scene remain alive. The workbench now
shows a topmost recovery surface with Continue/Retry/Stop actions, while the
tutorial owner clears only the invalid target. Continue reuses the existing
typed `cmd:TutorialNext` event for an active tour; Stop remains the explicit
lifecycle command. The regression
`missing_anchor_keeps_lesson_running_and_advances_on_continue` passes, and the
change is committed as `a863b48b2`.

### APP-13 — articulated rover needs production physical/visual acceptance

The six-wheel assets contain multiple representations: the authored
`six_wheel_rover` skid layout, independent-drive layout, and the rocker-bogie
profiles used by the tutorial. The engine has topology-driven wheel wiring and
focused motion tests, but a source review cannot establish that the current
visual arm transforms, joint admission, wheel contact, and turning all agree in
the interactive tutorial path.

**Implementation:** the canonical articulated rover already has authored rigid
rocker/bogie bodies, hinges, arm links, wheel attachments, and motor-to-wheel
relationships. The actual missing control topology was the front-wheel
`inputs:steer.connect` on both rocker wheels; those connections are now authored
to the rover's steering output. Rust remains topology-driven and unchanged.
Production headless verdicts now pass for both `SIX WHEEL` (18.24 m straight
travel, 47.83° yaw under steer) and `ROCKER BOGIE DRIVE` (39.04 m net travel,
31.44° steer yaw, 5 checks). A rendered visual capture is still open because
this acceptance used the supported no-UI production binary; no render-only
offset or physics workaround was added.

### APP-14 — tutorial “variants” mix profile assets with inspector variant sets

The B4 tutorial calls three profile files (`rover_easy`, `rover_medium`,
`rover_awful`) variants, while the Inspector's variant picker operates on
variant sets on the selected composed prim. Those are related USD mechanisms but
not the same UI contract. The profile assets rely on nested overrides through a
reference to `rocker_bogie`; if catalog publication, default prim mounting, or
nested override composition fails, B4 can show three names without three
composed vehicles.

**Implementation:** B4 now uses one authored USD payload that references the
three profile assets before the tutorial script starts. The script no longer
races asynchronous catalog publication with `SpawnEntity`; the existing generic
Inspector remains responsible for authored `doc` and parameter values, while
the profile files remain ordinary USD spawnable assets rather than fake child
variant sets. Production `ListEntities` composed
`/RoverVariants/ExplorerLT`, `/RoverVariants/Rover`, and
`/RoverVariants/Hauler`. `QueryUsdPrim` confirmed distinct authored livery
colors (green/amber/red) and motor stall torque (0.9/0.9/0.4) on the composed
profiles.

## Implementation groups and acceptance

1. **Electrical truth and vehicle HUD** — APP-01, APP-10, APP-11. Acceptance:
   solved battery discharge changes while driving, SOC changes, solar/charge
   flow is distinct, and the one-line HUD identifies active autopilot.
2. **UI compositor, build identity, vector icons, and executable branding** —
   APP-02 through APP-05. Acceptance: blocking overlays cover HUI and ordinary
   menus, Help links the exact GitHub build, no icon-as-text controls render tofu,
   and the native artifact carries the application icon.
3. **Terrain, placement, route, and antenna lifecycle** — APP-06 through
   APP-09. Acceptance: Lunokhod 2 loads and switches without a stuck request,
   autopilot cannot move before terrain-fit admission, the first route leg is
   visible and authored, and one antenna survives scene transitions at the
   authored scale.
4. **Tutorial recovery and rover/profile variants** — APP-12 through APP-14.
   Acceptance: missing anchors leave the lesson usable, the six-wheel rover
   drives and turns with attached arms, and B4 produces three visibly and
   physically distinct composed profiles.

Groups 1 and 2 are implemented and committed. Group 3 is implemented in the
engine checkout and the Twin, with focused and production headless verification;
it is committed before group 4 begins. Group 4's tutorial-recovery portion is
committed as `a863b48b2`; the authored steering/profile portion has passed its
focused and production headless checks and is committed as `91f36cd06`.
The remaining open item is rendered visual acceptance of the articulated rover;
existing unrelated work remains untouched.

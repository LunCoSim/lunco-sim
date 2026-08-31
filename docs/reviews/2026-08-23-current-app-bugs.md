# Current application bug audit — 2026-08-24

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

The root cause had two independent parts. The HUD was selecting a domain-
specific registry entry, and the domain projector dropped a causal connection
when its source was an external boundary output (for example
`/Rover.outputs:drive_left`) rather than another Modelica component. That let
the mobility path move while the generated electrical motor demand stayed at
zero.

The engine HUD producer (`lunco-luncosim/src/engine_exposure.rs`) therefore no
longer performs a domain-specific power lookup.

**Implementation:** the reusable Battery component now authors SOC,
net-charge, and discharge telemetry declarations once; rover wrappers no
longer duplicate them. The HUD reads the existing authored `Parameter`
recording declarations and the shared `SignalRegistry`, without electrical,
hydraulic, thermal, provenance, or magic-name logic. The generic domain
projector now classifies a `*.outputs:*` target as causal only when that source
is a member of the same generated network; otherwise it resolves the authored
network boundary source and emits the boundary equation. No fallback value or
legacy alias was added.

The storage model also fixes its authored initial state (`fixed = true`) and
declares the physical `[0, 1]` state bounds. This matters at the empty boundary:
the generated solver must start at exactly the authored zero state, so the
battery contributes zero terminal potential while all other authored sources
remain in the electrical island. Drained notification is an observable model
output; Rust does not stop actuators, restore charge, or disable solar.

**Production evidence:** the composed Summer School twin exposes all four
authored Battery channels. With autopilot driving, generated Modelica contains
the motor demand equations; motor demand is about `0.60`, motor electrical
power is about `337–343 W` each, battery discharge is about `2372 W`, net power
is negative, and SOC is below 100% and decreasing. A windowed production
capture (`target/ui-summer-autopilot8.png`) shows the driven rover HUD with
`AUTOPILOT ON` and one compact authored summary such as
`Charging 0.0 W | Net -3199.3 W | SOC 98.0 % | Used 3199.3 W`. The full
technical channel list remains available to the telemetry/API consumers.

### APP-02 — compositor ownership is only partially ordered

`EguiAboveBevyUi` correctly places egui chrome above runtime-authored HUI, but
individual modal/overlay systems still use independently chosen egui orders and
are not all part of one application overlay schedule. Help, update, tutorial,
modal-host, networking, celestial, and waypoint surfaces can therefore be
painted in a surprising order even though each one claims to be foreground.

**Implementation:** one public `ApplicationOverlayRenderSet` now orders the
modal host, help, tutorial, update, recovery, networking, avatar, Modelica,
and editor surfaces after the Workbench pass, with egui above authored Bevy
UI. World labels and route annotations remain intentionally below the
Workbench. The UI package check passes. The windowed tutorial capture
(`target/ui-tutorial.png`) shows the tutorial card above the rendered world and
the surrounding Workbench panels, confirming the authored overlay path in the
production binary.

### APP-03 — Help reports a local stamp, not a public GitHub build

`BuildIdentity` currently contains only version and a short local git stamp.
The Help menu renders that string, while the public source/release repository
and exact source-build link are absent. The updater repository is intentionally
machine-only and must not be used as the human build link.

**Implementation:** the host build script stamps version, short source revision,
dirty state, and the canonical public repository. Help renders the version and
a clickable GitHub commit URL; dirty builds retain the dirty marker instead of
pretending to be a release. The BuildIdentity unit test covers URL generation.

### APP-04 / APP-05 — Unicode glyphs were reintroduced and Linux package
identity was incomplete

The titlebar already uses vector-painted controls because missing glyphs become
tofu. Other current menus, help, tutorial, and Modelica controls still use
emoji/Unicode glyphs as visual icons. That is the regression after the earlier
tofu cleanup: the fallback font is not the root fix. Separately,
`lunco-luncosim/build.rs` rasterizes an SVG for `winit::Window::set_window_icon`.
The latest GitHub Linux AppImage contains a valid root PNG and `.DirIcon`, but
its generated desktop entry declares `StartupWMClass=LunCoSim-linux-x64` while
the compiled Bevy/winit window used `luncosim`. That identity mismatch prevents
Linux shells from associating the running window with the packaged icon.

**Implementation:** all scanned UI control glyphs were replaced with the shared
vector `UiIcon` vocabulary, including Mission Control, Celestial time, busy
cancellation, and Modelica experiment controls. The Rust build script renders
the canonical per-platform SVG and emits the package-native outputs: an
embedded Windows ICO/PE resource, a macOS iconset for `iconutil`, and Linux
hicolor PNGs. `build_native.sh` passes the resulting `.ico`, `.icns`, or `.png`
to Velopack, so the main executable and the generated installer/AppImage use
the same source artwork. `build_native.sh` now uses the fixed `luncosim`
identity for both the compiled Linux window and Velopack package, then verifies
the completed AppImage's root desktop/icon contract. A fresh GitHub Actions
package and platform-native inspection remain required for acceptance.

**Follow-up 2026-09-01:** the exact GitHub AppImage
`nightly-20260831T111355Z` (target `2b06680aa`) has the correct root
`luncosim.desktop`, root icon, `.DirIcon`, `Exec=luncosim`, and
`StartupWMClass=luncosim`, but extraction also found a redundant nested
`usr/bin/LunCoSim.desktop` left by staging. The packaging path now leaves the
desktop entry solely to Velopack and fails verification if any nested duplicate
or identity mismatch exists. A new GitHub Actions package is required to
confirm the cleaned artifact and complete platform-native acceptance.

### APP-06 — Lunokhod 2 terrain path needs runtime lifecycle proof

The `lunokhod2` terrain variant composes a real cached DEM directory and a valid
2 km / 512-sample request. The production binary now loads the variant, answers
finite `TerrainHeight` queries, and reports the composed terrain attributes
including the DEM cache, 512 target resolution, and collider ring. The existing
generation ownership and scene-teardown cancellation remain the authoritative
lifecycle mechanisms; no timeout terrain cancellation or fallback was added.

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

The state projection had been corrected to publish an active label and accent
colour, but the HUI template still put the bound label directly in a
`button`'s text content. HUI buttons render authored child `text` nodes; that
content therefore had no visible text node, and the auto-sized button collapsed
to a small empty control in the windowed path.

**Implementation:** the HUD now renders `autopilot_label` through an authored
child `text` node and gives the control an explicit interactive minimum size.
The producer publishes `AUTOPILOT ON`/`AUTOPILOT` and uses the normal
active/accent token while engaged; red is reserved for fault/refusal
presentation. The final production capture shows the text visibly rendered.

### APP-11 — battery status and current power use are not an operator summary

The previous HUD displayed every selected channel with technical names and
three decimal places. It also relied on unsupported `white-space` and
`text-overflow` CSS properties, so the long string wrapped into a multi-line
block. The electrical truth was present, but the operator could not scan it.

**Implementation:** the existing standard USD `ui:displayName` authoring field
is now the explicit membership and label for the compact operator summary. USD
telemetry projection carries it through the existing `Callsign` marker; no
domain names or numeric heuristics are added. Battery authors one child
declaration per channel with `SOC`, `Charging`, `Net`, and `Used` labels. The
HUD formats those authored values compactly on one line, while unpromoted
channels remain in the full telemetry catalog/API. Unsupported CSS properties
were removed, and the card has enough authored layout width for the compact
contract. The production exposure and screenshot above verify charging power,
net flow, SOC, and live discharge/use power while driving.

### APP-12 — a presentation contract error is treated as tutorial termination

`TutorialTargetUnavailable` is raised when an authored help anchor is absent.
`lunco-tutorial` converts it to `TUTORIAL_FAILED` and immediately triggers
`SkipTutorial`, which clears the host, overlays, and owned scene. A missing panel
is recoverable presentation state, not evidence that the lesson simulation is
invalid.

**Implementation:** the lesson host and scene remain alive. The workbench now
shows a topmost recovery surface with Continue/Retry/Stop actions, while the
tutorial owner clears only the invalid target. Continue reuses the existing
registered typed `TutorialNext` command for an active tour; Stop remains the
explicit lifecycle command. The regression
`missing_anchor_keeps_lesson_running_and_advances_on_continue` passes, and the
change is committed in `a863b48b2` and `517a6853e`.

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

Groups 1 and 2 are implemented and committed. The latest Group 1 HUD commit is
`32b6330a8`; the preceding architecture commits are `517a6853e` (authored
telemetry, overlay ordering, typed tutorial navigation), `9ecc142e6` (generic
authored-domain boundary equations), `926798c88`, and `506a0c95c` (vector UI
controls and glyph cleanup). Group 3 is implemented in the engine checkout and
Twin with focused and production headless verification. Remaining acceptance
includes a fresh GitHub Actions package and platform-native inspection of the
Windows PE/installer, macOS app bundle, and Linux AppImage; existing unrelated
Twin changes remain untouched.

## Verification addendum — 2026-08-26

The report was rerun after a cargo-clean recovery and the `tutorials` branch
integration. The production checkout now contains merge commit `9493a371b`
(`tutorials` is an ancestor of `main`), with no merge conflicts or whitespace
errors.

The Summer Space School presentation path is production-verified. A fresh
windowed `target/debug/luncosim` run reached `/api/ready` with
`pending_count: 0`; `SceneCameraAudit` found the unique authored
`/Traverse/Avatar` `LocalAvatar` camera; and `CaptureScreenshot` produced a
2561x1553 PNG with non-clear pixels (`min=0`, `max=62708`). The startup UI no
longer opens the heavy rover-build/Modelica presentation by default.

The offscreen clear-frame defect had two independent causes. Cold GPU pipelines
could consume the first capture before they were ready, and interactive API
image captures queued `Readback::texture` before the render-target write for the
frame. The recorder now applies a one-second offscreen warm-up before frame 0,
and API image captures are queued and dispatched in `Last`, sharing the same
render boundary as offline recording. Fresh production checks passed for both
paths: a 10-frame 640x360 take exited after draining with non-clear pixels
(`min=0`, `max=65535`), while interactive API `CaptureScreenshot` and
`CaptureFromCamera` each produced a 1280x720 non-clear PNG. Typed `Exit` closed
the API session and released its port.

The route projection cutover is now single-owner: `lunco-autopilot` exposes
`AuthoredRouteMetadata` for target identity, loop policy, and smoothness. The
editor and arrival handling consume it; they no longer carry duplicate XML
parsers or fall back from malformed authored XML to a stale runtime route. The
focused tests cover malformed authored data, navigation-only target extraction,
route completion, and the no-blue-first-leg contract.

The external Twin remains a separate dirty worktree and was not folded into the
engine merge. Its `sim/rovers/lunokhod2.usda` currently authors eight wheel
attachment/vehicle API pairs, eight motor shaft-speed connections, and eight
wheel-speed inputs into the electrical wrapper. The authored Modelica
`LunCo.Electrical.DCMotor` contract consumes that measured speed. Those checks
are source-backed; the file still contains unrelated pre-existing Twin edits
and must be committed/owned separately.

Focused verification after the merge: autopilot 28/28, editor 49/49, scene
commands 54/54, terrain surface 102/102, and workbench camera-capture 2/2.
The production UI build, `cargo fmt --all -- --check`, and `git diff --check`
also pass. The icon packaging cutover now has source-level contract coverage;
the post-change GitHub Actions package and platform-native icon inspections
remain open, as do untested cross-platform window captures. Generic authored
camera track capture remains outside this Twin-specific runtime proof.

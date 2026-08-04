# Bug report — `sandbox-windows-x86_64` nightly, tester session 2026-07-27

**Source:** tester terminal capture, one run, `08:36:34.816` → `08:36:56.434` UTC plus ~400 untimestamped lines after
**Platform:** Windows, AMD Radeon(TM) Graphics (IntegratedGpu, vendor 4098, device 5686), AMD proprietary driver 23.19.21.11, backend **Vulkan**
**Install dir:** `C:\Users\user\Desktop\sandbox-windows-x86_64\`
**Locale:** ru-RU (first line of the capture is `Операция успешно завершена.`)
**Scene:** `twin://sandbox/sandbox_scene.usda`, 571 prims, 243 assets, 135 USD assets, 16 bundled Modelica examples
**Predecessor:** the 2026-07-26 tester session on the same build (five runs, no scene load) — `git log -- docs/reviews/`

---

## Verdict

**The scene loaded and the renderer survived.** That is new — in five runs on 2026-07-26 the tester never once got both. The `render_robustness` ladder landed in `4b5e3183` and this log is the first evidence it works on the adapter that produced the original fault, which is exactly the thing its module docs said no test could prove:

```
08:36:37.236  ERROR  wgpu out of memory … 'directional_light_shadow_map_texture' … Not enough memory left.
08:36:37.246  WARN   wgpu validation error (frame dropped, continuing) …
08:36:37.293  WARN   GPU errors are not clearing (7 naming a shadow map, 1 out-of-memory)
                     — disabling shadow maps on 14 light(s) …
```

**57 ms** from first OOM to mitigation, and **not one wgpu error afterwards**. `GiveUp` never fired. Yesterday's identical fault produced 16 201 dropped frames at ~339 fps and was still climbing when the log ended. Issue 1 of 2026-07-26 is **closed**.

What remains is a different shape of problem. Nothing crashes now, so every defect below is a *silent* one — the app runs, reports success, and quietly does the wrong thing. Three of them are silent by construction:

| | symptom in the log | what the tester actually gets |
|---|---|---|
| Issue 1 | 400 lines of `[diag] worst dynamic body …` | a scene object accelerating out of the world, forever |
| Issue 3 | `program /SandboxScene/Amplifier bound` | a dead demo — there is no Python runtime |
| Issue 4 | `restored runtime overlay from …\.lunco\runtime\` | not the scene that is in the repo |

### Status of the 2026-07-26 findings

| 07-26 issue | today |
|---|---|
| 1. Renderer wedges after shadow-map invalidation | **Fixed** — ladder fired, recovered, no further errors |
| 2. Failed load leaks readiness ticket | Not exercised — the scene loaded |
| 3. GPU device lost on DX12 | Not exercised — Vulkan is now the Windows default (`preferred_wgpu_settings`) |
| 4. `lunco://` resolves to empty AppData cache | **Fixed / not reproduced** — 243 assets resolved from the install dir |
| 5. `BigSpace` second floating origin | Not reproduced — 4 cameras mounted cleanly as grid-direct followers |
| 6. Sim setup deadlock on unresolvable prim | Not reproduced |
| 7. Cosim connections target undeclared ports | **Still open** — 6 today (Issue 5) |
| 8. Scene cameras spawn without a render graph | **Still open** — 4 today (Issue 8) |
| 9. Log hygiene | **Regressed** — see Issue 10 |
| 10. MSL download fails | **Still open** — see Issue 9 |

---

## Issues, highest severity first

### 1. `RedBalloon` accelerates out of the world forever — and the only diagnostic that saw it was deleted two minutes after this build — **Blocker**

**Evidence.** 400 consecutive lines, monotonic, no recovery, running to the end of the capture:

```
[diag] divergence watch ACTIVE (0 dynamic bodies)          ← 08:36:35.7, before the scene spawned
[diag] worst dynamic body magnitude 1.0007368630261135e2 on /SandboxScene/RedBalloon
…
[diag] worst dynamic body magnitude 1.3101855069536657e2 on /SandboxScene/RedBalloon   ← log ends mid-line
```

Perfectly linear climb, ~7.8e-3 per sample, zero variance. Linear means terminal velocity: the body is not oscillating, not settling, not being caught by anything. It is ascending at a constant rate and will do so until the process ends.

**Root cause — the balloon is running Earth's atmosphere inside a vacuum.** `assets/models/Balloon.mo` computes a standard-atmosphere density from altitude and multiplies it by Earth gravity:

```modelica
parameter Real g = 9.81 "Gravity acceleration m/s²";
temperature = 288.15 - 0.0065 * height;
airDensity  = (101325.0 / (gasConstant * temperature)) * (1.0 - 0.0065*height/288.15)^5.255;
buoyancy    = airDensity * volume * g;
netForce    = buoyancy - drag;
```

The scene it is instantiated into is lunar:

```
08:36:36.345  INFO  [usd-avian] /SandboxScene/PhysicsScene sets gravity to 1.6200 m/s² along DVec3(0.0, -1.0, 0.0)
```

Work the numbers at h ≈ 100 m with the model's own parameters (`maxVolume = 6.0`, `mass = 4.5`, `dragCoeff = 0.47`):

| term | value |
|---|---|
| air density (model) | 1.213 kg/m³ |
| steady-state volume | 5.99 m³ |
| **buoyancy** | **71.3 N** |
| weight at 1.62 m/s² | 7.3 N |
| net lift | 64.0 N |
| **terminal ascent** | **≈ 4.7 m/s, unbounded** |

The buoyant force is **9.8× the body's lunar weight**. There is no altitude at which this model stops lifting — its density term asymptotes but never reaches zero, and nothing in the scene ever removes the force. The balloon leaves and never comes back.

**Why nothing caught it.** `crates/lunco-physics/src/escape.rs` is the diagnostic for exactly this, and it is deliberately blind to it. From its own module docs:

> **Upward: not at all — the ceiling is removed entirely.** A lander under thrust, a suborbital hop and a spacecraft on approach are all legitimately unbounded above the terrain … Escapes fall; they do not rise.

That reasoning is sound for a lander and wrong for this. `WorldBounds::Some { max.y: INFINITY }` means `report_escaped_bodies` can never fire on a rising body, no matter how far it goes.

**Why this is a blocker and not a curiosity.** Commit `ae7f1f45` — authored **08:38 UTC, two minutes after this build started** — deleted `diag_watch_bodies`, the only thing in the tree that saw this. Its message reads `refactor: remove temporary divergence diagnostic systems from escape module`, and the code comment it removed said the diagnostic existed to chase *a different bug* (`rover_comparison` spinning inside the physics step). It was retired as "temporary" while it was actively reporting an unfixed divergence in the default sandbox scene. On the next nightly this issue disappears from the logs without being fixed.

**Fix — three parts, all required:**

1. **Make the balloon honest about vacuum.** Feed ambient density in rather than deriving it from an Earth constant. Minimum viable: add `parameter Real ambientDensity = 0.0` and `parameter Real gravity = 1.62`, drive both from the scene (`PhysicsScene` gravity is already read at load), and let `buoyancy = ambientDensity * volume * gravity`. On the Moon that is 0 N and the balloon becomes a 4.5 kg sphere that falls — correct, and still a perfectly good cosim demo of Modelica → Avian force routing.
2. **Or move it.** If the balloon is meant to demonstrate buoyancy, it does not belong in `sandbox_scene.usda` at 1.62 m/s². Give it a dedicated Earth-gravity test scene.
3. **Give `escape.rs` a rise bound that does not false-positive on landers.** The absolute ceiling is genuinely undefensible; a *rate* bound is not. Flag a dynamic body whose distance from the static-collider AABB has increased monotonically for N consecutive seconds **while no thrust/force input is bound to it**. A lander under thrust has a force port; a runaway balloon driven by its own plant output is indistinguishable from one — so the cheaper discriminator is: flag any body that exits the lateral bounds *or* exceeds the AABB's largest extent in altitude **and** has non-zero net force with no user/controller authority. Log once, name the prim, name the force source.

**Regression test:** load `sandbox_scene.usda` headless, step 60 s of sim time, assert every dynamic body's `Position` is finite and within (lateral bounds, 10× AABB extent vertically). That test fails today.

---

### 2. Shadow atlas allocation is not budgeted for shared-memory adapters — the mitigation works, the cause does not — **High**

The ladder saved the session, but note *when* it fired: **2.4 seconds after the window opened**, on the very first frame that tried to render the scene. This is not a degradation under load. The shadow atlas for this scene cannot be allocated on this adapter at all, ever.

```
08:36:37.236  ERROR  wgpu out of memory on AMD Radeon(TM) Graphics (IntegratedGpu, backend Vulkan …): out of memory
  caused by: In Device::create_texture, label = 'directional_light_shadow_map_texture'
  caused by: Not enough memory left.
```

14 lights were shed to recover. The scene has 8 rovers, each with `Headlight_L` and `Headlight_R` — 16 spot/point lights plus the sun. If the atlas is dimensioned as `light_count × cascades × face_resolution`, that is the whole story, and every rover added to the scene makes it worse.

The consequence for the tester is not cosmetic. Shadows are off *for the rest of the session* (`Action::DisableShadowMaps` has no re-arm), so every screenshot from this build on integrated hardware is unshadowed and nobody looking at it knows why unless they read the log.

**Fix:**

- **Log the requested allocation.** `describe()` in `crates/lunco-workbench/src/render_robustness.rs` walks the wgpu source chain, which gives `Not enough memory left` but not *how much* was asked for. Query `RenderDevice` limits and the atlas descriptor at the point of failure and print requested bytes vs. adapter budget. Without that number this stays unfixable-by-inspection.
- **Budget before allocating, don't allocate and catch.** On `device_type == IntegratedGpu`, cap the shadow-caster count and cascade resolution up front — nearest-N casters to the active camera, reduced cascade count, half-resolution atlas. Degrading deliberately at startup beats a hard OOM plus a session-wide feature kill.
- **Make the degradation visible in the UI, not only in the log.** A workbench status chip ("shadows disabled — GPU memory") costs one line and saves a bug report.
- **Re-arm on scene unload.** The warning already promises "reload after closing some scene content to get them back", but `Ladder` has no transition back to `Rung::Healthy`. Either implement it or fix the message.

---

### 3. Two Python programs report `bound` against a Python runtime that is not there — **High**

```
08:36:35.131  INFO  lunco_scripting::python: Python status: Unavailable
…
08:36:36.657  INFO  [usd-cosim] program /SandboxScene/GreenBalloon bound (lunco://models/GreenBalloon.py)
08:36:36.664  INFO  [usd-cosim] program /SandboxScene/Amplifier bound (lunco://models/Amplifier.py)
```

Both bind at INFO. Neither is mentioned again. No warning, no error, no fallback.

`Amplifier` is the middle link of the scene's flagship mixed-language demo, documented in `sandbox_scene.usda` itself:

> Three-prim minimal mixed-language cosim: `Oscillator` (Modelica) → `Amplifier` (Python) → `CosimTarget` (Avian). Result: the target sphere bobs ±50 N at 1 Hz.

The target sphere does not bob. The Oscillator compiles and runs (`compile Oscillator finished in 0.01s (OK)`), produces `signal`, and the chain dies at the Python hop in silence. `GreenBalloon` is inert for the same reason. A tester following the scene's own documentation sees a static sphere and has no way to learn why.

**Fix:**

- **`bind` must fail when its interpreter is absent.** In `lunco-usd-sim::cosim`, check `get_python_status()` before binding a `.py` program. On `Unavailable`, log `WARN`/`ERROR` naming the prim, the asset, and the reason, and do not report `bound`.
- **Aggregate at startup.** One line after scene load: `N Python program(s) in this scene are inert — Python runtime unavailable`. A per-prim warning buried 300 lines up is not enough.
- **Decide whether Windows nightlies ship a Python.** `crates/lunco-scripting/src/python/mod.rs` probes for a shared library and reports `Unavailable` when it finds none. If the answer is "no Python on Windows", then the Python prims in the default scene are shipping known-dead and should be moved to a scene that documents the requirement.

---

### 4. Recorded session state is committed to the repo and ships inside the nightly, then overwrites the authored scene at startup — **High**

```
08:36:36.192  INFO  [usd-runtime] restored runtime overlay from
              C:\…\assets\scenes\sandbox\.lunco\runtime\sandbox_scene.usda
```

That file is **tracked in git**:

```
$ git ls-files assets/scenes/sandbox/.lunco/
assets/scenes/sandbox/.lunco/runtime/lander_test.usda
assets/scenes/sandbox/.lunco/runtime/sandbox_scene.usda
```

`.gitignore` covers `target/`, `.cache/`, `dist/` — but not `.lunco/`. Its 100 lines are someone's interactive session, captured verbatim:

```usda
over "SandboxScene" {
    over "Skid_Raycast_1" {
        def LunCoProgramAPI "Mission" {
            string info:sourceCode = """<root BTCPP_format="4" main_tree_to_execute="MainTree">
              <Action ID="drive_to" target="-0.492109061394828;0.0;-25.01599508837112"/>
              …
              <Action ID="drive_to" target="50.044711;12.793973;-25.310144"/>
```

Hand-clicked waypoints at 15-digit precision, including one at **y = 12.79 m** — a drive-to target 12.8 metres in the air. Every tester who launches this nightly gets `Skid_Raycast_1` running an infinite loop of someone else's mouse clicks, layered over the authored scene, and every bug filed against rover behaviour is filed against that.

This is also the most likely explanation for a class of "cannot reproduce" between the tester's box and a dev checkout: the overlay persists per-install and diverges the moment anyone touches the scene.

**Fix:**

1. `git rm -r --cached assets/scenes/sandbox/.lunco/` and add `.lunco/` to `.gitignore`. Runtime overlay is per-user session state; it is the same category as `target/`.
2. Exclude `.lunco/` from the release packaging step in `scripts/` — belt and braces, so a stray local overlay cannot ship again.
3. Consider gating restore behind an explicit opt-in for a *fresh* install: if there is no prior session for this install, there is nothing legitimate to restore.
4. If a mission behaviour tree is genuinely wanted in the sandbox scene, author it in `sandbox_scene.usda` with round numbers and a comment — not as a recorded overlay.

---

### 5. Six cosim connections target undeclared input ports — including avatar movement landing on a camera — **Medium (regression of 07-26 #7, not fixed)**

```
08:36:37.211  WARN  [cosim] SetPorts targets unknown input port 'forward' on 1277v0 — value dropped
08:36:37.211  WARN  [cosim] SetPorts targets unknown input port 'side'    on 1277v0 — value dropped
08:36:37.213  WARN  [cosim] SetPorts targets unknown input port 'up'      on 1277v0 — value dropped
08:36:37.253  WARN  [cosim] connection targets unknown input port 'drive_left'  on 1732v0 — value dropped
08:36:37.254  WARN  [cosim] connection targets unknown input port 'drive_right' on 1732v0 — value dropped
08:36:37.255  WARN  [cosim] connection targets unknown input port 'angle'       on 1389v0 — value dropped
```

The first three are the interesting ones. Entity `1277v0` is identified 0.86 s earlier:

```
08:36:36.352  INFO  [usd-bevy] /SandboxScene/Avatar Camera → inactive SceneCamera (perspective)
```

`forward` / `side` / `up` are the avatar's movement intents — `crates/lunco-avatar/src/lib.rs:993` binds `MoveForward → "forward"`. They are being written to the avatar's **camera**, not to the avatar body. Whatever resolves the avatar's control target is picking the camera child instead of the rigid body. The values are dropped, so the symptom the tester sees is "WASD does nothing", with a warning that reads like a scene-authoring problem.

`drive_left` / `drive_right` on `1732v0` are a drivetrain wire — `drive_left` is declared across six rover assets (`skid_rover.usda`, `ackermann_rover.usda`, …), so the port name is right and the *target* is wrong, same shape of bug.

**Fix:**

- **Resolve control targets to the body, not the first matching descendant.** Find the resolver behind `SetPorts` and make it select the prim carrying `InputPorts` / the rigid body, skipping `SceneCamera` entities. Add a test: an avatar with a child camera routes `forward` to the body.
- **Name the prim in the warning, not the entity id.** `1277v0` is unactionable in a tester's log; `/SandboxScene/Avatar Camera` is a bug report. `crates/lunco-cosim/src/lib.rs:350` already fetches `GlobalEntityId` — resolve and print the SDF path too.
- **Promote to ERROR when the target has *no* input ports at all.** A typo'd port on a real endpoint is a warning; a wire into an entity that declares nothing is a wiring bug and should be loud.

---

### 6. Two startup-order races warn once, latch forever, and self-heal 0.5 s later — **Medium**

```
08:36:37.231  WARN  [celestial] site-anchored scene has 0 Solar Grid entities (need exactly 1)
                    — cannot anchor the solar frame, so the sun stays ECLIPTIC-aligned and
                      the scene will render unlit
08:36:37.292  INFO  celestial takeover: obstacle-field subsystem defaulted OFF (site-anchored scene)
08:36:37.647  INFO  [celestial] body 399 took its imagery from dataset 'earth'
```

```
08:36:37.246  WARN  [environment] 34 co-sim model(s) want a local Earth direction, but
                    `EarthDirectionWorld` is degenerate — no Earth-relative port will be published
                    and every Earth-tracking mechanism will hold its authored pose …
08:36:37.742  INFO  [environment] local Earth direction is available again
```

Neither warning is true by the time it is read. The scene *does* reference `@lunco://celestial/solar_system.usda@</SolarSystem>` (`sandbox_scene.usda:46`) and the solar frame *does* anchor — 61 ms later. `place_site_anchored_solar_frame` in `crates/lunco-celestial/src/placement.rs:135` simply runs before the celestial big-space setup has spawned the grid, and its `*warned` latch (line 136) is one-shot, so the false alarm is the permanent record.

The messages are also disproportionate: "the scene will render unlit" and "every Earth-tracking mechanism will hold its authored pose" are exactly the symptoms of the genuine failure this warning was written for, so a tester reading the log will attribute the (real, unrelated) shadow loss from Issue 2 to this line.

**Fix:** defer the verdict instead of latching the first observation. Give both checks a settle window — warn only if the condition still holds after the scene has finished loading (a `SceneLoaded`/readiness gate, or N frames). Both already detect recovery (`local Earth direction is available again`), so the state machine exists; it just needs to run before the warning, not after. Where a warning has been superseded, say so explicitly.

---

### 7. The gate-effectiveness diagnostic reports gates that are `every_frame()` by design — **Medium**

```
08:36:44.091  WARN  [gate] `lunco_sandbox_edit::ui::refresh_view_help_controls` fired on 300/300 (100%)
                    — this run condition is not gating … Check what marks its inputs dirty every frame.
08:36:44.091  WARN  [gate] `…::inspector::populate_inspector_view` fired on 297/300 (99%)
08:36:44.095  WARN  [gate] `…::command_deck::populate_command_deck_view` fired on 300/300 (100%)
```

Two of the three are false positives *by construction*. `crates/lunco-sandbox-edit/src/ui/mod.rs:254` and `:450` register those systems with `every_frame()`, which is literally `|| true` (`:135`). The registration site even says so:

> `// One ControllerLink lookup and three Ref checks on a single entity, then an early return — an O(1) live readout, the sanctioned every_frame shape.`

`add_view_model` wraps *every* gate in `tracked()` on purpose (":no per-site discipline required, and no way to forget"), which is the right design — but it means a deliberate always-true gate is guaranteed to trip a warning telling the developer to go find what dirties its inputs. Nothing does. The advice is unfollowable, and a diagnostic that cries wolf twice out of three times trains people to skip the third.

`populate_inspector_view` at 297/300 is the real signal, and it is buried between two false ones. Its condition (`inspector_inputs_changed`, `inspector.rs:216`) is documented to return `false` "on an idle scene with nothing selected" — it is returning `true` 99% of the time on an idle scene, so something in `Changed<Transform>` / `Changed<DirectionalLight>` / `Changed<Exposure>` is dirtying every frame. Given 14 lights were being reconfigured by the shadow-map ladder, and the celestial system re-writes sun exposure per frame, `Changed<Exposure>` on the camera query is the first place to look.

**Fix:**

- Have `every_frame()` return a marker the tracker recognises (e.g. register it under a sentinel name, or add `add_view_model_unconditional()`), and have `report_ineffective_gates` skip it. A gate that declares itself unconditional is not a finding.
- Then investigate `populate_inspector_view` on its own merits, with a clean log.

---

### 8. Four scene cameras warn about a missing render graph — **Low (regression of 07-26 #8, cosmetic)**

```
08:36:36.352  WARN  bevy_render::camera: Entity 1277v0 has a `Camera` component, but it doesn't
                    have a render graph configured …            (also 1278v0, 1335v0, 1750v0)
```

Downgraded from yesterday's Medium: this is a one-frame ordering artifact, not a functional fault. `crates/lunco-usd-bevy/src/camera.rs:130` inserts a bare `Camera { is_active: false }` + `SceneCamera` marker, and `lunco-render-bevy` promotes it to `Camera3d` afterwards — confirmed in this very log, where the same entities appear as `Camera3d 1277v0` at `08:36:37.301`. All four then mount cleanly as grid-direct followers. Nothing is broken.

It is still four scary WARNs per scene load, in a log where scary WARNs are how real problems get noticed.

**Fix:** insert the render-graph component in the same command as the `Camera`, or spawn the camera disabled and insert `Camera` only after `lunco-render-bevy` has run. Failing that, suppress `bevy_render::camera`'s warning for entities carrying `SceneCamera`.

---

### 9. Modelica Standard Library: 0 system libraries, download refused, and bundled examples dropped 20 → 16 — **Low (environmental) + a packaging question**

```
08:36:35.357  INFO  [MSL] no on-disk root — starting background install
  ↓ downloading Modelica Standard Library (…/v4.1.0.tar.gz)...
08:36:35.380  INFO  [source-roots] registry built: 0 system libraries, 16 bundled examples (all NotLoaded)
08:36:35.466  INFO  [MSL] prewarmed component library: 0 entries in 382.5µs
08:36:56.434  ERROR [MSL] failed: MSL download: … io: Connection refused
```

21 seconds to fail. `Connection refused` (not a DNS or TLS failure) is a blocked egress on the tester's box — environmental, same as 07-26 #10. Two things are ours:

- **The failure is terminal and silent in the UI.** Every MSL-dependent model is unavailable for the session with no retry and no visible state. The tester only learns this from a log line 21 s in.
- **Bundled examples went 20 → 16.** Yesterday's runs reported 19–20; today reports 16. Either four examples were removed deliberately or the packaging step is dropping them. Worth confirming against the release manifest, since "bundled" is precisely the offline fallback that matters when the download is refused.

**Fix:** ship a minimal MSL subset in the release bundle so `0 system libraries` never happens offline; surface `MslLoadState` in the Modelica panel; retry with backoff rather than failing once and staying dead; and reconcile the 20 → 16 example count.

---

### 10. Log hygiene regressed — the most important signal in this capture is the one that bypasses `tracing` — **Low, fix before the next test round**

The 400 divergence lines are `eprintln!`, not `tracing`. They therefore have **no timestamp, no level, no target, and no filter** — so the single highest-severity finding in this log (Issue 1) is the one line the tester cannot grep by severity, cannot rate-limit, and cannot correlate in time with anything else. It also renders the last ~48% of the capture useless for anything else.

The capture also ends mid-token (`[diag] worst`), consistent with the terminal buffer being overrun by that spam rather than with a clean shutdown — so we do not know how the session actually ended.

Smaller items in the same category:

- ANSI escape sequences survive into the captured file (`←[2m…←[0m`), which is why this report needed a strip pass before it could be read. Detect a non-TTY sink and disable colour.
- The `WorkbenchViewportCamera` warning (4×) ends with "If this binary intentionally uses a full-window 3D camera … this warning is benign." The binary knows which it is. Decide at registration and drop the warning for the benign case.
- `[ephemeris] NAIF -1024 has no cached vectors — download 'artemis2_vectors' from Settings ▸ Downloadable data` is correct and actionable. Left here as the example of what the others should look like.

**Fix:** no `eprintln!`/`println!` in shipped crates — route through `tracing` with a target and a rate limit. Worth a clippy lint (`clippy.toml` already exists) so this cannot recur.

---

## Not bugs, but worth a decision

- **`rk45` on client-predicted models** (`08:36:38.069`) — the run exposed that non-fixed-step integration cannot serve prediction. The runtime now rejects that solver/profile pairing; `rk45` remains authoritative-live only. Qualified continuous, event-free Modelica programs use the fixed-lattice `fixed-rk4` backend instead, so peers cannot silently diverge through an adaptive stepper.
- **The balloon's feedback path** (`08:36:37.253`) — `1285v0 ↔ 1823v0` (`RedBalloon` body ↔ its `Plant`) is an explicit causal state-feedback path, not an algebraic loop. The runtime no longer fabricates a loop warning or safety fault for it; true acausal equations remain a backend-island concern.
- **Journal persistence is off by default** (`08:36:35.514`) — session-only unless `[journal] persist = true` in `twin.toml`. Reasonable default, but it means a tester who hits something interesting cannot hand us the history. Consider enabling it in nightly builds specifically.

---

## Suggested order of work

1. **Issue 4** (untrack `.lunco/`) — one commit, and everything else is measured against a scene we actually control.
2. **Issue 1** — restore a divergence check *before* `ae7f1f45`'s deletion goes out in a nightly, then fix `Balloon.mo`.
3. **Issue 3** — a two-line status check turns a dead demo into a legible one.
4. **Issue 5** — avatar control targeting a camera is a real functional bug hiding behind a warning.
5. **Issue 2** — needs the allocation-size logging first; it is not fixable by inspection.
6. **Issues 6, 7, 8, 10** — log noise, but they are why 1 and 3 went unnoticed for a day.

## Current disposition (2026-08-04)

The following dispositions are based on the current source, not the historical
tester binary above. A fresh Windows run is still required for adapter-specific
render evidence.

| issue | current disposition |
|---|---|
| 1 | **Fixed in source.** The lunar balloon uses the authored ambient/gravity contract and the escape diagnostics no longer treat upward motion as an unconditional escape. Production scene execution remains part of the verification pass. |
| 2 | **Fixed in source.** Integrated adapters receive a pre-extraction shadow budget: bounded map sizes, cascade count, and nearest-light caster limits. The workbench exposes the degradation, scene teardown re-arms it, and device loss remains terminal. Actual AMD/Vulkan Windows evidence is still unverified locally. |
| 3 | **Fixed in source.** Python programs check the authoritative interpreter status before binding, publish their declared interface as a terminal error, and emit one scene-level aggregate diagnostic. Terminal participants now retire their derived edges at the binding boundary, so unavailable Python cannot create secondary missing-port or algebraic-loop faults. The interactive sandbox intentionally has no verdict channel; `assets/scenes/tests/sandbox_smoke.usda` is the explicit composed smoke contract and the production thermal scene remains a separate gate. |
| 4 | **Closed.** Runtime overlays and history are ignored by source control and excluded by the native packaging copier; no sandbox `.lunco` files are tracked. |
| 5 | **Fixed in source.** Avatar/control resolution uses the authored control endpoint and skips camera descendants; warnings include the authored identity rather than only an entity id. |
| 6 | **Fixed in source.** Earth and solar-frame diagnostics use startup settle windows and explicitly report recovery; transient entity-construction order is not treated as a permanent scene fault. Earth-direction demand is also projected only from composed Earth-vector wires, so ordinary environment probes cannot trigger the Earth-tracking warning. |
| 7 | **Fixed in source.** Deliberate every-frame view models use an explicit registration path outside gate-effectiveness tracking, and deterministic celestial cadence declares its always-open policy to the gate monitor; accidental continuously-true gates retain actionable diagnostics. |
| 8 | **Fixed in source.** USD camera authoring carries render-free intent only; the render binder adds the complete camera/render graph atomically. |
| 9 | **Operationally surfaced.** MSL state is mirrored into the workbench status bus/settings UI and downloader retries are bounded. The captured connection refusal remains an external network/cache condition; GitHub Actions packaging still needs a post-change run. |
| 10 | **Fixed for shipped runtime diagnostics.** Window-placement warnings and the journal nesting diagnostic use logging, and native packaging disables ANSI colour for non-TTY output. CLI, REPL, and test reports intentionally retain stdout/stderr because they are their user-facing protocol. |

The remaining acceptance item is environmental rather than a compatibility path:
run the Windows nightly on the reported integrated adapter and verify the
packaged asset cache. The local production binary validates the relevant USD
and Modelica inputs, passes the thermal scene, and reports unavailable Python
programs as terminal diagnostics; the sandbox scene itself intentionally has
no PASS/FAIL scenario channel.

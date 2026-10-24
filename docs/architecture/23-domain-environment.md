# 23 — Environment Domain

> Status: Active · Audience: contributors on gravity, atmosphere, and celestial-body providers
>
> Per-entity environmental state (gravity, atmosphere, radiation, ...)
> computed from celestial-body providers. Implements the Modelica
> `inner`/`outer` pattern in ECS terms.

For in-depth engineering docs, see
**[`../../crates/lunco-environment/README.md`](../../crates/lunco-environment/README.md)**.

## Why environment is its own domain

In a space sim, "the environment" varies by location and celestial body:

| Location | g (m/s²) | Atmosphere | Solar flux |
|----------|----------|-----------|-----------|
| Earth surface | 9.81 | 101 kPa, 1.2 kg/m³ | 1361 W/m² |
| Moon surface | 1.62 | vacuum | 1361 W/m² |
| Mars surface | 3.72 | 0.6 kPa | 586 W/m² |
| LEO orbit | ~8.7 | trace | 1361 W/m² |

A global `Gravity` resource can't express "balloon on Mars while rover on
Moon." Environmental state must be **per-entity and position-dependent**.

## The three-layer pattern

```
       PROVIDERS                     COMPUTED                  CONSUMERS
   (on celestial Body)          (on each entity)

   GravityProvider ─────►    LocalGravity ────────► ConstantLinearAcceleration (Avian)
                                                    inject_environment (cosim — planned)
   AtmosphereProvider ──sys►  LocalAtmosphere ────► aerodynamic models, cosim
   RadiationProvider ───►    LocalRadiation ─────► solar panel models, cosim
```

Providers define the physics model (how does gravity vary with altitude,
what's the atmospheric density profile, etc.). `Local*` components cache
the computed value. Surface gravity reads the shared native simulation-pose
boundary and responds to ancestor motion/readiness and body links in
`FixedUpdate`; mounted consumers never use a rendered parent pose. Steady surface work selects linked
consumers and existing fields that may need cleanup; unlinked spatial entities
without a field cannot produce an update. A flat field has no position
dependence: between global invalidations it admits only spatial entities
missing `LocalGravity`, using an archetypal query. Flat-field global
invalidations check all spatial entities; surface invalidations remain
restricted to linked consumers and existing fields. Consumers
read `Local*` rather than recomputing from position.

The reusable IMU accepts the environment probe's `gravity_x/y/z` through
`double inputs:gravity_world_x/y/z`. Referencing vessels and fixtures retain
those double connection types; a float override would narrow the canonical
environment sample before the Modelica conversion. Raw rigid-body ports keep
their separately declared interface types.

`LocalGravity` owns the exact vector and its change provenance. Consumer motion
updates Avian's persistent acceleration without an
external-force wake notification, including the final movement sample at sleep
admission. Changed source fields retain native wake notifications. External
position/rotation edits use native pose wake detection. The provider-pose cache
compares physical poses once per system pass, so a prescribed sample's tick stamp
alone is not source motion. The environment owner registers the provider-pose
cache as scene state, so teardown releases its retained samples before replacement.
Native sleep thresholds and timers remain unchanged. Live integration and rollback
consume the same exact acceleration component.
The celestial site mount writes `GravityBody` only when its body identity differs
or is absent, preserving the link's change tick during ordinary scene updates.

## Lighting — the environmental analog of gravity

`lunco-environment` also owns the **physical lighting parameters of the sky**
(`lighting.rs`: `LunarSun`, `FULL_EARTH_EARTHSHINE_LUX`). Same reasoning as gravity: brightness is
environmental state that varies by body and location, not a render setting. The airless
Moon's surface is lit by exactly two things — the Sun (hard key) and earthshine (faint
cool-blue, shadowless fill).

USD light photometry is resolved by the native schema registry. Intensity,
exposure, colour, temperature controls, dimensions, and normalization are
required composed reads; malformed or unavailable values reject
the light. Graphics does not replace omitted USD photometry. Editor preview
key/fill lights have their own `editor_preview_illuminance` setting and
`EditorPreviewLight` marker, preserving their relative illumination on live
quality updates. `LightRangeAuthorship` retains the separate renderer range
choice, and dome cubemap resolution remains a Graphics setting.

### Exposure is a RATIO — `illuminance / 2^EV100`

Bevy renders physically: final pixel ≈ luminance ÷ 2^`ev100`. **Neither the light's
illuminance nor the camera's EV100 means anything on its own** — only their ratio does.
This is why `LunarSun` carries both as one resource: a scene that dims the sun cannot then
leave a camera under-exposed, because the two move together.

| Context | illuminance | EV100 | ratio | result |
|---|---|---|---|---|
| Studio / editor default | 12 000 lx | 9.7 | 14.4 | legible, intended |
| Studio "corrected" to lunar illuminance only | 128 000 lx | 9.7 | 153.9 | blown white |
| Lunar EV with studio illuminance | 12 000 lx | 16 | 0.183 | 4.1 stops under |
| **Calibrated lunar pair** | **128 000 lx** | **16** | **1.95** | correct |

`LunarSun::default()` is the canonical lunar calibration (128 000 lx / EV 16 / 0.53°
angular diameter); EV 16 lands 0.13-albedo regolith at mid-gray.
**Raise EV100 to darken the image, lower it to brighten.**

> A non-lunar scene (the luncosim) deliberately `insert_resource`s its own **studio** values
> before plugins are added, because the calibrated lunar pair crushes an editor's dark
> blueprint ground to black. That is not a bug to "fix at the source" — a shared test scene
> has many consumers. **Author cinematic values in the scene that wants them**, as an
> override on the light prim, not in the shared asset.

The exact mismatch above produced a black viewport in practice: a 10 klx luncosim sun under
a 128 klx-tuned EV16 camera.

### Uniform ambient is the SUM of authored untextured `DomeLight` prims

Uniform environment illumination is standard UsdLux — an untextured `UsdLuxDomeLight` — and
`GlobalAmbientLight` is composed as the **sum** over those domes
(`lunco-usd-bevy-light::light::on_usd_light_added`). Summing is what UsdLux semantics require:
lights add, and one light's presence must never delete another's contribution.

Two consequences that are not obvious:

- **A *textured* dome contributes nothing to that sum.** Its texture becomes IBL instead —
  the strictly better version of the same quantity. So adding a starfield sky will not
  darken an authored fill, but replacing a fill dome's texture with one will remove it from
  the sum entirely.
- A procedural starfield camera background is not a `DomeLight` and contributes no
  illumination. Its USD material is rendered as an emissive fullscreen backdrop;
  authored `DomeLight`/earthshine values remain the lighting source.
- **There is no `lunco:env:ambientBrightness`.** It is **deleted, not deprecated**, with
  deliberately **no fallback read**.

> [!WARNING]
> **This is the repo's worked example of the two-writers bug** (see `AGENTS.md` §3, "Do not
> preserve legacy, shims, or fallbacks"). A scene could once author both the custom
> attribute — assigned by the `lunco-luncosim-ui` presentation bridge — and a dome, whose sum was
> assigned by the light loader. Two writers, one field, load order deciding the winner.
> Because a textured dome contributes zero, authoring a starfield sky drove the sum to zero
> and **silently deleted the scene's regolith-bounce fill**; the projector's memoised
> `last` guard meant it never ran again to put it back. The symptom was a scene that
> rendered correctly until someone gave it a sky, and then rendered dark.
>
> If a second independent ambient contributor is ever introduced, this must become a
> composition of tracked contributions rather than an assignment — that is precisely what
> would reintroduce the bug.

### Where the remaining knobs live

The scene-level `LunCoEnvironment` prim (a singleton under the default prim, e.g.
`/World/Environment`) carries the render knobs with no natural light-prim home:
`lunco:env:exposureEv100`, `lunco:env:bloomIntensity`. **Ambient and earthshine are
deliberately not among them** — both are lights, and USD already spells lights. The
ambient slider persists onto a `DomeLight` child of that prim
(`<Environment>/AmbientFill`); earthshine's **tint** persists as `inputs:color` on the
authored `DistantLight` nested under the body it reflects from.

Its **brightness** persists nowhere, because it is not an opinion. `drive_earthshine_from_phase`
recomputes it every frame from the Sun–Earth–site geometry as
`FULL_EARTH_EARTHSHINE_LUX × (1 + cos α) / 2`, so an authored value would be overwritten by
its own driver on the next frame. There is correspondingly no slider — the inspector shows
a readout, and what moves it is the sim clock.

Every finite direction target is one position-bearing entity. Celestial bodies
derive their stable source id from their existing `lunco:body` NAIF identifier:
`sun`, `earth`, and `moon` for NAIF 10, 399, and 301, then `body_<NAIF>` for
other bodies. Do not apply `LunCoDirectionTargetAPI` to a celestial body. Other
objects use that API to author one unique lower-case id. The selected source id
is the stem of the probe output triplet (`<id>_mount_x/y/z`); it does not
determine consumer input names. Each consumer exposes a generic
`target_mount_x/y/z` vector, so the same solar panel or pointing controller can
receive a bearing to the Sun, Earth, Moon, a spacecraft, or another vehicle by
changing only its USD wire.

The environment publisher reads source demand from composed USD connections,
resolves each probe-to-target displacement through `SimulationPoseQuery`,
normalizes it once into `UnitDirection3`, then writes the
three `double` values to that probe's `SimComponent` before
`CosimSet::Propagate`. A missing, coincident, ambiguous, or unresolvable target
removes the sample and emits a structured `RuntimeDiagnostic`; no local-body
projection cache or guessed direction feeds Modelica.

Celestial point providers publish `PrescribedSimulationPose` in the active
physics frame, with the admitted cycle tick. Pending, stale, conflicting native
and prescribed ownership, invalid rotation, or a different frame makes a pose
unavailable. The publisher withdraws unavailable samples. Native bodies and
mounted probes retain the same shared reader. Solar occlusion resolves its probe
origin, direction and terrain conversion through that reader too.

Static directional lights use the same resolver as explicit framed rays. The
render light consumes the resolved `sun` ray or target for presentation,
`SunRenderState` records its finalized render direction for shadows, and
Modelica reads the probe outputs through ordinary USD connections. A celestial
scene gets finite body positions from the ephemeris driven by the one
`CelestialClock` projection of the same celestial child at each physical
communication boundary; static scenes get their fixed direction
from one authored `DistantLight`. Neither choice adds another coordinate or
time-conversion path.

`lunco-celestial-ephemeris` owns exact-epoch analytical reuse. Its provider cache
retains final ecliptic body results, native heliocentric ICRF f64 operands and
the native geocentric Moon sample. VSOP2013 EMB and ELP/MPP02 Moon each evaluate
once at a given Julian-Date bit pattern, including a missing result. Native
`Vsop2013Earth::heliocentric_position_from_samples` composes the checked samples;
the application does not implement the Earth/Moon relation. The native sample
calls own epoch validity; the provider's serialized exact-epoch boundary keeps
their epochs coherent. EMB-only queries retain their independent valid range.
A different epoch clears all samples. Reuse preserves the existing subtraction,
rotation and parent-composition order; it does not quantize time or approximate
positions. The cache belongs to the provider instance and uses its existing
serialized access boundary.

Direction target ids must be unique and valid, every composed consumer triplet
must be complete and `double`, and its three axes must come from one
probe/source or relay through one assembly input. A spawnable component may
expose the complete unconnected `target_mount_x/y/z` interface; its parent
composition supplies and validates those wires. The consumer port name is
always `target_mount_x/y/z`; the source id appears only on the probe output
wire. A `CollectionAPI:components` assembly may expose a declared program
member's triplet as `<member>_target_mount_x/y/z`. The member retains its
generic inputs and must relay all three axes through its own boundary alias;
using another member's alias is invalid. Each assembly triplet still needs a
complete double-valued connection to one probe/source. The alias names a
declared program member, never an implicit direction target.
Rhai policy `lint_usd.rhai` reports source-named consumer inputs, invalid
target identity, missing targets, ambiguous targets, incomplete triplets, and
precision narrowing before simulation.

There is no implicit direction target. In an assembled scene, a target triplet
without a valid source connection is a lint error; the publisher never guesses
Sun/Earth or fabricates zero components. A connected source that cannot be
resolved has its sample withdrawn and raises a diagnostic, and a running
co-simulation consumer faults. The authoring action is to connect one valid
source id to the generic target triplet or resolve the lint finding before
running the model.

The passive antenna, `components/comms/antenna.usda`, contains geometry and its
directional endpoint, with no nested rigid bodies, joints or controller. It
follows its parent through the ordinary hierarchy. The articulated
`components/comms/antenna_tracking.usda` references that geometry and owns its
two rigid links, hinge joints, mass properties and `AntennaTrackerController`.
A pointing scene references this component and connects a named
`LunCoDirectionTargetAPI` probe directly to the controller's
`inputs:target_mount_x/y/z`. Those inputs have no target defaults. The plain
antenna Xform does not duplicate this interface. A passive antenna needs no
station or `SolarSystem`; a target positioned relative to a celestial body
still requires that scene's celestial composition.

## Invariants

**1. Nothing here is gated on rendering.** `lunco-environment` is **render-free** —
it has no `render` feature and must not grow one ([`render-decoupling.md`](render-decoupling.md)).
Gravity, the earthshine fill and **the sun feed** all run headless.

> The `solar` module was once behind `#[cfg(feature = "render")]` despite naming
> nothing render-related. The consequence was silent: **a sun-tracking Modelica model
> running headless received nothing at all** — no error, no warning, just zeros. If a
> module here needs a renderer, it is in the wrong crate; the two things that genuinely
> did (`wire_terrain_materials` / `shade_dynamic_entities`, and the `Bloom` arm of
> `SetEnvironmentLight`) now live in `lunco-render-bevy`.

**2. Direction normalization has one owner.** The generic provider converts
finite position displacement to `UnitDirection3` once. Rigid frame rotations
preserve unit length; downstream Modelica models consume that contract directly.
Authored panel normals remain independently normalized because they are model
parameters, not coordinate-conversion output.

## Modelica `inner`/`outer` analog

Modelica's `inner World world` / `outer World world` pattern, implemented
in ECS:

| Modelica | LunCoSim |
|----------|----------|
| `inner World world` | `GravityProvider` component on the body entity |
| `outer World world` | `GravityBody { body_entity }` component on consumer entities |
| `world.g` | `LocalGravity::acceleration()` cached on consumer entities |

## Status

- **Gravity:** implemented. `LocalGravity`, `compute_local_gravity`,
  `sync_local_gravity_to_avian` (the Avian acceleration projection) all live in
  `lunco-environment`. Replaces the previous standalone `gravity_system`
  in `lunco-celestial`.
- **Atmosphere, radiation, magnetic field, thermal ambient:** scaffolded
  in the crate README as templates for future work; no implementations yet.

## Integration with co-simulation

Modelica models declare environmental needs as `input`:

```modelica
model Balloon
  input Real g = 9.81;            // will be injected from LocalGravity
  input Real airDensity = 1.225;  // from LocalAtmosphere (when implemented)
  // ...
end Balloon;
```

`lunco-environment` writes the probe's current gravity samples and requested
direction triplets to its `SimComponent.outputs`. Cosim topology remains an
ordinary USD output-to-input connection, and Modelica consumes the values at
its configured communication points. An absent provider sample is removed and
reported; the bridge does not inject an input-side alias or keep a second
per-body projection cache.

## See also

- [`../../crates/lunco-environment/README.md`](../../crates/lunco-environment/README.md) — engineering docs, how to add a new domain
- [`22-domain-cosim.md`](22-domain-cosim.md) — where the injection happens
- [`../../crates/lunco-celestial/`](../../crates/lunco-celestial/) — celestial bodies and their providers
- `specs/018-astronomical-environment` — detailed environment spec

Physical solar probes, sampling geometry, optical proxy selection, required
co-simulation samples and Modelica consumers are defined in
[the terrain illumination contract](terrain-layered-rendering.md). `SunState`
keeps physical W/m2 separate from render lux. Fixed lighting uses the authored
Sun direction and the one-AU physical calibration; celestial lighting applies
the provider's inverse-square distance at its admitted celestial epoch.

Before co-simulation initialization, `InitializationSampling` reuses the physical
gravity, direction and solar sample producers without advancing the fixed clock.
This lets required Modelica inputs be admitted while scene readiness holds that
clock. Terrain and collider preparation must be admitted before a solar visibility
sample is published; malformed authored sampling still produces a runtime fault.

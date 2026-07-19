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

   GravityProvider ─────►    LocalGravity ────────► apply_gravity_to_rigid_bodies (Avian)
                                                    inject_environment (cosim — planned)
   AtmosphereProvider ──sys►  LocalAtmosphere ────► aerodynamic models, cosim
   RadiationProvider ───►    LocalRadiation ─────► solar panel models, cosim
```

Providers define the physics model (how does gravity vary with altitude,
what's the atmospheric density profile, etc.). `Local*` components cache
the computed value at each entity's current position each `FixedUpdate`.
Consumers read `Local*` — they don't recompute from position.

## Lighting — the environmental analog of gravity

`lunco-environment` also owns the **physical lighting parameters of the sky**
(`lighting.rs`: `LunarSun`, `EarthshineParams`). Same reasoning as gravity: brightness is
environmental state that varies by body and location, not a render setting. The airless
Moon's surface is lit by exactly two things — the Sun (hard key) and earthshine (faint
cool-blue, shadowless fill).

### Exposure is a RATIO — `illuminance / 2^EV100`

Bevy renders physically: final pixel ≈ luminance ÷ 2^`ev100`. **Neither the light's
illuminance nor the camera's EV100 means anything on its own** — only their ratio does.
This is why `LunarSun` carries both as one resource: a scene that dims the sun cannot then
leave a camera under-exposed, because the two move together.

| Context | illuminance | EV100 | ratio | result |
|---|---|---|---|---|
| Studio / editor default | 12 000 lx | 9.7 | 14.4 | legible, intended |
| Studio "corrected" to lunar illuminance only | 128 000 lx | 9.7 | 153.9 | blown white |
| Lunar EV with studio illuminance | 12 000 lx | 15.4 | 0.277 | 3.4 stops under |
| **Calibrated lunar pair** | **128 000 lx** | **15.4** | **2.96** | correct |

`LunarSun::default()` is the canonical lunar calibration (128 000 lx / EV 15 / 0.53°
angular diameter); EV 15 ≈ `Exposure::SUNLIGHT` lands 0.13-albedo regolith at mid-gray.
**Raise EV100 to darken the image, lower it to brighten.**

> A non-lunar scene (the sandbox) deliberately `insert_resource`s its own **studio** values
> before plugins are added, because the calibrated lunar pair crushes an editor's dark
> blueprint ground to black. That is not a bug to "fix at the source" — a shared test scene
> has many consumers. **Author cinematic values in the scene that wants them**, as an
> override on the light prim, not in the shared asset.

The exact mismatch above produced a black viewport in practice: a 10 klx sandbox sun under
a 128 klx-tuned EV15 camera.

### Uniform ambient is the SUM of authored untextured `DomeLight` prims

Uniform environment illumination is standard UsdLux — an untextured `UsdLuxDomeLight` — and
`GlobalAmbientLight` is composed as the **sum** over those domes
(`lunco-usd-bevy::light.rs::on_usd_light_added`). Summing is what UsdLux semantics require:
lights add, and one light's presence must never delete another's contribution.

Two consequences that are not obvious:

- **A *textured* dome contributes nothing to that sum.** Its texture becomes IBL instead —
  the strictly better version of the same quantity. So adding a starfield sky will not
  darken an authored fill, but replacing a fill dome's texture with one will remove it from
  the sum entirely.
- **There is no `lunco:env:ambientBrightness`.** It is **deleted, not deprecated**, with
  deliberately **no fallback read**.

> [!WARNING]
> **This is the repo's worked example of the two-writers bug** (see `AGENTS.md` §3, "Do not
> preserve legacy, shims, or fallbacks"). A scene could once author both the custom
> attribute — assigned by `lunco-sandbox::project_env_settings` — and a dome, whose sum was
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
`lunco:env:exposureEv100`, `lunco:env:bloomIntensity`, `lunco:env:earthshineIntensity`,
`lunco:env:earthshineColor`. **Ambient is deliberately not among them** — the ambient
slider persists onto a `DomeLight` child of that prim (`<Environment>/AmbientFill`).

> These are **static almanac values** for the Shackleton region. The intended end state is
> ephemeris-driven (Sun direction/distance ⇒ illuminance and angular size; Earth phase ⇒
> earthshine), at which point the constants become the fallback and live values flow from a
> runtime `Sun`/`Earth` entity.

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

**2. Solar azimuth is NORTH-referenced** — radians clockwise from north
(`0 = N`, `+π/2 = E`, `±π = S`), which is the standard solar convention. This is the
value published on the `sun_azimuth` port and consumed by sun-tracker models.

> A south-referenced azimuth is off by exactly 180° and produces output that looks
> entirely plausible — a panel that tracks confidently in the wrong direction. The
> convention is stated on `SunDirection::azimuth` and on `SOLAR_AZIMUTH_CONNECTOR`
> for exactly that reason.

## Modelica `inner`/`outer` analog

Modelica's `inner World world` / `outer World world` pattern, implemented
in ECS:

| Modelica | LunCoSim |
|----------|----------|
| `inner World world` | `GravityProvider` component on the body entity |
| `outer World world` | `GravityBody { body_entity }` component on consumer entities |
| `world.g` | `LocalGravity(DVec3)` cached on consumer entities |

## Status

- **Gravity:** implemented. `LocalGravity`, `compute_local_gravity`,
  `apply_gravity_to_rigid_bodies` (the Avian force applier) all live in
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

A planned `inject_environment` system in `lunco-cosim` will read `Local*`
components and write matching keys into `SimComponent.inputs` — opt-in by
input name. Not yet implemented.

## See also

- [`../../crates/lunco-environment/README.md`](../../crates/lunco-environment/README.md) — engineering docs, how to add a new domain
- [`22-domain-cosim.md`](22-domain-cosim.md) — where the injection happens
- [`../../crates/lunco-celestial/`](../../crates/lunco-celestial/) — celestial bodies and their providers
- `specs/018-astronomical-environment` — detailed environment spec

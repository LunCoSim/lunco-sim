# 23 — Environment Domain

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
- Legacy `../GRAVITY_ARCHITECTURE.md` — older gravity architecture doc (to be retired)

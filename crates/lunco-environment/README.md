# lunco-environment

Per-entity environmental state for LunCoSim — gravity, atmosphere, radiation,
magnetic field, etc. — computed from celestial body providers and consumed by
physics, co-simulation, and UI.

**Currently implements:** gravity (`LocalGravity`), universal framed direction
resolution for any tagged target, lunar-sky lighting parameters (`LunarSun`,
`FULL_EARTH_EARTHSHINE_LUX`, the `SetEnvironmentLight` tuner command), and baked
horizon terrain self-shadowing (`HorizonShadowPlugin`). Environment probes expose
f64 gravity values and publish demanded unit direction triplets through ordinary
co-simulation outputs.
**Designed to grow into:** atmosphere, solar *radiation* (irradiance/eclipse),
magnetic field, ambient temperature — anything else that varies with position
and body.

> Environment direction conversion is render-free. A headless host runs the
> same target/frame conversion and cosimulation publication as a rendered host.

## Why this crate exists

In a space simulation, "the environment" means different things depending on
where you are:

| Entity on... | g (m/s²) | Atmosphere      | Solar flux  |
| ------------ | -------- | --------------- | ----------- |
| Earth surface| 9.81     | 101 kPa, 1.2 kg/m³ | 1361 W/m²   |
| Moon surface | 1.62     | none            | 1361 W/m²   |
| Mars surface | 3.72     | 0.6 kPa         | 586 W/m²    |
| LEO orbit    | 8.7      | trace           | 1361 W/m²   |

A global `Gravity` resource doesn't work — it can't express "this balloon is
on Mars while that rover is on the Moon." The environment must be **per-entity
and position-dependent**.

## The architecture

Three layers, mapped to ECS:

```
        PROVIDERS                       COMPUTED                CONSUMERS
   (on celestial Body entity)        (on each entity)

   ┌─ GravityProvider ─────────►  ┌─ LocalGravity ──────► ConstantLinearAcceleration (Avian)
   │                              │                       EnvironmentProbe outputs
   ├─ AtmosphereProvider ──sys──► ├─ LocalAtmosphere ──► aerodynamic models (planned)
   │                              │
   └─ DirectionTargetId ───────►  └─ UnitDirection3 ────► EnvironmentProbe outputs
```

| Layer | Lives on | Role |
| ----- | -------- | ---- |
| **Provider** | body entity or framed ray | Defines a field or stable direction identity |
| **Local\*** component | entity that needs a cached field | Stores computed values such as gravity at this position |
| **Compute system** | system | Reads provider plus spatial state and updates the owning component |
| **Consumer system** | system | Reads local state or publishes a demanded value to an ordinary cosim port |

## Mapping to Modelica's `inner`/`outer`

Modelica's standard pattern for environment is `inner`/`outer`:

```modelica
inner Modelica.Mechanics.MultiBody.World world;  // declared once at top level

model Balloon
  outer Modelica.Mechanics.MultiBody.World world;  // referenced from anywhere
  Real g = world.g;
end Balloon;
```

Our ECS analog:

| Modelica         | LunCoSim ECS                           |
| ---------------- | -------------------------------------- |
| `inner World`    | `GravityProvider` on the body entity   |
| `outer World`    | `GravityBody` on the consumer entity   |
| `world.g`        | `LocalGravity` on the consumer entity  |

Same scoping concept, ECS implementation. Gravity is computed once into
`LocalGravity` and used both by Avian and the probe's f64 cosim outputs. Direction
inputs use a separate generic conversion: a wire names one source id, and the
environment resolves its target bearing in that probe's own BigSpace frame.

## What's implemented

### `LocalGravity(DVec3)`

Gravity vector at an entity's world-space position, in m/s². Computed each
`FixedUpdate` from the global [`Gravity`](https://docs.rs/lunco-celestial)
resource:

- **`Gravity::Flat`** — same vector for all entities (sandbox / single-body)
- **`Gravity::Surface`** — per-entity vector via `GravityBody` link to a body
  with a `GravityProvider`

```rust
use lunco_environment::LocalGravity;

fn read_gravity(q: Query<&LocalGravity>) {
    for grav in &q {
        println!("g = {} m/s²", grav.magnitude());
        println!("down = {:?}", grav.direction());
    }
}
```

### Solar direction → Modelica

The direction system is universal. Celestial bodies use their existing NAIF
identity (`sun`, `earth`, `moon`, or `body_<NAIF>`). Apply
`LunCoDirectionTargetAPI` with a unique lower-case id to any other
position-bearing USD object, such as a spacecraft or moving vehicle. A probe connection to
`<id>_mount_x/y/z` declares demand. Before cosim propagation,
`publish_direction_sources_to_cosim` resolves that target relative to each
probe through `lunco-spatial` BigSpace helpers, normalizes the displacement
once into `UnitDirection3`, and publishes three f64 outputs. The same target can
therefore produce different vectors for two observers. A model exposes the
generic `target_mount_x/y/z` input; its wire selects the target, so changing
from Sun to Earth, Moon, or a spacecraft does not change the model equations.
There is no default target. A spawnable component may expose the complete
unconnected target input triplet for its parent assembly to wire; an assembled
consumer without a valid source connection is a lint error. The publisher
never fabricates zero components; an unresolved connected source is diagnosed
and faults a running consumer instead of selecting a guessed celestial body.

Static authored directional lights provide explicitly framed rays through the
same resolver. Celestial body positions come from the one `CelestialTime` child
of `WorldTime`; `SunState` carries irradiance only. Missing, ambiguous,
coincident, or unresolvable sources remove the sample and publish a structured
diagnostic. The authored `lint_usd.rhai` policy checks source identity,
complete double triplets, matching providers, and target cardinality.

### Lighting parameters: `LunarSun`, `FULL_EARTH_EARTHSHINE_LUX` (`render` feature)

Physical lighting state of the lunar sky — the lighting analog of gravity. The
`SetEnvironmentLight` command live-tunes the sun, the earthshine fill light
(spawned once at startup, native render only — WebGL2 allows a single
`DirectionalLight`), and bloom. `EnvironmentPlugin` also registers
`Earthshine` on the render path.

### `HorizonShadowPlugin` + `HorizonMap` (`render` feature)

Baked horizon-map terrain self-shadowing — the long-range half of the two-system
shadow design. Inert until a terrain carries the (USD-stamped)
`HorizonShadowTerrain` marker.

### `EnvironmentPlugin`

Adds `compute_local_gravity` to `FixedUpdate` in the `EnvironmentSet::Compute`
set, `sync_local_gravity_to_avian`, `inject_local_gravity_into_cosim`, and the
generic demanded-direction publisher in `EnvironmentSet::Apply`, plus the
render-free lighting and horizon state. Add it once during app setup:

```rust
app.add_plugins(lunco_celestial_spatial::GravityPlugin);
app.add_plugins(lunco_environment::EnvironmentPlugin);
```

## How to add a new environment domain

The pattern is the same for every domain — gravity is just the first one
implemented. To add atmosphere, radiation, magnetic field, etc., follow
these four steps.

### 1. Define a Provider component (lives on the body)

```rust
/// Atmospheric model on a celestial body.
#[derive(Component)]
pub struct AtmosphereProvider {
    /// Body radius (m), used to compute altitude.
    pub body_radius: f64,
    /// The atmosphere model.
    pub model: Box<dyn AtmosphereModel>,
}

/// Compute pressure (Pa), density (kg/m³), temperature (K) at an altitude (m).
pub trait AtmosphereModel: Send + Sync + 'static {
    fn at_altitude(&self, altitude_m: f64) -> (f64, f64, f64);
}

/// US Standard Atmosphere 1976 — concrete implementation.
pub struct StandardAtmosphere {
    pub t0: f64,           // sea-level temperature (K), 288.15 for Earth
    pub p0: f64,           // sea-level pressure (Pa), 101325 for Earth
    pub r_specific: f64,   // gas constant, 287.058 for dry air
    pub lapse_rate: f64,   // K/m, 0.0065 for Earth troposphere
}

impl AtmosphereModel for StandardAtmosphere {
    fn at_altitude(&self, altitude_m: f64) -> (f64, f64, f64) {
        let altitude = altitude_m.max(0.0);
        let t = self.t0 - self.lapse_rate * altitude;
        let p = self.p0 * (1.0 - self.lapse_rate * altitude / self.t0).powf(5.255);
        let rho = p / (self.r_specific * t);
        (p, rho, t)
    }
}
```

### 2. Define a Local\* component (lives on each entity)

```rust
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct LocalAtmosphere {
    pub pressure: f64,     // Pa. Vacuum = 0.
    pub density: f64,      // kg/m³. Vacuum = 0.
    pub temperature: f64,  // K. Deep space ≈ 2.7.
}
```

### 3. Add a compute system

```rust
pub fn compute_local_atmosphere(
    mut commands: Commands,
    q_bodies: Query<(&AtmosphereProvider, &Transform)>,
    q_entities: Query<(Entity, &Transform, &GravityBody)>,
) {
    for (entity, entity_tf, body_link) in &q_entities {
        let Ok((atm, body_tf)) = q_bodies.get(body_link.body_entity) else { continue };
        let altitude = (entity_tf.translation - body_tf.translation).length() as f64
            - atm.body_radius;
        let (p, rho, t) = atm.model.at_altitude(altitude);
        commands.entity(entity).insert(LocalAtmosphere {
            pressure: p, density: rho, temperature: t,
        });
    }
}
```

### 4. Register it in `EnvironmentPlugin`

```rust
impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LocalGravity>()
            .register_type::<LocalAtmosphere>();  // add new type

        app.add_systems(
            FixedUpdate,
            (
                compute_local_gravity,
                compute_local_atmosphere,  // add new system
            ).in_set(EnvironmentSet::Compute),
        );
    }
}
```

That's the entire pattern. Three components, one system. Done.

## How environment values reach Modelica models

Modelica receives environmental values through standard composed USD
output-to-input connections. The environment domain publishes probe samples;
cosim propagates those sources through its existing connection graph. There is
no second input-side injection path.

The Modelica model declares whatever environment it needs:

```modelica
model Balloon
  input Real g = 9.81;             // injected from LocalGravity
  input Real airDensity = 1.225;   // injected from LocalAtmosphere
  input Real temperature = 288.15; // injected from LocalAtmosphere
  ...
end Balloon;
```

Standalone defaults are for isolated Modelica initialization only. In a scene,
the composed USD wire defines the runtime source, and missing samples are
diagnosed rather than replaced with a fabricated environmental value.

## Roadmap

- [x] **Gravity** — `LocalGravity`, `compute_local_gravity`, `sync_local_gravity_to_avian`, `inject_local_gravity_into_cosim`
- [x] **Universal direction** — stable target ids, shared BigSpace conversion, per-probe unit vectors, and direct cosim output publication
- [x] **Lunar lighting** — `LunarSun`, `FULL_EARTH_EARTHSHINE_LUX`, `SetEnvironmentLight` tuner, earthshine fill
- [x] **Horizon self-shadowing** — `HorizonShadowPlugin`, `HorizonMap`
- [ ] **Atmosphere** — `LocalAtmosphere`, `AtmosphereProvider`, `StandardAtmosphere` model
- [ ] **Solar radiation** — `LocalRadiation` irradiance + eclipse occlusion (distinct from the direction bridge above)
- [ ] **Magnetic field** — `LocalMagneticField`, dipole + IGRF models
- [ ] **Ambient temperature** — for thermal subsystem models (radiator design, electronics cooling)

## Design notes

**Why Local\* components instead of recomputing on demand?**
Gravity is read by multiple consumers each tick (Avian acceleration,
cosim injection, UI display). Computing once and storing as a component is
faster and more idiomatic ECS than re-deriving from Position + Body each time.

**Why `Bevy` change detection?**
The compute systems observe their actual dependencies and compare the computed
value before inserting a Local\* component. Consumers such as Avian's
acceleration projection therefore wake only when the field really changes;
reactive UI can still use `Changed<LocalGravity>` for events such as crossing
an SOI boundary.

**Why opt-in by input name in `inject_environment`?**
A solar panel doesn't need `g`, so it doesn't declare `input Real g`. The
injector skips it. No "global environment fed to everything" — each model
explicitly lists what it depends on. This matches FMI's model interface
contract and prevents accidental coupling.

**Why not put this in `lunco-celestial`?**
Celestial mechanics (orbits, body kinematics) and environment computation
(per-entity local state) are different concerns. `lunco-celestial` is "what
the bodies are doing." `lunco-environment` is "what an entity feels at its
current location." The former is solar-system dynamics; the latter is
subsystem context.

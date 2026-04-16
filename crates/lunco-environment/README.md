# lunco-environment

Per-entity environmental state for LunCoSim — gravity, atmosphere, radiation,
magnetic field, etc. — computed from celestial body providers and consumed by
physics, co-simulation, and UI.

**Currently implements:** gravity only.
**Designed to grow into:** atmosphere, solar radiation, magnetic field,
ambient temperature, anything else that varies with position and body.

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

   ┌─ GravityProvider ─────────►  ┌─ LocalGravity ──────► gravity_system (Avian forces)
   │                              │                       inject_environment (cosim)
   ├─ AtmosphereProvider ──sys──► ├─ LocalAtmosphere ──► aerodynamic models
   │                              │                       inject_environment
   └─ SolarRadiationProvider ──►  └─ LocalRadiation ───► solar panel models
                                                          inject_environment
```

| Layer | Lives on | Role |
| ----- | -------- | ---- |
| **Provider** | celestial Body entity | Defines HOW the environment varies (gravity model, atmosphere model, etc.) |
| **Local\*** component | every entity | Cached COMPUTED value at this entity's position THIS tick |
| **Compute system** | (system) | Reads Provider + entity Transform → writes Local\* |
| **Consumer system** | (system) | Reads Local\* — doesn't know about celestial bodies |

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

Same scoping concept, ECS implementation. The injection from `LocalGravity`
into a Modelica model's `input Real g` happens in `lunco-cosim`'s
`inject_environment` system (not yet implemented — see roadmap below).

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

### `EnvironmentPlugin`

Adds `compute_local_gravity` to `FixedUpdate` in the
`EnvironmentSet::Compute` set. Add it once during app setup:

```rust
app.add_plugins(lunco_celestial::GravityPlugin);
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

Once `Local*` components exist, Modelica models get the values via
`lunco-cosim`'s `inject_environment` system (TBD — see roadmap):

```rust
// Sketch — to be implemented in lunco-cosim
fn inject_environment(
    q: Query<(&LocalGravity, Option<&LocalAtmosphere>, &mut SimComponent)>,
) {
    for (gravity, atm, mut comp) in &mut q {
        // Only inject inputs the model declared
        if comp.inputs.contains_key("g") {
            comp.inputs.insert("g".into(), gravity.magnitude());
        }
        if let Some(atm) = atm {
            if comp.inputs.contains_key("airDensity") {
                comp.inputs.insert("airDensity".into(), atm.density);
            }
            if comp.inputs.contains_key("temperature") {
                comp.inputs.insert("temperature".into(), atm.temperature);
            }
        }
    }
}
```

The Modelica model declares whatever environment it needs:

```modelica
model Balloon
  input Real g = 9.81;             // injected from LocalGravity
  input Real airDensity = 1.225;   // injected from LocalAtmosphere
  input Real temperature = 288.15; // injected from LocalAtmosphere
  ...
end Balloon;
```

The default values (`= 9.81`, etc.) are used during initial-condition solving
and stand-alone Modelica testing. At runtime, `inject_environment` overwrites
them each tick. Models that don't declare an input simply don't get it
injected — opt-in by name.

## Roadmap

- [x] **Gravity** — `LocalGravity`, `compute_local_gravity` (this crate)
- [ ] **Atmosphere** — `LocalAtmosphere`, `AtmosphereProvider`, `StandardAtmosphere` model
- [ ] **Radiation** — `LocalRadiation`, `SolarRadiationProvider`, distance falloff + shadow occlusion
- [ ] **Magnetic field** — `LocalMagneticField`, dipole + IGRF models
- [ ] **Ambient temperature** — for thermal subsystem models (radiator design, electronics cooling)
- [ ] **`inject_environment` in lunco-cosim** — auto-fills `SimComponent.inputs` from `Local*` components based on input name match

## Design notes

**Why Local\* components instead of recomputing on demand?**
Gravity is read by multiple consumers each tick (Avian force application,
cosim injection, UI display). Computing once and storing as a component is
faster and more idiomatic ECS than re-deriving from Position + Body each time.

**Why `Bevy` change detection?**
The compute systems write the Local\* components every tick unconditionally.
That triggers `Changed<LocalGravity>` filters in consumers — useful for
reactive UI ("gravity just changed because we crossed an SOI boundary"). If
this becomes a perf issue, add equality checks before insert.

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

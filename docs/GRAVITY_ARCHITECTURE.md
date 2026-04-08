# Gravity Architecture

> **Status**: Phase 1 implemented and working. Sandbox verified.
> **Date**: 2026-04-08

## Overview

LunCoSim uses a **resource-driven gravity system** that replaces Avian3D's built-in `Gravity`. It supports flat-ground simulations (sandbox, tests) and spherical surface gravity (Moon, Earth, Mars) with the same API.

## Core Types

### `Gravity` (Resource)

The global gravity mode. Set once during app setup.

```rust
// Sandbox / flat ground: one line, same semantics as avian3d::Gravity
app.insert_resource(Gravity::flat(9.81, DVec3::NEG_Y));

// Full client: surface gravity on spherical bodies
app.insert_resource(Gravity::surface());
```

| Variant | Behavior | Use Case |
|---------|----------|----------|
| `Gravity::Flat { g, direction }` | Same constant force on ALL `RigidBody` entities | Sandbox, tests, flat ground |
| `Gravity::Surface` | Per-entity: direction from body-local position, magnitude from body's `GravityProvider` | Moon rovers, Earth landers |

### `GravityBody` (Component)

Links an entity to the celestial body it sits on. **Only needed in `Surface` mode.**

```rust
// When spawning a rover on the Moon:
commands.entity(rover)
    .insert(GravityBody { body_entity: moon_entity })
    .set_parent_in_place(moon_entity); // child of Body → inherits rotation
```

In `Flat` mode, this component is **not needed** — all bodies get the same gravity.

### `GravityProvider` (Component)

Placed on each celestial Body entity. Holds the gravity model (GM, etc.).

```rust
// On Moon Body entity:
commands.entity(moon).insert(GravityProvider {
    model: Box::new(PointMassGravity { gm: 4.90486948e12 }),
});
```

### `GravityModel` (Trait)

The extensibility foundation. Any gravity model implements this:

```rust
pub trait GravityModel: Send + Sync + 'static {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3;
}

// Built-in:
pub struct PointMassGravity { pub gm: f64 }  // a = GM/r²
// Future:
pub struct J2Perturbation { ... }            // a = GM/r² + J2 terms
pub struct AtmosphericDrag { ... }           // velocity-dependent
```

### `LocalGravityField` (Resource)

Cached gravity state for camera/UI systems. Updated every `PreUpdate`.

```rust
pub struct LocalGravityField {
    pub body_entity: Option<Entity>,  // Which body we're bound to
    pub up: DVec3,                     // "Up" in world space
    pub local_up: DVec3,               // "Up" in body-local space
    pub surface_g: f64,                // Surface gravity magnitude (m/s²)
}
```

**Camera systems read this to determine orientation.** When the avatar possesses a rover or lands on a surface, `local_up` tells the camera which way is "up" relative to the body.

## How It Works

### Sandbox (Gravity::Flat)

```
App setup:
  insert_resource(Gravity::flat(9.81, NEG_Y))

FixedUpdate (gravity_system):
  For every RigidBody entity:
    force = NEG_Y * 9.81 * mass
    apply_force(force)
```

Zero per-entity setup. No components needed. Rovers just fall.

### Full Client (Gravity::Surface)

```
App setup:
  insert_resource(Gravity::surface())
  // Each Body has GravityProvider { model: PointMassGravity { gm } }

FixedUpdate (gravity_system):
  For every RigidBody entity with GravityBody:
    local_pos = entity.Transform.translation  (body-fixed coords)
    direction = -normalize(local_pos)          (toward body center)
    g = body.GravityProvider.model.acceleration(local_pos).length()
    force = direction * g * mass
    apply_force(force)
```

Each rover gets gravity from **its own body** — rovers on Moon get 1.625 m/s², rovers on Earth get 9.81 m/s², simultaneously.

### Avatar Positioning Based on Gravity

When the avatar possesses a surface entity (rover) or teleports to a body:

1. `update_local_gravity_field` runs in `PreUpdate`
2. Reads avatar's `Transform` and `GravityBody`
3. Computes `local_up = normalize(body_local_position)`
4. Caches `surface_g` from the body's `GravityProvider`
5. Camera systems (FreeFlight, SpringArm) read `LocalGravityField.local_up` to determine "up"

```rust
// In camera system:
let up = local_gravity_field.local_up;  // body-local "up"
let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
// Build camera frame using 'up' instead of hardcoded Vec3::Y
```

## System Registration

### Sandbox

```rust
app.insert_resource(Gravity::flat(9.81, DVec3::NEG_Y))
   .add_plugins(GravityPlugin)          // handles LocalGravityField, gravity_system, update_local_gravity_field
   .add_systems(PreUpdate, global_transform_propagation);  // sandbox's own transform system
```

### Full Client (CelestialPlugin)

`CelestialPlugin` uses `GravityPlugin` internally — no duplication:

```rust
// CelestialPlugin::build():
app.add_plugins(GravityPlugin);  // gravity + LocalGravityField + GravityBody registration

app.add_systems(PreUpdate, (
    celestial_clock_tick_system,
    ephemeris_update_system,
    body_rotation_system,
    soi_transition_system,
    // gravity handled by GravityPlugin
).chain());
```

## Future Evolution (Additive, Zero Breaking Changes)

### Phase 2: N-Body Gravity

Add one enum variant — existing modes untouched:

```rust
pub enum Gravity {
    Flat { g: f64, direction: DVec3 },
    Surface,
    NBody { sources: Vec<Entity> },  // sum of multiple bodies
}
```

Spacecraft between Earth and Moon gets pull from both simultaneously.

### Phase 3: Per-Entity ForceModel (GMAT-style)

```rust
/// Per-entity override — spacecraft can differ from global
#[derive(Component)]
pub struct ForceModel {
    pub sources: Vec<Entity>,  // which bodies contribute
}
```

### Phase 4: Composable Models

```rust
pub trait GravityContributor: Send + Sync {
    fn acceleration(&self, pos: DVec3, vel: DVec3, epoch: f64) -> DVec3;
}

// Stack: PointMass + J2 + Drag + SRP
let earth_force = vec![
    PointMass { gm: 3.986e14 }.boxed(),
    J2Perturbation { j2: 1.0826e-3 }.boxed(),
    AtmosphericDrag { ... }.boxed(),
];
```

### Phase 5: SOI-Driven Model Switching

The existing `soi_transition_system` detects sphere-of-inificance crossings and switches the entity's `ForceModel`:

| Region | Primary | Perturbations |
|--------|---------|---------------|
| Near Earth | PointMass(Earth) + J2 | Moon, Sun |
| Lunar SOI | PointMass(Moon) | Earth, Sun |
| Interplanetary | Sun | Earth, Moon, Mars, Jupiter |
| On surface | SurfaceGravity (constant g) | — |

## Known Weaknesses

| Issue | Impact | Severity | Fix |
|-------|--------|----------|-----|
| Missing `GravityProvider` → silent 0 gravity | Hard to debug | 🟠 | Add `debug_assert` |
| Avatar without `GravityBody` → `surface_g = 0` | Edge case | 🟢 | Fallback to nearest body |
| Per-tick `Query::get()` for 1000+ entities | Micro-optimization | 🟡 | Cache in HashMap resource |
| Coupled to Avian3D `Forces` | Engine lock-in | 🟡 | Write to `GravityForce` component, separate adapter |

## Files

| File | Purpose |
|------|---------|
| `crates/lunco-celestial/src/gravity.rs` | Core types + systems |
| `crates/lunco-celestial/src/lib.rs` | `CelestialPlugin` + `GravityPlugin` |
| `crates/lunco-client/src/bin/rover_sandbox.rs` | Flat gravity demo |

# Avian Port Backend — unify avian exposure behind one spec table

**Status:** planned (2026-06-21). Mechanism decision **locked: typed-closure spec
table** (not reflection — see §2). Builds on the `BACKENDS` resolver table
(`lunco-cosim/src/ports.rs`) and the fabric-order/guard work already shipped.

## 1. Problem

Every avian feature we expose as co-sim ports costs **five hand-written pieces**:
a marker struct with mirror `HashMap`s, a `Default` that hard-codes port names,
an `on_add_*` observer, an `apply_*` drive system, and a `read_*` output system.
`RigidBody` and `RevoluteJoint` each pay this in full (`avian.rs`, `joint.rs`,
`systems/apply_forces.rs`, `systems/step_avian.rs`, plus observers in `lib.rs`).
Adding `PrismaticJoint`, sensors, springs, more body state → 5 more pieces each,
and the copies drift (motor overdamp lives in one place, `DQuat→Quat` in another).

Hidden inside this is a second smell: the **mirror `HashMap`s**. `read_avian_outputs`
copies `Position.0.y → AvianSim.outputs["height"]` every tick so a wire can read
the map. The value is stored twice and a per-tick system exists only to keep the
copy in sync. The avian component should *be* the port store.

## 2. Decision: typed-closure table, not reflection

Avian's components derive `Reflect`, so reflection *could* auto-expose fields.
We rejected it for the avian (foreign-type) side:

- We don't own avian's types → can't `#[derive(SimExposed)]` on them; reflection
  means stringly-typed reflect-paths resolved through the `TypeRegistry`
  (runtime-typed, version-coupled strings, downcast dance).
- The hard ports need code regardless: twist `angle` is computed (no field path),
  motor-write is not a field set, force is an additive sink with a query-shaped
  API. Reflection only helps the *trivial* field ports — already one-line closures.
- Reflection's headline feature (zero-config discovery of un-anticipated fields)
  is a **misfeature** here: it would leak dozens of internal fields under ugly
  names (`Position.0.y`) as a de-facto wire API. We want a **curated, named**
  surface (`force_y`, `height`, `angle`).
- Closures are direct typed access; reflect-paths would force a bind-time accessor
  cache just to recover the speed.

`#[derive(SimExposed)]` stays the right answer for **our own** `SimComponent`-family
types (TODO(ports)). Ownership decides the mechanism: derive for owned, external
typed-closure table for foreign (avian).

## 3. The four realization mechanisms

The entire avian surface — present and future — reduces to four mechanisms:

| Mechanism | Read | Write | Example |
|---|---|---|---|
| **field read** | typed `world.get::<T>` | — | `position_y` ← `Position.0.y` |
| **additive sink** | — | accumulate, applied + cleared per step | `force_y` → `Forces` |
| **semantic write** | — | set field(s) + side effects | `angle` → `motor.target_position` (+enable, +`max_torque` default) |
| **derived read** | compute from other components | — | `angle` ← `twist(b1, b2, axis)` |

## 4. Shape

One module — `lunco-cosim/src/backend/avian/` — becomes **the sole point of avian
coupling**.

```rust
struct AvianPort {
    name: &'static str,
    dir: PortDirection,
    ptype: PortType,
    read:  Option<fn(&World, Entity) -> Option<f64>>,    // None = write-only (force)
    write: Option<fn(&mut World, Entity, f64) -> bool>,  // None = read-only (state)
}

struct AvianGroup {
    present: fn(&World, Entity) -> bool,   // |w,e| w.get::<RevoluteJoint>(e).is_some()
    ports: &'static [AvianPort],
}

const AVIAN: &[AvianGroup] = &[
    rigid_body(),       // Position/LinearVelocity reads + force sinks
    revolute_joint(),   // motor-write in, twist-derived out
    // prismatic_joint(),  ← adding a joint type is one line here
];
```

This is a **single** `PortBackend` entry in `BACKENDS` (`ports.rs`). `entity_ports`,
`read_output_port`, `read_port`, `write_port` fold over it exactly as they do the
other backends — no change to the four public resolver functions or to `propagate`.

### 4.1 Ported spec (the de-facto spec made explicit)

`rigid_body()` (gate: `RigidBody` present):

- out `position_x/y/z` ← `Position.0.{x,y,z}`  (field read)
- out `velocity_x/y/z` ← `LinearVelocity.0.{x,y,z}`  (field read)
- out `height` ← `Position.0.y`  (alias, field read)
- in  `force_x/y/z` → `PendingForces.f.{x,y,z}`  (additive sink, see §5)

`revolute_joint()` (gate: `RevoluteJoint` present):

- in  `angle` → `motor.target_position = v` + `enabled = true` + `motor_model =
  JOINT_MOTOR_MODEL` + `max_torque` default if `<= 0`  (semantic write)
- out `angle` ← `twist(body1.Rotation, body2.Rotation, hinge_axis)`  (derived read)

## 5. The one wrinkle: query-shaped force API

Avian's force write is `Query<Forces>` / `WriteRigidBodyForces`, not a plain
`world.get_mut::<T>()`. So the additive sink cannot fully live in a resolver
closure. Resolution:

- The `force_*` **write closure** sets a typed `PendingForces { f: DVec3, t: DVec3 }`
  component (`world.get_mut`/insert). Per-axis writes (`f.x/f.y/f.z`) are
  independent; `propagate` already sums multiple wires into one value per target,
  so the closure overwrites (not `+=`).
- **One** generic `apply_pending_forces` system (`Query<(&mut PendingForces,
  Forces)>`) applies the vector and zeroes it. Avian clears non-constant forces
  each step (proven by today's re-apply-every-tick working), so per-tick apply is
  correct.

Net: this single generic system is the *only* surviving per-tick avian system, and
it exists solely to bridge avian's query-shaped force API — it is not bespoke
per-type.

## 6. What is deleted / what migrates

**Deleted:**
- structs `AvianSim`, `JointSim` (+ their `Default`/`init_*`/`read_state`/`take_inputs`)
- mirror `HashMap`s and their sync systems: `read_avian_outputs`, `read_joint_outputs`
- observers `on_add_rigid_body`, `on_add_revolute_joint` (presence is queried, motor
  defaults set lazily in the write closure)
- `apply_sim_forces` (replaced by the single generic `apply_pending_forces`)
- `apply_joint_drives` (folded into the `angle` semantic-write closure)

**Migrated:**
- `register_type::<AvianSim/JointSim>()` removed; register `PendingForces`.
- inspector `joint_holder` / `joint_control_section`: `With<JointSim>` →
  `With<RevoluteJoint>`; reads/writes already go through `read_port`/`write_port`
  so only the holder query changes.
- `lunco-usd-sim` and the API/`ListPorts` naming reference entities, not the marker
  structs — unaffected (they go through the resolver).
- `pub use avian::*; pub use joint::*;` exports: drop the removed symbols.

## 7. Scheduling — preserved, not changed

- `propagate` (FixedUpdate, `CosimSet::Propagate`, gated `!Client`) — unchanged;
  reads pull avian state live in read closures, writes push to `PendingForces` /
  motor / inputs.
- `apply_pending_forces` (FixedUpdate, `CosimSet::ApplyForces`, after Propagate,
  gated `!Client`) — replaces `apply_sim_forces`/`apply_joint_drives`.
- output reads are **on-demand** in read closures: avian `Position`/`Rotation` are
  stable between steps (post-Writeback last step = pre-step this tick), so reading
  live during the next `propagate` is latency-equivalent to today's post-Writeback
  snapshot. No separate read system needed.
- `ControlDacSet.before(Propagate)` (item 1) — unchanged.

## 8. Combine policy (folds in for free)

The mechanism tag *is* the combine policy: **additive sink** sums (forces), **field/
semantic/derived** replace (motor target, state). This subsumes the separate
"per-port replace vs sum" fork — no extra machinery.

## 9. Build order

1. **Phase A — refactor body + revolute behind the table, behavior-preserving.**
   Regression oracle: the live sun-tracker scene (`ang_in`→motor→`ang_out`
   convergence) + balloon force tests + existing cosim tests. No new behavior.
2. **Phase B — add `prismatic_joint()` as table rows only** (out `position` ←
   derived slide distance, in `position` → motor target). Zero new structs/systems
   — the proof the abstraction generalizes.

## 10. Out of scope / future

- **Discrete events** (contacts, collisions) are event *streams*, not scalar state —
  they do not fit the scalar-port model and need a separate event→signal bridge.
- **Perf**: per-tick closure access is direct/typed; if `propagate` ever shows in a
  profile, bind-time accessor caching (the SignalBus flat-index TODO) applies
  uniformly and is independent of this refactor.
- **Multi-DOF joints** (6-DOF) expose several ports; the single-`angle` assumption
  is revolute-specific and handled per-group, not globally.

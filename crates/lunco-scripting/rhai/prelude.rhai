// LunCo scripting prelude — ergonomic wrappers over the three host verbs.
//
// The host (Rust) exposes only the minimal, generic bridge:
//   cmd(name, #{params}) -> i64   write: fire any #[Command] by name
//   query(name, #{params})        read:  call any ApiQueryProvider (Raycast,
//                                        Nearest, GroundHeight, …) — the read
//                                        twin of cmd(); physics stays in its
//                                        owning crate, not in scripting
//   world_pos(id) -> [x,y,z]      read:  float-origin-correct world position
//   world_forward(id) -> [x,y,z]  read:  world heading
//   get(id, "Comp.field")         read:  reflected field; vectors/quats come
//                                        back as arrays ([x,y,z]/[x,y,z,w]),
//                                        structs as maps
//   list_entities() -> [#{id,name}]
//   find(name) -> id
//   sim_tick() -> i64             time:  current fixed tick (integer)
//   dt() -> f64                   time:  fixed-step delta in seconds (1/60)
//   elapsed_seconds() -> f64      time:  monotonic sim seconds since startup
//   emit(name, value)             event: fire a TelemetryEvent
//   on_event(self, evt)           event: receives emit()s AND physics
//                                        COLLISION_START / COLLISION_END pulses
//
// Everything below is POLICY, authored here (hot-reloadable), never compiled
// into core. Add helpers freely — no Rust rebuild required.

// ── Math on the [x,y,z] arrays world_pos returns ───────────────────────────

fn vsub(a, b) { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn vadd(a, b) { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn vlen(a)    { (a[0]*a[0] + a[1]*a[1] + a[2]*a[2]).sqrt() }
fn vdot(a, b) { a[0]*b[0] + a[1]*b[1] + a[2]*b[2] }
fn vcross(a, b) {
    [ a[1]*b[2] - a[2]*b[1],
      a[2]*b[0] - a[0]*b[2],
      a[0]*b[1] - a[1]*b[0] ]
}
fn vnorm(a) {
    let l = vlen(a);
    if l <= 0.0 { a } else { [a[0]/l, a[1]/l, a[2]/l] }
}
fn clamp(x, lo, hi) { if x < lo { lo } else if x > hi { hi } else { x } }

/// Distance between two entities in metres, or -1.0 if either lacks a position.
fn distance(a, b) {
    let pa = world_pos(a);
    let pb = world_pos(b);
    if pa == () || pb == () { return -1.0; }
    vlen(vsub(pa, pb))
}

/// True once `entity` is within `radius` metres of world point `target` ([x,y,z]).
fn arrived(entity, target, radius) {
    let p = world_pos(entity);
    if p == () { return false; }
    vlen(vsub(p, target)) <= radius
}

// ── Navigation (PURE rhai policy over the world_pos/world_forward reads) ─────
//
// All steering math lives here — Rust only exposes the raw position/heading
// reads + the DriveRover command. Tune or replace freely; no Rust rebuild.

/// Signed steer command in [-1, 1] that turns `entity` toward world point
/// `target`. Sign from the yaw-plane (up = +Y) cross product of heading × goal.
fn steer_to(entity, target) {
    let p = world_pos(entity);
    let f = world_forward(entity);
    if p == () || f == () { return 0.0; }
    let to  = vnorm(vsub(target, p));
    let fwd = vnorm(f);
    let cross = vcross(fwd, to);
    clamp(cross[1], -1.0, 1.0)   // +Y component = left/right yaw error
}

/// Drive `entity` toward world point `target` at `speed`. Returns true once
/// within `radius` (and brakes). The core per-tick navigation primitive.
/// (Named `nav_to`, not `goto` — `goto` is a reserved word in rhai.)
fn nav_to(entity, target, speed, radius) {
    if arrived(entity, target, radius) {
        brake(entity);
        return true;
    }
    drive(entity, speed, steer_to(entity, target));
    false
}

// ── Declarative plan / objectives (PURE rhai — objectives live here, not core) ─
//
// A "plan" is just data: an array of world waypoints `[[x,y,z], ...]`. run_plan
// advances one objective at a time, emitting OBJECTIVE_COMPLETE per waypoint and
// PLAN_COMPLETE at the end. The step index IS the resumable state (no coroutines
// needed). IMPORTANT rhai rule: `this` is bound ONLY in the hook that the engine
// calls (on_tick), NOT in functions the hook calls. So run_plan is STATELESS —
// it takes the current index and returns the next; the hook holds the index on
// `this`. Richer objectives (sample, dwell, wait-for-event) compose the same way.

/// Advance a waypoint plan by one tick. `idx` is the current objective index;
/// returns the next index (incremented when the current waypoint is reached).
/// When `idx >= plan.len()` the plan is complete. Typical use in on_tick:
///   `this.idx = run_plan(me, PLAN, this.idx, SPEED, RADIUS);`
fn run_plan(entity, plan, idx, speed, radius) {
    if idx >= plan.len() { return idx; }
    let wp = plan[idx];
    if nav_to(entity, wp, speed, radius) {
        emit("OBJECTIVE_COMPLETE", idx);
        idx += 1;
        if idx >= plan.len() { emit("PLAN_COMPLETE", true); }
    }
    idx
}

// ── Rover control (re-emit every tick: DriveRover has no persistent setpoint) ──

/// Drive a rover this tick. `seq`/`tick` default to 0 — fine for host-local
/// scenarios; networked control should thread real sequence/tick numbers.
fn drive(rover, forward, steer) {
    cmd("DriveRover", #{ target: rover, forward: forward, steer: steer, seq: 0, tick: 0 })
}

/// Stop a rover this tick.
fn brake(rover) {
    cmd("BrakeRover", #{ target: rover })
}

// ── Scene / world manipulation shortcuts (all generic cmd() under the hood) ──

fn load_scene(path)            { cmd("LoadScene", #{ path: path }) }
fn clear_scene()               { cmd("ClearScene") }
fn set_property(id, key, val)  { cmd("SetObjectProperty", #{ target: id, key: key, value: val }) }

// ── Sensing (reads over get()/query() — gives scripts eyes, not just hands) ──
//
// get() now returns vectors as [x,y,z], so component reads feed the vector math
// above directly. query() reaches the spatial providers (avian-backed Raycast /
// GroundHeight live in lunco-mobility; Nearest / EntitiesInRadius in lunco-api).

/// Linear velocity [vx,vy,vz] of an entity (Avian `LinearVelocity`), or () if
/// the entity has no velocity component.
fn velocity(id) { get(id, "LinearVelocity") }

/// Speed in m/s (0.0 if no velocity).
fn speed(id) { let v = velocity(id); if v == () { 0.0 } else { vlen(v) } }

/// Cast a world-space ray. Returns #{hit, entity, distance, point, normal};
/// `hit` is false when nothing is struck.
fn raycast(origin, dir, max) { query("Raycast", #{ origin: origin, dir: dir, max: max }) }

/// True if a collider is within `dist` straight ahead of `entity` — the basic
/// obstacle-avoidance probe.
fn obstacle_ahead(entity, dist) {
    let p = world_pos(entity);
    let f = world_forward(entity);
    if p == () || f == () { return false; }
    let r = raycast(p, f, dist);
    if r == () { return false; }
    r.hit == true
}

/// Ground/terrain height (world Y) under a world (x,z), or () if nothing below.
fn ground_height(x, z) {
    let r = query("GroundHeight", #{ x: x, z: z });
    if r == () { return (); }
    r.height
}

/// Nearest registered entity id to a world point, or () if none. Pass `me` as
/// `self_id` to exclude yourself.
fn nearest(point, self_id) {
    let r = query("Nearest", #{ point: point, exclude: self_id });
    if r == () || r.id == () { return (); }
    r.id
}

/// Ids of every registered entity within `radius` of a world point.
fn entities_in_radius(point, radius) {
    let r = query("EntitiesInRadius", #{ point: point, radius: radius });
    if r == () { return []; }
    r.ids
}

// ── Collision / trigger events (react in on_event, no polling) ───────────────
//
// The mobility crate mirrors avian `CollisionStart`/`CollisionEnd` onto the
// telemetry bus as `COLLISION_START` / `COLLISION_END` events whose value is the
// gid pair "a:b" (a = avian event target, 0 = unregistered collider). They land
// in `on_event(self, evt)` one tick after contact (same actor-model delay as
// emit). avian only fires for entities with collision events enabled — trigger
// volumes (Sensors) are auto-armed; solid bodies opt in via CollisionEventsEnabled.
//
//   fn on_event(self, evt) {
//       let other = collision_other(evt, self);
//       if other != () { /* touched `other` — react */ }
//   }

/// `[gidA, gidB]` of a collision `evt` (0 = unregistered side), or () if `evt`
/// isn't a COLLISION_START/COLLISION_END pulse.
fn collision_pair(evt) {
    if evt.name != "COLLISION_START" && evt.name != "COLLISION_END" { return (); }
    let parts = evt.value.split(":");
    if parts.len() < 2 { return (); }
    [parse_int(parts[0]), parse_int(parts[1])]
}

/// The OTHER entity in a collision `evt` involving `self_id`, or () if `evt`
/// isn't a collision touching `self_id`. The basic contact probe. A return of
/// `0` means you touched an unregistered collider (e.g. raw terrain).
fn collision_other(evt, self_id) {
    let p = collision_pair(evt);
    if p == () { return (); }
    if p[0] == self_id { p[1] }
    else if p[1] == self_id { p[0] }
    else { () }
}

/// True if `evt` is a contact START touching `self_id` (entered a zone / hit
/// something). Use inside on_event.
fn entered(evt, self_id) {
    evt.name == "COLLISION_START" && collision_other(evt, self_id) != ()
}

/// True if `evt` is a contact END touching `self_id` (left a zone / separated).
fn exited(evt, self_id) {
    evt.name == "COLLISION_END" && collision_other(evt, self_id) != ()
}

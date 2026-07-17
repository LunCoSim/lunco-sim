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
///
/// Casts a ray against COLLIDERS, so it only answers where physics has actually
/// streamed in. On a scene with a collider ring (colliders exist only around the
/// rovers) a point a few hundred metres away has nothing to hit and this returns ().
/// For placing things on terrain, prefer `terrain_height` below.
fn ground_height(x, z) {
    let r = query("GroundHeight", #{ x: x, z: z });
    if r == () { return (); }
    r.height
}

/// Terrain height (world Y) under a world (x,z) from the DEM oracle, or () if the
/// point is outside the height field.
///
/// **The one to use for placing anything on the surface.** It reads the retained
/// height grid directly, so — unlike `ground_height` — it needs no collider, works
/// before physics streams in, works headless, and is deterministic across peers.
///
/// Use it instead of authoring a literal Y. Terrain heights are ABSOLUTE body-datum
/// metres (a real DEM site sits at y ≈ -1900, not ≈ 0), and `distance`/`arrived` are
/// 3-D — so a hardcoded `y = 1.0` is not a small error, it is a target ~1.9 km
/// overhead that no rover can ever "arrive" at. Sample lazily (in the per-tick
/// closure), not once at load: the terrain may not be resident yet at on_start.
fn terrain_height(x, z) {
    let r = query("TerrainHeight", #{ x: x, z: z });
    if r == () || r.found != true { return (); }
    r.height
}

/// A ground point [x, y, z] on the terrain surface under (x, z), or () off-field.
/// The 3-D target `nav_to`/`arrived` want, built from the oracle rather than a guess.
fn on_ground(x, z) {
    let y = terrain_height(x, z);
    if y == () { return (); }
    [x, y, z]
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

// Named trigger volumes — a Sensor authored with a Name fires, IN ADDITION to
// the generic COLLISION_START/END, an `enter:<name>` / `exit:<name>` pulse whose
// value is the ENTERING entity's gid. So a scenario reacts to a zone by name,
// not by reverse-mapping gids:
//
//   fn on_event(self, evt) {
//       if entered_zone(evt, "pad_2") { possess(evt.value); }   // evt.value = who entered
//       let z = zone_of(evt);                                   // or branch on the name
//   }

/// The trigger-zone name an `enter:`/`exit:` pulse carries, or () if `evt` isn't
/// a zone pulse. `enter:pad_2` → "pad_2".
fn zone_of(evt) {
    if evt.name.starts_with("enter:") { evt.name.sub_string(6) }
    else if evt.name.starts_with("exit:") { evt.name.sub_string(5) }
    else { () }
}

/// True if `evt` is an ENTER pulse for trigger zone `name` (something entered it;
/// `evt.value` is the entrant's gid).
fn entered_zone(evt, name) { evt.name == "enter:" + name }

/// True if `evt` is an EXIT pulse for trigger zone `name` (`evt.value` = the gid
/// that left).
fn exited_zone(evt, name) { evt.name == "exit:" + name }


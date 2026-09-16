// LunCo scripting prelude — ergonomic wrappers over the three host verbs.
//
// The host (Rust) exposes only the minimal, generic bridge:
//   cmd(name, #{params})          write: fire any #[Command] by name; returns
//                                        #{ id, ok, status, data?, error? } —
//                                        `status` is applied/rejected/failed/
//                                        pending; poll command_result(id) when
//                                        the command is pending. `data` is the
//                                        handler's result data (spawned gid, …),
//                                        so you can create-then-manipulate
//   query(name, #{params})        read:  call any ApiQueryProvider (Raycast,
//                                        Nearest, GroundHeight, …) — the read
//                                        twin of cmd(); physics stays in its
//                                        owning crate, not in scripting
//   world_pos(id) -> [x,y,z]      read:  float-origin-correct vector; use
//                                        world_point(id) at position boundaries
//   world_pos3(id) -> Vec3        read:  native glam vector for hot loops;
//                                        lower with vec3_array at wire boundaries
//   world_point(id) -> point3     read:  position tagged `active_physics`
//   world_forward(id) -> [x,y,z]  read:  world heading
//   world_forward3(id) -> Vec3    read:  native glam heading
//   world_rotation_quat(id) -> Quat read: native glam orientation
//   get(id, "Comp.field")         read:  reflected field; vectors/quats come
//                                        back as arrays ([x,y,z]/[x,y,z,w]),
//                                        structs as maps
//   list_entities() -> [#{id,name,type,pos,catalog_id,input_surface,...}]  (catalog_id is authored identity)
//   find(name) -> id              lookup: id of the entity named `name` (-1 if none)
//   find_catalog(id) -> id        lookup: first entity spawned from catalog id, or -1
//   name(id) -> string            presentation label for an entity, or ()
//   usd_path(id) -> string        canonical USD path from QueryEntity, or ()
//   parent(id) -> id              tree:   parent's id, or () if none/unregistered
//   children(id) -> [id, ...]     tree:   ids of direct registered children
//   sim_tick() -> i64             time:  current fixed tick (integer)
//   dt() -> f64                   time:  fixed-step delta in seconds (1/60)
//   elapsed_seconds() -> f64      time:  admitted sim seconds derived from SimTick
//   clock_snapshot() -> map       time:  fixed/virtual/physics/mission clocks plus
//                                        physics_deterministic admission status
//   emit(name, value)             event: fire a TelemetryEvent
//   input_key/input_click/...     UI automation: enqueue native Bevy window
//                                        input; compose gestures in Rhai
//   on_event(me, evt, ctx)        event: receives emit()s AND physics
//                                        COLLISION_START / COLLISION_END pulses
//
// Everything below is POLICY, authored here (hot-reloadable), never compiled
// into core. Add helpers freely — no Rust rebuild required.
//
// Position policy: bare [x,y,z] values are frame-free vectors. Use point3(),
// world_point(), and pointer_point() when a value crosses a scene, USD, or
// authoring boundary; those helpers keep the coordinate frame explicit.
// Pointer policy: scene tools consume pointer_intent(context, "name") from the
// shared settings-derived semantic surface; they do not inspect raw buttons or
// modifier flags to define behavior.

/// Resolve an entity's canonical USD topology address.
///
/// `name(id)` is deliberately a presentation label and may come from a
/// display-name override. Topology composition must use the identity returned
/// by the existing QueryEntity provider instead of reconstructing a path from
/// UI text.
fn usd_path(id) {
    let entity = query("QueryEntity", #{ id: id });
    if entity == () || entity.usd_prim_path == () || entity.usd_prim_path == "" {
        return ();
    }
    entity.usd_prim_path
}

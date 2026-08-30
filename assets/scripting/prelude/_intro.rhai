// LunCo scripting prelude — ergonomic wrappers over the three host verbs.
//
// The host (Rust) exposes only the minimal, generic bridge:
//   cmd(name, #{params})          write: fire any #[Command] by name; returns
//                                        #{ id, ok, data, error } — `data` is the
//                                        handler's result data (spawned gid, …),
//                                        so you can create-then-manipulate
//   query(name, #{params})        read:  call any ApiQueryProvider (Raycast,
//                                        Nearest, GroundHeight, …) — the read
//                                        twin of cmd(); physics stays in its
//                                        owning crate, not in scripting
//   world_pos(id) -> [x,y,z]      read:  float-origin-correct world position
//   world_forward(id) -> [x,y,z]  read:  world heading
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
//   elapsed_seconds() -> f64      time:  monotonic sim seconds since startup
//   emit(name, value)             event: fire a TelemetryEvent
//   on_event(me, evt)             event: receives emit()s AND physics
//                                        COLLISION_START / COLLISION_END pulses
//
// Everything below is POLICY, authored here (hot-reloadable), never compiled
// into core. Add helpers freely — no Rust rebuild required.

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

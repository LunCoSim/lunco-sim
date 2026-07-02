// LunCo scripting prelude — ergonomic wrappers over the three host verbs.
//
// The host (Rust) exposes only the minimal, generic bridge:
//   cmd(name, #{params})          write: fire any #[Command] by name; returns
//                                        #{ id, ok, data, error } — `data` is the
//                                        handler's assigned bag (spawned gid, …),
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
//   list_entities() -> [#{id,name,type,pos}]  (type+pos enable in-rhai selection)
//   find(name) -> id              lookup: id of the entity named `name` (-1 if none)
//   name(id) -> string            lookup: reverse of find — id back to its Name, or ()
//   parent(id) -> id              tree:   parent's id, or () if none/unregistered
//   children(id) -> [id, ...]     tree:   ids of direct registered children
//   sim_tick() -> i64             time:  current fixed tick (integer)
//   dt() -> f64                   time:  fixed-step delta in seconds (1/60)
//   elapsed_seconds() -> f64      time:  monotonic sim seconds since startup
//   emit(name, value)             event: fire a TelemetryEvent
//   on_event(self, evt)           event: receives emit()s AND physics
//                                        COLLISION_START / COLLISION_END pulses
//
// Everything below is POLICY, authored here (hot-reloadable), never compiled
// into core. Add helpers freely — no Rust rebuild required.


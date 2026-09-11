// ── Science instruments ── the Rhai authoring surface for tool calls. The Rust
//    core owns firing and cleanup through the generic `lunco-tools` registry;
//    these helpers name a tool from authored policy without managing its
//    lifecycle.
//
// `take_photo` fires the registered `science::take_photo` tool, which captures
// a frame from the vessel's mounted camera (see lunco-avatar/src/science.rs).
// Reachable three ways — direct, from a route event, or as an ad-hoc
// `run_tool` leaf:
//
//   photo_from(rover)                                  // capture from `rover`
//   on_event(me, evt) { if evt.name == "route_point_reached" { take_photo(); } }
//   run_tool("science::take_photo")                    // ad-hoc leaf

/// Capture a frame from `vessel`'s mounted camera (or the active scene camera
/// when the vessel has none). Thin wrapper over the typed `CaptureFromCamera`
/// command — the same verb a UI button or HTTP call uses (§4.2).
/// `target` is an `Option<Entity>`, so it goes on the wire as the tagged enum
/// `#{ Some: <gid> }` — a bare id is not an object, so the executor never
/// descends into the enum and silently drops the whole command.
fn photo_from(vessel) { cmd("CaptureFromCamera", #{ target: #{ Some: vessel } }); }

/// `take_photo()` → a `run_tool` action value usable in a task or route policy.
/// It returns declarative action data; the runtime dispatches the registered
/// tool and owns its lifecycle.
fn take_photo() {
    #{ kind: "run_tool", tool: "science::take_photo" }
}

/// `take_photo(args)` → the same action with opaque args forwarded to the
/// handler (e.g. `take_photo(#{exposure: 0.5})` — the handler parses them).
fn take_photo(args) {
    #{ kind: "run_tool", tool: "science::take_photo", args: to_json(args) }
}

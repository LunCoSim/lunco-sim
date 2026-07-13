// ── Science instruments ── the rhai authoring surface for behaviour-tree
//    tool calls. The Rust core owns firing & cleaning (the `lunco-tools`
//    registry + the `run_tool` leaf → `ToolFired` → handler); these helpers
//    let a mission NAME a tool from data, not manage its lifecycle.
//
// `take_photo` fires the registered `science::take_photo` tool, which captures
// a frame from the vessel's mounted camera (see lunco-avatar/src/science.rs).
// Reachable three ways — direct, as a patrol waypoint arrival action, or as an
// ad-hoc `run_tool` leaf:
//
//   photo_from(rover)                                  // capture from `rover`
//   patrol(rover, [#{pos:[10,0,0], on_arrival:[take_photo()]}])  // waypoint action
//   run_tool("science::take_photo")                    // ad-hoc leaf

/// Capture a frame from `vessel`'s mounted camera (or the active scene camera
/// when the vessel has none). Thin wrapper over the typed `CaptureFromCamera`
/// command — the same verb a UI button or HTTP call uses (§4.2).
/// `target` is an `Option<Entity>`, so it goes on the wire as the tagged enum
/// `#{ Some: <gid> }` — a bare id is not an object, so the executor never
/// descends into the enum and silently drops the whole command.
fn photo_from(vessel) { cmd("CaptureFromCamera", #{ target: #{ Some: vessel } }); }

/// `take_photo()` → a `run_tool` action value usable in a waypoint's
/// `on_arrival` list. Returns the map shape the patrol builder injects; a
/// mission writes `on_arrival: [take_photo()]` (declarative data, not a tree).
fn take_photo() {
    #{ kind: "run_tool", tool: "science::take_photo" }
}

/// `take_photo(args)` → the same action with opaque args forwarded to the
/// handler (e.g. `take_photo(#{exposure: 0.5})` — the handler parses them).
fn take_photo(args) {
    #{ kind: "run_tool", tool: "science::take_photo", args: to_json(args) }
}

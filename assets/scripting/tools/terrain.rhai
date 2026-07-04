// Terrain editing tools — dig, raise, and flatten the ground from any Rhai
// scenario or hook. Each mirrors a typed command via the `cmd()` bus, so the
// same edit is reachable from UI, HTTP API, MCP, and scripts. Edits append a
// non-destructive layer to the terrain and re-bake off-thread, landing in BOTH
// the visual tiles and the physics collider — the rover drives what you see.
//
// A tool library binds as a static-module NAMESPACE — call `terrain::fn(...)`
// directly, from anywhere including inside `on_tick`. NO `import` statement.
//
// Coordinates are terrain-local metres (origin at the terrain centre).
//
//   fn on_start(me) {
//       terrain::dig(0.0, 0.0, 8.0, 2.0);        // 8 m pit, 2 m deep at origin
//       terrain::flatten(30.0, 0.0, 12.0, 0.0);  // level a 12 m pad to y = 0
//   }

/// Dig a smooth round pit: `depth` metres deep at the centre, radius `r`,
/// blending to the surrounding ground at the edge.
fn dig(x, z, r, depth) {
    cmd("BrushTerrain", #{x: x, z: z, radius: r, amplitude: -depth});
}

/// Raise a smooth round mound / berm: `height` metres at the centre, radius `r`.
fn raise(x, z, r, height) {
    cmd("BrushTerrain", #{x: x, z: z, radius: r, amplitude: height});
}

/// Level a landing pad: flatten the ground toward height `target_y` within
/// radius `r`, blending back to the existing terrain at the edge.
fn flatten(x, z, r, target_y) {
    cmd("FlattenTerrain", #{x: x, z: z, radius: r, target_y: target_y});
}

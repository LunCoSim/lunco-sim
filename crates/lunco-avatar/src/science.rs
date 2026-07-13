//! Science instrument tools — registered as closure-defined tools so a
//! behaviour tree's `run_tool` leaf can fire them.
//!
//! Each instrument is a [`ClosureTool`](lunco_tools_bevy::ClosureTool): the
//! closure IS the tool definition, and it triggers its typed command **directly**
//! via `ctx.world().trigger(...)` — no JSON, no reflection. Adding a new
//! instrument is one closure; no per-instrument Rust struct boilerplate.
//!
//! The `run_tool` leaf → `ToolFired` → `lunco-tools-bevy` dispatch path reaches
//! these closures; the rhai `science.rhai` prelude only NAMES them (e.g.
//! `take_photo()` returns a `run_tool` action value for a waypoint's
//! `on_arrival` list). Core owns firing & cleaning; rhai just names the tool.

use crate::CaptureFromCamera;
use lunco_tools_bevy::{ToolResult, register_closure_tool};

/// Register the avatar-crate's science instrument tools into the global
/// [`lunco_tools`] registry. Idempotent (re-register replaces). Call from
/// `AvatarPlugin::build` (or any plugin whose host wants these tools).
pub fn register_science_tools() {
    // `science::take_photo` — capture a frame from the firing vessel's mounted
    // camera. Triggers the typed `CaptureFromCamera` command directly; the
    // command's observer resolves the vessel's `Camera3d` descendant.
    register_closure_tool(
        "science::take_photo",
        vec!["take_photo/0".into()],
        |world, vessel, _gid, _args| {
            // Trigger the typed command directly — no JSON, no reflection.
            // The command's observer resolves the vessel's `Camera3d`
            // descendant and captures from the window it renders to. If the
            // vessel has no camera, the observer no-ops with a warn (B3 fix).
            world.trigger(CaptureFromCamera { target: Some(vessel) });
            ToolResult::Ok
        },
    );
}

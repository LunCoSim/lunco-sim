//! Shared setup for the `lunco-usd-avian` integration tests.
//!
//! Lives in a `tests/support/` subdirectory (not a file directly under `tests/`)
//! so Cargo treats it as a shared module — `mod support;` from a test file — rather
//! than compiling it as its own test binary.
//!
//! `dead_code`/`unreachable_pub` are allowed because each test binary that does
//! `mod support;` compiles the whole module but may use only part of it, and its
//! `pub` items have no external crate to be reachable from.
#![allow(dead_code, unreachable_pub)]

use avian3d::prelude::*;
use bevy::prelude::*;

/// A headless [`App`] wired to STEP Avian physics in a test: `MinimalPlugins` +
/// `AssetPlugin` + the `Mesh` asset + `PhysicsPlugins`.
///
/// The `AssetPlugin` + `Mesh` pair is the non-obvious, easy-to-miss part. Avian's
/// collider cache runs a system with a `MessageReader<AssetEvent<Mesh>>` param, and
/// under Bevy 0.18 a system whose message resource was never initialised PANICS via
/// the default error handler ("Message not initialized") on the first step. Build a
/// headless physics app on bare `MinimalPlugins` and it blows up with an error that
/// reads like a physics crash but is really a missing-plugin harness bug. This is
/// the one place that knowledge is encoded, so no test re-derives it the hard way.
///
/// `TransformPlugin` is deliberately NOT added — a plain physics test wants it, but
/// a `big_space` test drives its own propagation and must not double it. Add it (or
/// any other plugins) on the returned app.
///
/// The app is NOT finished: add extra plugins, spawn your scene, then call
/// `app.finish(); app.cleanup();` before stepping — Avian registers its own
/// messages/types in that deferred plugin setup.
pub fn headless_physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), PhysicsPlugins::default()));
    app.init_asset::<Mesh>();
    app
}

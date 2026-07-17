//! Program drivers — the Rust half of `LunCoProgram`.
//!
//! A `LunCoProgram` prim names its implementation one of three ways, exactly as
//! `UsdShade.Shader` does: `lunco:program:sourceAsset` (a file — the engine comes
//! from its extension), `lunco:program:sourceCode` (text in place), or
//! **`lunco:program:id`** — a name the runtime already implements, resolved here.
//! That third arm is `info:id = "UsdPreviewSurface"`: a thing every renderer is
//! expected to have, named rather than shipped.
//!
//! USD **selects**; it does not define. A crate registers a driver under a name; a
//! scene picks it and parameterises it with authored attributes. Adding a
//! visualization is registering a name — not editing a schema, and not extending a
//! central `match` that every new behaviour has to touch.
//!
//! See `docs/architecture/50-usd-driven-visuals.md`.
//!
//! ## Why this is not `ControlKernelRegistry`
//!
//! [`crate::kernels::ControlKernelRegistry`] maps a name to a `fn` pointer because a
//! `ControlKernel` is **pure** — inputs and params in, port writes out, no world
//! access. A program driver is not: it reads a sensor and writes a `Transform`, so a
//! `fn` pointer would have to take `&mut World` and give up parallelism for every
//! driven prim.
//!
//! So a driver is an ordinary Bevy **system**, registered through
//! [`ProgramDriverAppExt::register_program_driver`] (the shape
//! [`crate::telemetry::ScriptEventAppExt::project_events`] already uses), and this
//! registry holds only the *names* — enough to warn about an id nothing implements.
//! Same contract as the kernels, different storage, because the shape of the work
//! differs.
//!
//! Cross-reference `TODO(behaviour-registry)` in [`crate::kernels`]: a driver is
//! plausibly one of the "kinds" that eventually folds into one behaviour system.

use bevy::prelude::*;
use std::collections::HashSet;

/// The program id authored on a `LunCoProgram` prim (`lunco:program:id`), stamped on
/// the prim that **owns** the program — not on the program prim itself.
///
/// Owner, because that is what the program drives: `me` is the Cone, and the program
/// prim carries only the binding and its parameters. The rhai path stamps
/// `EmbeddedScenarioSource`/`ScriptParams` on the owner for the same reason.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct ProgramDriverId(pub String);

/// The names that have a Rust driver behind them.
///
/// Only names. The drivers themselves are systems — this exists so an id nothing
/// implements can be *reported* rather than silently doing nothing.
#[derive(Resource, Default, Clone)]
pub struct ProgramDriverRegistry {
    known: HashSet<String>,
}

impl ProgramDriverRegistry {
    /// Whether a driver is registered under `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.known.contains(id)
    }

    /// Every registered driver name — for diagnostics and tooling.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.known.iter().map(String::as_str)
    }
}

/// Register a Rust driver for a `lunco:program:id`.
pub trait ProgramDriverAppExt {
    /// Bind `id` to `system`, which runs every frame for the prims that select it.
    ///
    /// The system is an ordinary Bevy system: filter it on [`ProgramDriverId`]
    /// yourself, so it stays parallel and needs no exclusive world access.
    fn register_program_driver<M>(
        &mut self,
        id: &str,
        system: impl IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
    ) -> &mut Self;
}

impl ProgramDriverAppExt for App {
    fn register_program_driver<M>(
        &mut self,
        id: &str,
        system: impl IntoScheduleConfigs<bevy::ecs::system::ScheduleSystem, M>,
    ) -> &mut Self {
        // First driver in this app brings the reporter with it. Registering it here
        // rather than in a plugin is what keeps a driverless build SILENT: the
        // `--no-ui` server projects the same USD and stamps the same ids, but draws
        // nothing and registers nothing, so an unresolved id there is correct rather
        // than worth reporting.
        if !self.world().contains_resource::<ProgramDriverRegistry>() {
            self.init_resource::<ProgramDriverRegistry>();
            self.add_systems(Update, warn_unknown_program_drivers);
        }
        self.world_mut()
            .resource_mut::<ProgramDriverRegistry>()
            .known
            .insert(id.to_string());
        self.add_systems(Update, system)
    }
}

/// Report a `lunco:program:id` that nothing implements.
///
/// **A no-op with a warning, never a panic.** A scene authored against a newer
/// runtime — or against a driver in a crate this binary did not link — must still
/// open. Deduped per id, because the id does not change. Same contract as an unknown
/// drive kernel in `lunco-mobility`.
///
/// Installed by the first [`ProgramDriverAppExt::register_program_driver`], so an app
/// that registers NO drivers never runs it. That is deliberate: the `--no-ui` server
/// projects the same USD and stamps the same ids, but has no render crate and no
/// business drawing a beam — an unresolved id there is the design, not a defect.
fn warn_unknown_program_drivers(
    q: Query<(Entity, &ProgramDriverId), Added<ProgramDriverId>>,
    registry: Res<ProgramDriverRegistry>,
    mut warned: Local<HashSet<String>>,
) {
    for (entity, id) in q.iter() {
        if !registry.contains(&id.0) && warned.insert(id.0.clone()) {
            let names: Vec<&str> = registry.names().collect();
            warn!(
                "unknown lunco:program:id {:?} on {:?} — no Rust driver is registered \
                 under that name; the prim is not driven. Registered: {:?}",
                id.0, entity, names
            );
        }
    }
}

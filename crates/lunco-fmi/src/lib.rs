//! External FMI 3 Co-Simulation participants. Native instances stay on one
//! worker thread; only immutable requests and completed samples cross to ECS.

#[cfg(not(target_arch = "wasm32"))]
mod metadata;
#[cfg(not(target_arch = "wasm32"))]
mod session;

#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{FmiCompletionSet, FmiPlugin, FmuLimits, FmuParticipant, LoadFmu, SetFmuPaused};

/// Native FMU binaries cannot be dynamically loaded by a browser host.
#[cfg(target_arch = "wasm32")]
pub struct FmiPlugin;
#[cfg(target_arch = "wasm32")]
impl bevy::prelude::Plugin for FmiPlugin {
    fn build(&self, _: &mut bevy::prelude::App) {}
}

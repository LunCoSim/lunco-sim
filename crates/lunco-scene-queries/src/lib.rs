//! Read-only scene providers shared by every runtime host.
//!
//! `QueryEntity` reports the active physics-frame identity and pose, while
//! `QueryUsdPrim` reads composed USD facts and optional runtime topology. The
//! providers are intentionally separate from scene mutation commands: query
//! implementation changes then rebuild this package and its consumers without
//! recompiling the command-handler implementation.

pub mod entity_query;
pub mod usd_prim_query;

use bevy::prelude::*;

/// Installs the read-only scene query providers.
pub struct SceneQueryPlugin;

impl Plugin for SceneQueryPlugin {
    fn build(&self, app: &mut App) {
        entity_query::register(app);
        usd_prim_query::register(app);
    }
}

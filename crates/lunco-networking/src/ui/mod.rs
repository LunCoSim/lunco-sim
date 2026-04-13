//! LunCoSim Networking UI Plugin (Layer 4).
//!
//! Provides UI panels for network status, connection management,
//! authority negotiation, and peer list viewing.
//!
//! This is a **Layer 4** plugin — optional, separate from domain logic.
//! The simulation runs perfectly fine without it.

use bevy::prelude::*;

/// UI plugin for networking panels and debug views.
pub struct LunCoNetworkingUiPlugin;

impl Plugin for LunCoNetworkingUiPlugin {
    fn build(&self, _app: &mut App) {
        // TODO: Register panels with bevy_workbench
        // - Connection status panel
        // - Authority panel (request/release control)
        // - Peer list viewer
        // - Interest debug visualizer
    }
}

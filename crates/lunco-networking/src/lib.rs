//! LunCoSim Networking Plugin.
//!
//! Transparent networking layer that provides:
//! - **Transport abstraction** (UDP / WebSocket / WebTransport) with compile-time selection
//! - **ECS replication** via `bevy_replicon` + `bevy_renet2`
//! - **Space-standards compatibility** (CCSDS, XTCE, YAMCS bridge)
//! - **Seamless compression** (quantization, delta encoding, LZ4)
//!
//! Domain crates (`lunco-mobility`, `lunco-celestial`, etc.) never import this crate.
//! They declare `app.replicate::<MyComponent>()` and the networking layer handles
//! wire format, compression, and protocol translation transparently.

use bevy::prelude::*;

/// Plugin that registers networking infrastructure.
///
/// Added to the simulation alongside domain plugins.
/// Transport is selected at compile time via Cargo features.
pub struct LunCoNetworkingPlugin;

impl Plugin for LunCoNetworkingPlugin {
    fn build(&self, _app: &mut App) {
        // TODO: Register transport, replicon, entity ID mapping, compression, etc.
        // See README.md for full architecture.
    }
}

//! LunCoSim Networking UI bridge (Layer 4).
//!
//! The **Connect** controls themselves live in the workbench's top menu bar
//! (the *Network* menu) — drawn with no lunco-networking dependency, off the
//! always-on [`lunco_core::NetStatus`] seam. This plugin is the thin adapter
//! that closes the loop:
//!
//! - **seeds** [`NetStatus::connect_hint`] with [`crate::default_connect_host`]
//!   (page origin on wasm, localhost on native) so the menu's address field has
//!   a sensible default;
//! - **observes** the menu's [`NetConnectRequest`] / [`NetDisconnectRequest`]
//!   bridge events and re-dispatches the typed
//!   [`JoinServer`](crate::client::JoinServer) /
//!   [`LeaveServer`](crate::client::LeaveServer) commands — the **same** commands
//!   the HTTP API, MCP, and CLI dispatch.
//!
//! Layer 4: optional. Headless builds simply never add this plugin; the menu's
//! bridge events then go unobserved (no-op) and the sim runs single-player.

use bevy::prelude::*;
use lunco_core::{NetConnectRequest, NetDisconnectRequest, NetStatus};

use crate::client::{JoinServer, LeaveServer};

/// Wires the Network-menu bridge: seeds the connect hint and forwards the menu's
/// connect/disconnect requests to the typed networking commands.
pub struct LunCoNetworkingUiPlugin;

impl Plugin for LunCoNetworkingUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, seed_connect_hint)
            .add_observer(on_net_connect_request)
            .add_observer(on_net_disconnect_request);
    }
}

/// Pre-fill the Connect field's suggested address (once, if not already set).
fn seed_connect_hint(mut status: ResMut<NetStatus>) {
    if status.connect_hint.is_empty() {
        status.connect_hint = crate::default_connect_host();
    }
}

/// Menu *Connect* → dispatch the typed [`JoinServer`] command.
fn on_net_connect_request(trigger: On<NetConnectRequest>, mut commands: Commands) {
    let address = trigger.address.trim().to_string();
    if address.is_empty() {
        return;
    }
    commands.trigger(JoinServer { address });
}

/// Menu *Disconnect* → dispatch the typed [`LeaveServer`] command.
fn on_net_disconnect_request(_trigger: On<NetDisconnectRequest>, mut commands: Commands) {
    commands.trigger(LeaveServer {});
}

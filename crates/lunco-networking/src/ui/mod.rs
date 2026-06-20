//! LunCoSim Networking UI Plugin (Layer 4).
//!
//! A small in-sim **Connect** panel: when local, an address field (defaulting to
//! the page origin via [`crate::default_connect_host`]) + a Connect button; when
//! connected, a Disconnect button. The buttons dispatch the typed
//! [`JoinServer`](crate::client::JoinServer) / [`LeaveServer`](crate::client::LeaveServer)
//! commands — the **same** commands the HTTP API, MCP, and CLI dispatch — so the
//! UI never touches the connection directly; the networking internals do.
//!
//! Layer 4: optional, separate from domain logic. The sim runs fine without it.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use lunco_core::{NetStatus, NetworkRole};

use crate::client::{JoinServer, LeaveServer};

/// Adds the Connect panel to the egui pass. Requires `EguiPlugin` (the sandbox
/// gets it via the workbench); headless builds simply never add this plugin.
pub struct LunCoNetworkingUiPlugin;

impl Plugin for LunCoNetworkingUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, render_connect_panel);
    }
}

/// Draw the Connect/Disconnect panel and dispatch `JoinServer`/`LeaveServer`.
fn render_connect_panel(
    mut egui_ctx: EguiContexts,
    status: Option<Res<NetStatus>>,
    mut commands: Commands,
    // The editable address; seeded once from the page origin / localhost.
    mut field: Local<Option<String>>,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else {
        return;
    };
    let Some(status) = status else {
        return;
    };

    let address = field.get_or_insert_with(crate::default_connect_host);

    egui::Window::new("🌐 Network")
        .resizable(false)
        .collapsible(true)
        .default_open(false)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 48.0))
        .show(ctx, |ui| match status.role {
            NetworkRole::Host => {
                ui.label(format!("Hosting · {}", status.endpoint));
            }
            NetworkRole::Client => {
                let state = if status.connected {
                    "Connected"
                } else {
                    "Connecting…"
                };
                ui.label(format!("{state} → {}", status.endpoint));
                if ui.button("Disconnect").clicked() {
                    commands.trigger(LeaveServer {});
                }
            }
            NetworkRole::Standalone => {
                ui.label("Single-player (local)");
                ui.horizontal(|ui| {
                    ui.label("Server:");
                    ui.text_edit_singleline(address);
                });
                let enabled = !address.trim().is_empty();
                if ui
                    .add_enabled(enabled, egui::Button::new("Connect"))
                    .clicked()
                {
                    commands.trigger(JoinServer {
                        address: address.clone(),
                    });
                }
            }
        });
}

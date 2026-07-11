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
use bevy_egui::{egui, EguiContexts};
use lunco_core::{LocalSession, NetConnectRequest, NetDisconnectRequest, NetStatus};
use lunco_doc_bevy::Presence;

use crate::client::{JoinServer, LeaveServer};

/// Wires the Network-menu bridge: seeds the connect hint and forwards the menu's
/// connect/disconnect requests to the typed networking commands.
pub struct LunCoNetworkingUiPlugin;

impl Plugin for LunCoNetworkingUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, seed_connect_hint)
            .add_observer(on_net_connect_request)
            .add_observer(on_net_disconnect_request)
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (draw_collaborator_cursors, draw_pending_connect_prompt),
            );

        #[cfg(feature = "workbench")]
        app.add_systems(Startup, register_settings_menu);
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
    commands.trigger(JoinServer {
        address,
        digest: trigger.digest.trim().to_string(),
    });
}

/// Menu *Disconnect* → dispatch the typed [`LeaveServer`] command.
fn on_net_disconnect_request(_trigger: On<NetDisconnectRequest>, mut commands: Commands) {
    commands.trigger(LeaveServer {});
}

/// While a deep link is awaiting confirmation ([`PendingConnect`]), draw a modal
/// "Connect to X? [Join] [Cancel]". *Join* dispatches [`JoinServer`] with the
/// link's address + digest; either choice clears the pending request. Gating an
/// unsolicited link behind an explicit click stops a planted `luncosim://` /
/// `?connect=` link from silently redirecting the session.
fn draw_pending_connect_prompt(
    mut contexts: EguiContexts,
    mut pending: ResMut<lunco_core::session::PendingConnect>,
    mut commands: Commands,
) {
    let Some(req) = pending.request.clone() else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let mut decision: Option<bool> = None; // Some(true)=join, Some(false)=cancel
    egui::Window::new("Connect to server?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label("A link is asking to connect to:");
            ui.add_space(4.0);
            ui.strong(&req.address);
            if !req.digest.is_empty() {
                ui.add_space(2.0);
                ui.weak(format!("pinned cert digest: {}…", &req.digest[..req.digest.len().min(16)]));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Join").clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
    match decision {
        Some(true) => {
            commands.trigger(JoinServer {
                address: req.address,
                digest: req.digest,
            });
            pending.request = None;
        }
        Some(false) => pending.request = None,
        None => {}
    }
}

#[cfg(feature = "workbench")]
fn register_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, world| {
        // Read/clone all needed resources up front to avoid borrow checker conflicts on world
        let mut settings = world.resource::<crate::sync::CursorSettings>().clone();
        let presence_users = world.resource::<crate::sync::Presence>().users.clone();
        let tut_settings = world.resource::<crate::sync::TutorialSettings>().clone();
        let tutor_status = world.resource::<crate::sync::TutorStatusResource>().clone();

        ui.label(egui::RichText::new("Presence Cursors").weak().small());
        let mut cursor_settings_changed = false;
        if ui.checkbox(&mut settings.enabled, "Transmit my cursor position")
            .on_hover_text(
                "Share your mouse cursor position with other collaborators in real-time. \
                 Persisted to ~/.lunco/settings.json.",
            )
            .changed()
        {
            cursor_settings_changed = true;
        }

        ui.horizontal(|ui| {
            ui.label("Cursor Color:");
            if ui.color_edit_button_srgb(&mut settings.color).changed() {
                cursor_settings_changed = true;
            }
        });

        if cursor_settings_changed {
            *world.resource_mut::<crate::sync::CursorSettings>() = settings;
        }

        ui.separator();

        ui.label(egui::RichText::new("Tutorial / Teach Mode").weak().small());
        
        let mut teach_mode = tut_settings.teach_mode;
        if ui.checkbox(&mut teach_mode, "🎓 Teach Mode (Broadcast status)")
            .on_hover_text("Take control of the system and stream your window and avatar status to followers.")
            .changed() 
        {
            world.trigger(crate::sync::SetTeachMode { enabled: teach_mode });
        }
        
        if teach_mode {
            ui.indent("tutor_indent", |ui| {
                let current_target = tut_settings.target_client;
                let mut selected_target = current_target;
                
                let combo_label = selected_target
                    .and_then(|id| presence_users.get(&crate::sync::UserId(id)))
                    .map(|u| u.display_name.as_str())
                    .unwrap_or("Everyone");

                let mut changed = false;
                egui::ComboBox::from_label("Target Follower")
                    .selected_text(combo_label)
                    .show_ui(ui, |ui| {
                        if ui.selectable_value(&mut selected_target, None, "Everyone").clicked() {
                            changed = true;
                        }
                        for (&uid, info) in &presence_users {
                            if ui.selectable_value(&mut selected_target, Some(uid.0), &info.display_name).clicked() {
                                changed = true;
                            }
                        }
                    });
                
                if changed {
                    world.trigger(crate::sync::SetTargetClient { target: selected_target });
                }
                
                let mut allow_free = tut_settings.allow_free_movement;
                if ui.checkbox(&mut allow_free, "🔓 Allow followers to move freely")
                    .on_hover_text("If checked, followers can move as they want. Otherwise, they are locked to your perspective.")
                    .changed()
                {
                    world.trigger(crate::sync::SetAllowFreeMovement { enabled: allow_free });
                }
                
                let target_name = selected_target
                    .and_then(|id| presence_users.get(&crate::sync::UserId(id)))
                    .map(|u| u.display_name.as_str())
                    .unwrap_or("Everyone");

                if ui.button(format!("👁 Send 'Look at My View' to {target_name}"))
                    .on_hover_text("Force followers to snap to your current active document and avatar perspective once.")
                    .clicked()
                {
                    world.trigger(crate::sync::SharePerspective {});
                }
                    
                if selected_target.is_some() {
                    let mut observe_mode = tut_settings.observe_mode;
                    if ui.checkbox(&mut observe_mode, "🔍 Observe Target's View (Reverse stream)")
                        .on_hover_text("Observe the target student's screen and position instead of streaming yours.")
                        .changed()
                    {
                        world.trigger(crate::sync::SetObserveMode { enabled: observe_mode });
                    }
                }
            });
        }
        
        // Per-peer opt-in: consent to be locked by a tutor broadcasting to Everyone.
        let mut follow_opt_in = tut_settings.follow_opt_in;
        if ui.checkbox(&mut follow_opt_in, "🤝 Let a tutor lock my view (opt in)")
            .on_hover_text(
                "Allow a tutor broadcasting to 'Everyone' to lock your view and input. \
                 Off by default — you are never force-locked without consent. A tutor \
                 targeting you specifically can still lock you.",
            )
            .changed()
        {
            world.trigger(crate::sync::SetFollowOptIn { enabled: follow_opt_in });
        }

        // The tutor lock applies to me only if I'm explicitly targeted, or it's a
        // broadcast and I opted in. Only then is the manual toggle disabled.
        let local_session = world.resource::<lunco_core::LocalSession>().0 .0;
        let locked_for_me = tutor_lock_active(&tutor_status)
            && (tutor_status.target_client == Some(local_session)
                || (tutor_status.target_client.is_none() && tut_settings.follow_opt_in));

        let mut follow_mode = tut_settings.follow_mode;
        ui.add_enabled_ui(!locked_for_me, |ui| {
            let label = if locked_for_me {
                "📖 Follow Mode (Locked by Tutor)"
            } else {
                "📖 Follow Mode (Mirror tutor)"
            };
            if ui.checkbox(&mut follow_mode, label)
                .on_hover_text("Block local inputs and mirror the tutor's window and avatar status.")
                .changed()
            {
                world.trigger(crate::sync::SetFollowMode { enabled: follow_mode });
            }
        });
    });
}

/// Whether a tutor currently holds the view/input lock (before per-peer targeting).
/// The `tutor_active && !allow_free_movement` core was repeated across the Follow-Mode
/// toggle's `locked_for_me`, the banner label, and the banner's exit gate.
fn tutor_lock_active(t: &crate::sync::TutorStatusResource) -> bool {
    t.tutor_active && !t.allow_free_movement
}

/// Draw one of the centered top-of-screen overlay banners (tutor / student / teaching)
/// — they shared ~30 lines of identical chrome each. `scrim` adds the full-screen dark
/// tint that also blocks clicks behind it (Follow Mode only); `add_contents` fills the
/// inner row (label + optional button).
#[allow(clippy::too_many_arguments)]
fn draw_top_banner(
    ctx: &egui::Context,
    id: &str,
    screen_rect: egui::Rect,
    size: egui::Vec2,
    stroke: egui::Stroke,
    scrim: bool,
    shrink: egui::Vec2,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            if scrim {
                // Block clicks behind by allocating the full screen + a subtle dark tint.
                let (rect, _response) =
                    ui.allocate_at_least(screen_rect.size(), egui::Sense::click_and_drag());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(20));
            }
            let banner_rect = egui::Rect::from_center_size(
                egui::pos2(screen_rect.center().x, screen_rect.min.y + 40.0),
                size,
            );
            ui.painter()
                .rect_filled(banner_rect, 6.0, egui::Color32::from_rgb(30, 30, 46));
            ui.painter()
                .rect_stroke(banner_rect, 6.0, stroke, egui::StrokeKind::Outside);
            let child_rect = banner_rect.shrink2(shrink);
            let mut child_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(child_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child_ui.horizontal(|ui| add_contents(ui));
        });
}

/// Draw collaborator cursors on top of the screen in egui.
pub fn draw_collaborator_cursors(
    mut egui_ctx: EguiContexts,
    presence: Res<Presence>,
    local: Res<LocalSession>,
    tutorial_settings: Res<crate::sync::TutorialSettings>,
    tutor_status: Res<crate::sync::TutorStatusResource>,
    mut commands: Commands,
) {
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };

    let screen_rect = ctx.viewport_rect();

    // 1. Draw Tutorial Mode Overlay if following
    if tutorial_settings.follow_mode {
        let can_exit = !tutor_lock_active(&tutor_status);
        let label = if tutor_lock_active(&tutor_status) {
            "📖 Tutorial Mode (Locked by Tutor)"
        } else {
            "📖 Tutorial Mode (Mirroring)"
        };
        draw_top_banner(
            ctx,
            "tutorial_overlay",
            screen_rect,
            egui::vec2(320.0, 36.0),
            egui::Stroke::new(1.5, egui::Color32::from_rgb(243, 139, 168)), // Peach/Red accent
            true,                       // scrim: block input behind the Follow-Mode lock
            egui::vec2(12.0, 4.0),
            |ui| {
                ui.label(egui::RichText::new(label).color(egui::Color32::WHITE).strong());
                if can_exit {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(egui::RichText::new("Exit").color(egui::Color32::from_rgb(243, 139, 168)).strong())
                            .clicked()
                        {
                            commands.trigger(crate::sync::SetFollowMode { enabled: false });
                        }
                    });
                }
            },
        );
    }

    // 1b. Draw Student Mode indicator when a tutor is active and this client is
    // the targeted student, but we are NOT in follow mode (free movement allowed).
    // Without this, a targeted/observed student has no indication they are the
    // active student. (When follow_mode is on, the "Mirroring" banner above
    // already conveys it, so skip to avoid stacking two banners.)
    let is_targeted = tutor_status.target_client.is_none()
        || tutor_status.target_client == Some(local.0 .0);
    let is_active_student = tutor_status.tutor_active
        && !tutorial_settings.follow_mode
        && is_targeted;
    if is_active_student {
        let label = if tutor_status.observe_mode {
            "👤 Student Mode (Tutor is observing you)"
        } else {
            "👤 Student Mode (Selected by tutor)"
        };
        draw_top_banner(
            ctx,
            "student_overlay",
            screen_rect,
            egui::vec2(260.0, 32.0),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(137, 180, 250)), // Blue accent
            false,
            egui::vec2(10.0, 2.0),
            |ui| {
                ui.label(egui::RichText::new(label).color(egui::Color32::WHITE).small());
            },
        );
    }

    // 2. Draw Tutor Mode Indicator if teaching
    if tutorial_settings.teach_mode {
        draw_top_banner(
            ctx,
            "tutor_overlay",
            screen_rect,
            egui::vec2(245.0, 32.0),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(166, 227, 161)), // Green accent
            false,
            egui::vec2(10.0, 2.0),
            |ui| {
                ui.label(egui::RichText::new("🎓 Teaching Mode (Broadcasting)").color(egui::Color32::WHITE).small());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new("Stop").color(egui::Color32::from_rgb(166, 227, 161)).small())
                        .clicked()
                    {
                        commands.trigger(crate::sync::SetTeachMode { enabled: false });
                    }
                });
            },
        );
    }

    // Foreground painter so it is on top of everything
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("presence_cursors"),
    ));

    for (user_id, info) in &presence.users {
        // Skip drawing the local user's cursor
        if user_id.0 == local.0 .0 {
            continue;
        }

        if let Some(cursor_pos) = info.cursor {
            // Determine a high-contrast text color based on relative luminance of the user color
            let luminance = (0.2126 * info.color[0] as f32
                + 0.7152 * info.color[1] as f32
                + 0.0722 * info.color[2] as f32)
                / 255.0;
            let text_color = if luminance > 0.6 {
                egui::Color32::BLACK
            } else {
                egui::Color32::WHITE
            };

            let font = egui::FontId::proportional(11.0);
            let galley = painter.layout_no_wrap(info.display_name.clone(), font, text_color);
            let size = galley.size();

            // Map absolute pixel coordinates to screen coordinates
            let x = screen_rect.min.x + cursor_pos[0];
            let y = screen_rect.min.y + cursor_pos[1];

            // Clamp coordinates to screen boundaries to keep cursor and name tag visible
            let margin_left = 2.0;
            let margin_top = 2.0;
            let margin_right = 16.0 + size.x + 8.0;
            let margin_bottom = 20.0 + size.y + 4.0;

            let clamped_x = x.clamp(screen_rect.min.x + margin_left, screen_rect.max.x - margin_right);
            let clamped_y = y.clamp(screen_rect.min.y + margin_top, screen_rect.max.y - margin_bottom);
            let pos = egui::pos2(clamped_x, clamped_y);

            let color = egui::Color32::from_rgb(info.color[0], info.color[1], info.color[2]);

            // Draw pointer (cursor arrow) with a black stroke for high-contrast on light background
            let stroke = egui::Stroke::new(1.5, egui::Color32::BLACK);
            let p1 = pos;
            let p2 = pos + egui::vec2(0.0, 16.0);
            let p3 = pos + egui::vec2(4.5, 12.0);
            let p4 = pos + egui::vec2(12.0, 12.0);
            painter.add(egui::Shape::convex_polygon(vec![p1, p2, p3, p4], color, stroke));

            // Draw name tag below and to the right with a thin black border
            let tag_pos = pos + egui::vec2(12.0, 16.0);
            let bg_rect = egui::Rect::from_min_size(tag_pos, size).expand2(egui::vec2(4.0, 2.0));
            painter.rect_filled(bg_rect, 2.0, color);
            painter.rect_stroke(
                bg_rect,
                2.0,
                egui::Stroke::new(1.0, egui::Color32::BLACK),
                egui::StrokeKind::Outside,
            );
            painter.galley(tag_pos, galley, text_color);
        }
    }
}

//! Generic context menus for USD-authored scene markers.
//!
//! Waypoints have a richer route editor in `checkpoint_click`.  This module is
//! the reusable fallback for markers such as a landing target: the visual mesh
//! remains a right-click target even when its left-click policy passes through
//! to a vessel behind it.

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_core::{EguiFocus, PointerInteraction, ScenePointerPolicy, WaypointMenuOpen};
use lunco_usd_bevy::UsdPrimPath;

/// The generic scene marker currently opened by a secondary click.
#[derive(Resource, Default)]
pub struct SceneContextMenuState {
    pub entity: Option<Entity>,
    pub position: Vec2,
}

/// Translate the render-free USD policy into Bevy mesh-picking behavior.
///
/// `should_block_lower = false` is the engine's native click-through behavior:
/// the marker still emits pointer events, but the vessel/rover underneath is
/// also hovered and receives the primary click.  This is intentionally a
/// change-driven system; interaction policy is authored topology, not a
/// per-frame computation.
pub fn apply_pointer_policies(
    mut commands: Commands,
    q_policy: Query<(Entity, &ScenePointerPolicy), Added<ScenePointerPolicy>>,
) {
    for (entity, policy) in q_policy.iter() {
        commands.entity(entity).insert(Pickable {
            should_block_lower: policy.left != PointerInteraction::PassThrough,
            is_hoverable: true,
        });
    }
}

/// Open a generic context menu for any USD prim that declares a right-click
/// context policy, except route waypoints which keep their dedicated editor.
pub fn on_scene_right_click_context(
    mut click: On<Pointer<Click>>,
    egui_focus: Res<EguiFocus>,
    q_policy: Query<&ScenePointerPolicy>,
    q_waypoint: Query<(), With<lunco_usd_sim::marker::WaypointMarker>>,
    q_parents: Query<&ChildOf>,
    mut state: ResMut<SceneContextMenuState>,
) {
    if egui_focus.wants_pointer || click.button != PointerButton::Secondary {
        return;
    }

    let mut entity = click.entity;
    for _ in 0..16 {
        // The route editor owns waypoint context menus and must win over the
        // generic marker menu, even though both assets share the same policy.
        if q_waypoint.get(entity).is_ok() {
            return;
        }
        if q_policy
            .get(entity)
            .is_ok_and(|policy| policy.right == PointerInteraction::Context)
        {
            click.propagate(false);
            state.entity = Some(entity);
            state.position = click.pointer_location.position;
            return;
        }
        let Ok(parent) = q_parents.get(entity) else {
            break;
        };
        entity = parent.parent();
    }
}

/// Draw the generic marker menu in the egui pass.
pub fn draw_scene_context_menu(
    mut contexts: bevy_egui::EguiContexts,
    mut state: ResMut<SceneContextMenuState>,
    mut menu_open: ResMut<WaypointMenuOpen>,
    q_prim: Query<&UsdPrimPath>,
) {
    let Some(entity) = state.entity else {
        return;
    };
    let Ok(prim) = q_prim.get(entity) else {
        state.entity = None;
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    menu_open.0 = true;
    let origin = ctx.content_rect().min.to_vec2();
    let pos = egui::pos2(state.position.x, state.position.y) + origin;
    let mut close = false;
    egui::Area::new(egui::Id::new("scene_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new("Scene marker").strong());
                ui.monospace(prim.path.as_str());
                ui.separator();
                ui.label("Left-click passes through to objects behind it.");
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    if close {
        state.entity = None;
        menu_open.0 = false;
    }
}

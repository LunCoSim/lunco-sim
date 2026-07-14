//! Models palette — click a model to enter "attach mode", then click
//! an entity in the 3D scene to attach the corresponding simulation.
//!
//! ## Scope (Phase 2 of interactive modeling)
//!
//! Each palette item maps to a marker component that existing setup
//! systems (`balloon_setup`, `python_balloon_setup`) already react to.
//! Dropping a marker on a plain `BallDynamic` rigid body triggers the
//! Modelica or Python pipeline.
//!
//! Source files are still hardcoded in those setup systems (`balloon.mo`,
//! `green_balloon.py`) — "load any .mo/.py" is Phase 2b.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_sandbox_edit::catalog::{BalloonModelMarker, PythonBalloonMarker};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

/// Which model the user has selected to attach next.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachState {
    /// No attachment pending — clicks in the 3D scene behave normally.
    #[default]
    Idle,
    /// User picked a model from the palette; next click on an entity in
    /// 3D attaches the matching marker.
    Pending(PendingAttachment),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingAttachment {
    ModelicaBalloon,
    PythonBalloon,
}

impl PendingAttachment {
    fn title(&self) -> &'static str {
        match self {
            Self::ModelicaBalloon => "balloon.mo",
            Self::PythonBalloon => "green_balloon.py",
        }
    }

    fn language_label(&self) -> &'static str {
        match self {
            Self::ModelicaBalloon => "Modelica",
            Self::PythonBalloon => "Python",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Panel
// ─────────────────────────────────────────────────────────────────────

pub(crate) struct ModelsPalette;

impl Panel for ModelsPalette {
    fn id(&self) -> PanelId { PanelId("rover_models") }
    fn title(&self) -> String { "🧩 Models".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some((mantle, tokens)) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|t| (t.colors.mantle, t.tokens.clone()))
        else {
            return;
        };
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| models_palette_content(ui, ctx, &tokens));
    }
}

fn models_palette_content(ui: &mut egui::Ui, ctx: &mut PanelCtx, tokens: &lunco_theme::DesignTokens) {
    ui.heading("Models");

    // Current attach state (for highlighting selected row + status banner).
    let pending = ctx
        .resource::<AttachState>()
        .map(|s| match s {
            AttachState::Pending(p) => Some(*p),
            AttachState::Idle => None,
        })
        .unwrap_or(None);

    if let Some(p) = pending {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Attach:").color(tokens.success_subdued.linear_multiply(2.0))); // A bit brighter than background
            ui.label(egui::RichText::new(p.title()).strong());
            if ui.button("Cancel").clicked() {
                ctx.defer(|world| {
                    if let Some(mut s) = world.get_resource_mut::<AttachState>() {
                        *s = AttachState::Idle;
                    }
                });
            }
        });
        ui.label(
            egui::RichText::new("Click a ball in the scene to attach.")
                .weak()
                .small(),
        );
        ui.separator();
    }

    for item in [PendingAttachment::ModelicaBalloon, PendingAttachment::PythonBalloon] {
        let is_selected = pending == Some(item);
        let mut label = format!("{}  ({})", item.title(), item.language_label());

        let mut enabled = true;
        if item == PendingAttachment::PythonBalloon {
            #[cfg(not(feature = "python"))]
            {
                label.push_str(" [Disabled - No Python]");
                enabled = false;
            }
        }

        let button = egui::Button::new(label).selected(is_selected).min_size(egui::vec2(ui.available_width(), 24.0));
        if ui.add_enabled(enabled, button).clicked() {
            let new_state = if is_selected {
                AttachState::Idle
            } else {
                AttachState::Pending(item)
            };
            ctx.defer(move |world| {
                if let Some(mut s) = world.get_resource_mut::<AttachState>() {
                    *s = new_state;
                }
            });
        }
    }

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Spawn a plain Dynamic Ball from the Spawn panel, then click a model here, then click the ball.")
            .weak()
            .small(),
    );
}

// ─────────────────────────────────────────────────────────────────────
// Input system — applies the pending attachment on 3D click
// ─────────────────────────────────────────────────────────────────────

/// When `AttachState::Pending`, a left-click on a scene entity inserts the
/// matching marker. Existing setup systems (`Added<BalloonModelMarker>` /
/// `Added<PythonBalloonMarker>`) then compile and wire the model.
///
/// Driven by **bevy_picking** (`On<Pointer<Click>>`): egui occlusion is handled
/// by the framework, and the chrome guard is `hit.position.is_none()`. Ground is
/// skipped so attach lands only on props (balls), matching the old ray-cast that
/// excluded ground colliders. Escape-cancel lives in [`attach_escape_system`].
pub(crate) fn on_scene_click_attach(
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    mut state: ResMut<AttachState>,
    keys: Res<ButtonInput<KeyCode>>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    use bevy::picking::pointer::PointerButton;
    let AttachState::Pending(pending) = *state else { return };
    // Stop the click bubbling to ancestors (global observer re-fires up the tree).
    click.propagate(false);
    if click.button != PointerButton::Primary { return; }
    // Chrome guard — egui's pick has no world position.
    if click.hit.position.is_none() { return; }
    // Shift is reserved (no attach on shift-click).
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { return; }

    // Walk up to the nearest SelectableRoot so attach lands on the user-facing
    // entity (the ball root), not an internal mesh child.
    let target = find_selectable(click.entity, &q_selectable, &q_parents)
        .unwrap_or(click.entity);
    // Don't attach to terrain/ground (old ray-cast excluded ground entities).
    if q_ground.get(target).is_ok() { return; }

    match pending {
        PendingAttachment::ModelicaBalloon => {
            commands.entity(target).try_insert((
                Name::new("Red Balloon (Modelica)"),
                BalloonModelMarker,
            ));
        }
        PendingAttachment::PythonBalloon => {
            commands.entity(target).try_insert((
                Name::new("Green Balloon (Python)"),
                PythonBalloonMarker,
            ));
        }
    }

    *state = AttachState::Idle;
}

/// Escape cancels a pending attachment (keyboard, not a pointer pick).
pub(crate) fn attach_escape_system(
    mut state: ResMut<AttachState>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    if matches!(*state, AttachState::Pending(_)) && keys.just_pressed(KeyCode::Escape) {
        *state = AttachState::Idle;
    }
}

fn find_selectable(
    mut e: Entity,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    loop {
        if q_selectable.get(e).is_ok() {
            return Some(e);
        }
        match q_parents.get(e) {
            Ok(child_of) => e = child_of.parent(),
            Err(_) => return None,
        }
    }
}

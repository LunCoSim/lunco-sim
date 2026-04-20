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
use lunco_workbench::{Panel, PanelId, PanelSlot};

/// Which model the user has selected to attach next.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub enum AttachState {
    /// No attachment pending — clicks in the 3D scene behave normally.
    #[default]
    Idle,
    /// User picked a model from the palette; next click on an entity in
    /// 3D attaches the matching marker.
    Pending(PendingAttachment),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAttachment {
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

pub struct ModelsPalette;

impl Panel for ModelsPalette {
    fn id(&self) -> PanelId { PanelId("rover_models") }
    fn title(&self) -> String { "🧩 Models".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let (mantle, tokens) = {
            let theme = world.resource::<lunco_theme::Theme>();
            (theme.colors.mantle, theme.tokens.clone())
        };
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| models_palette_content(ui, world, &tokens));
    }
}

fn models_palette_content(ui: &mut egui::Ui, world: &mut World, tokens: &lunco_theme::DesignTokens) {
    ui.heading("Models");

    // Current attach state (for highlighting selected row + status banner).
    let pending = world
        .get_resource::<AttachState>()
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
                if let Some(mut s) = world.get_resource_mut::<AttachState>() {
                    *s = AttachState::Idle;
                }
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
        let label = format!("{}  ({})", item.title(), item.language_label());
        let button = egui::Button::new(label).selected(is_selected).min_size(egui::vec2(ui.available_width(), 24.0));
        if ui.add(button).clicked() {
            if let Some(mut s) = world.get_resource_mut::<AttachState>() {
                *s = if is_selected {
                    AttachState::Idle
                } else {
                    AttachState::Pending(item)
                };
            }
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

/// When `AttachState::Pending`, left-click in the 3D scene raycasts for
/// a selectable entity and inserts the matching marker. Existing setup
/// systems (`Added<BalloonModelMarker>` / `Added<PythonBalloonMarker>`)
/// then compile and wire the model. Escape cancels.
///
/// Ordered **before** `handle_entity_selection` so a click that resolves
/// to an attach consumes the frame's input.
pub fn handle_attach_click(
    mut state: ResMut<AttachState>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    windows: Query<&Window>,
    raycaster: avian3d::prelude::SpatialQuery,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let AttachState::Pending(pending) = *state else { return };

    if keys.just_pressed(KeyCode::Escape) {
        *state = AttachState::Idle;
        return;
    }

    // Plain left-click (no shift) in the 3D area attaches.
    if !mouse.just_pressed(MouseButton::Left) { return }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { return }

    let Ok((camera, cam_tf)) = cameras.single() else { return };
    let Ok(window) = windows.single() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Some((origin, direction)) = cursor_ray(camera, cam_tf, cursor) else { return };

    let exclude: Vec<Entity> = q_ground.iter().collect();
    let filter = avian3d::prelude::SpatialQueryFilter::default().with_excluded_entities(exclude);
    let Ok(dir) = Dir3::new(direction) else { return };
    let Some(hit) = raycaster.cast_ray(origin.into(), dir, 1000.0, false, &filter) else {
        return;
    };

    // Walk up to the nearest SelectableRoot so attach lands on the
    // user-facing entity (the ball root), not an internal mesh child.
    let target = find_selectable(hit.entity, &q_selectable, &q_parents)
        .unwrap_or(hit.entity);

    match pending {
        PendingAttachment::ModelicaBalloon => {
            commands.entity(target).insert((
                Name::new("Red Balloon (Modelica)"),
                BalloonModelMarker,
            ));
        }
        PendingAttachment::PythonBalloon => {
            commands.entity(target).insert((
                Name::new("Green Balloon (Python)"),
                PythonBalloonMarker,
            ));
        }
    }

    *state = AttachState::Idle;
}

fn cursor_ray(
    camera: &Camera,
    cam_tf: &GlobalTransform,
    cursor: Vec2,
) -> Option<(Vec3, Vec3)> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    Some((ray.origin, ray.direction.as_vec3()))
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

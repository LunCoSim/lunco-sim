//! Inspector panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Provides editable sliders for transform, physics, and wheel parameters.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use lunco_mobility::WheelRaycast;
use lunco_cosim::{joint_angle_holder, read_input_port, read_output_port, JOINT_ANGLE_PORT};

use lunco_obstacle_field::{ObstacleFieldSpec, Pattern, plugin::UpdateObstacleFieldSpec};

use crate::{SelectedEntities, UndoStack, UndoAction};
use lunco_usd::document::{UsdOp, LayerId};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd::ui::viewport::UsdViewportState;
use lunco_doc::DocumentOrigin;
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset, SdfPath, resolve_bound_shader};

/// Inspector panel — editable entity parameters.
pub struct Inspector;

impl Panel for Inspector {
    fn id(&self) -> PanelId { PanelId("sandbox_inspector") }
    fn title(&self) -> String { "Inspector".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let theme = world.resource::<lunco_theme::Theme>();
        egui::Frame::new()
            .fill(theme.colors.mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| {
                // The Inspector stacks many sections (Environment, Transform,
                // Physics, Wheel, Shader, Material, Modelica) and can exceed the
                // panel height — scroll so the lower sections stay reachable.
                // Shrink VERTICALLY to the content (`auto_shrink` y = true) so a
                // short selection doesn't paint an opaque full-height band of
                // unused panel; the area below then falls through to the
                // transparent panel background (the 3D scene). Keep full WIDTH
                // (x = false) so sliders/labels use the whole column.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| inspector_content(self, ui, world));
            });
    }
}

fn inspector_content(_panel: &mut Inspector, ui: &mut egui::Ui, world: &mut World) {

        // Delete hotkey
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            let primary = world.get_resource::<SelectedEntities>().and_then(|s| s.primary());
            if let Some(entity) = primary {
                if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                    undo.push(UndoAction::Spawned { entity });
                }
                if world.get_entity(entity).is_ok() {
                    world.commands().entity(entity).despawn();
                }
                if let Some(mut selected) = world.get_resource_mut::<SelectedEntities>() {
                    selected.entities.retain(|e| *e != entity);
                }
                return;
            }
        }

        // Esc / Backspace deselection lives in the Bevy `handle_entity_selection`
        // system (the single mutation path), not here — mutating the World
        // mid-egui-render fought the next frame's selection + shader swap.

        ui.heading("Inspector");

        // ── Environment (sun + ambient) ──────────────────────────────
        // Always reachable — a directional light has no clickable geometry, so
        // click-selection can never reach it. Collapsed by default so it doesn't
        // crowd the top of the panel (and can't be mistaken for the selected
        // object's controls). Edits write the LIVE light components/resources
        // directly; they are session-transient (persisting back into the scene
        // layer is the save-scene workstream).
        egui::CollapsingHeader::new("Environment (Sun & Ambient)")
            .default_open(false)
            .show(ui, |ui| environment_section(ui, world));
        ui.separator();

        // ── Camera (exposure + post-process) ─────────────────────────
        // Physical exposure and bloom live on the camera, not the lights, so
        // they get their own section. Same live, session-transient editing as
        // Environment; dispatched through the same `SetEnvironmentLight` path.
        egui::CollapsingHeader::new("Camera")
            .default_open(false)
            .show(ui, |ui| camera_section(ui, world));
        ui.separator();

        // ── Obstacle Field (procedural craters + rocks) ──────────────
        // Global generator controls (a Resource, not a selected entity), so it
        // sits alongside Environment/Camera. Sliders edit the spec live; the
        // field only rebuilds on release / button (regen is one synchronous
        // pass — a brief hitch — so we don't rebuild every drag frame).
        egui::CollapsingHeader::new("Obstacle Field (Craters & Rocks)")
            .default_open(true)
            .show(ui, |ui| obstacle_field_section(ui, world));
        ui.separator();

        // The terrain shader is NO LONGER an always-on section: the ground is
        // click-selectable, so its shader params appear (like any object's) only
        // when it's the selected entity. This stops the old always-on terrain
        // controls — which sat at the very top — from being edited by mistake
        // while a different object was selected.

        // Get current selection
        let Some(entity) = world.get_resource::<SelectedEntities>().and_then(|s| s.primary()) else {
            ui.label("No entity selected.");
            ui.label("Press Shift+Left-click on an object to select it.");
            return;
        };

        ui.label(format!("ID: {entity:?}"));

        // Name (read-only)
        if let Ok(name) = world.query::<&Name>().get(world, entity) {
            ui.label(format!("Name: {}", name.as_str()));
        }

        ui.separator();

        // ── Transform component ──────────────────────────────────────
        // First component: open by default — most users want to nudge
        // position immediately. Other components start collapsed.
        if world.query::<&Transform>().get(world, entity).is_ok() {
            egui::CollapsingHeader::new("Transform")
                .default_open(true)
                .show(ui, |ui| {
                    if let Some((old_tf, new_vals)) =
                        world.query::<&Transform>().get(world, entity).ok().map(|tf| {
                            (
                                (tf.translation, tf.rotation),
                                (tf.translation.x, tf.translation.y, tf.translation.z),
                            )
                        })
                    {
                        let mut x = new_vals.0;
                        let mut y = new_vals.1;
                        let mut z = new_vals.2;
                        let changed = ui.add(egui::Slider::new(&mut x, -1000.0..=1000.0).text("X")).changed()
                            | ui.add(egui::Slider::new(&mut y, -1000.0..=1000.0).text("Y")).changed()
                            | ui.add(egui::Slider::new(&mut z, -1000.0..=1000.0).text("Z")).changed();
                        if changed {
                            if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                                undo.push(UndoAction::TransformChanged {
                                    entity,
                                    old_translation: old_tf.0,
                                    old_rotation: old_tf.1,
                                });
                            }
                            if let Ok(mut tf) = world.query::<&mut Transform>().get_mut(world, entity) {
                                tf.translation = Vec3::new(x, y, z);
                            }
                        }
                    }
                });
        }

        // ── Physics component ────────────────────────────────────────
        let has_physics = world.query::<&avian3d::prelude::RigidBody>().get(world, entity).is_ok()
            || world.query::<&avian3d::prelude::Mass>().get(world, entity).is_ok()
            || world.query::<&avian3d::prelude::LinearDamping>().get(world, entity).is_ok()
            || world.query::<&avian3d::prelude::AngularDamping>().get(world, entity).is_ok();
        if has_physics {
            egui::CollapsingHeader::new("Physics")
                .default_open(false)
                .show(ui, |ui| {
                    if let Ok(rb) = world.query::<&avian3d::prelude::RigidBody>().get(world, entity) {
                        ui.label(format!("Type: {rb:?}"));
                    }
                    if let Ok(current) = world.query::<&avian3d::prelude::Mass>().get(world, entity) {
                        let mut m = current.0;
                        if ui.add(egui::Slider::new(&mut m, 0.1..=100000.0).text("Mass (kg)").logarithmic(true)).changed() {
                            if let Ok(mut mass) = world.query::<&mut avian3d::prelude::Mass>().get_mut(world, entity) {
                                mass.0 = m;
                            }
                        }
                    }
                    if let Ok(current) = world.query::<&avian3d::prelude::LinearDamping>().get(world, entity) {
                        let mut d = current.0 as f32;
                        if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Linear Damping")).changed() {
                            if let Ok(mut damp) = world.query::<&mut avian3d::prelude::LinearDamping>().get_mut(world, entity) {
                                damp.0 = d as f64;
                            }
                        }
                    }
                    if let Ok(current) = world.query::<&avian3d::prelude::AngularDamping>().get(world, entity) {
                        let mut d = current.0 as f32;
                        if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Angular Damping")).changed() {
                            if let Ok(mut damp) = world.query::<&mut avian3d::prelude::AngularDamping>().get_mut(world, entity) {
                                damp.0 = d as f64;
                            }
                        }
                    }
                });
        }

        // ── Wheel (Raycast) component ────────────────────────────────
        if world.query::<&WheelRaycast>().get(world, entity).is_ok() {
            egui::CollapsingHeader::new("Wheel (Raycast)")
                .default_open(false)
                .show(ui, |ui| {
                    if let Ok(current) = world.query::<&WheelRaycast>().get(world, entity) {
                        let mut rest = current.rest_length as f32;
                        let mut k = current.spring_k as f32;
                        let mut d = current.damping_c as f32;
                        let mut radius = current.wheel_radius as f32;

                        let rest_changed = ui.add(egui::Slider::new(&mut rest, 0.1..=2.0).text("Rest Length (m)")).changed();
                        let k_changed = ui.add(egui::Slider::new(&mut k, 100.0..=100000.0).text("Spring K (N/m)").logarithmic(true)).changed();
                        let d_changed = ui.add(egui::Slider::new(&mut d, 100.0..=10000.0).text("Damping C (N·s/m)").logarithmic(true)).changed();
                        let r_changed = ui.add(egui::Slider::new(&mut radius, 0.1..=2.0).text("Wheel Radius (m)")).changed();

                        if rest_changed || k_changed || d_changed || r_changed {
                            if let Ok(mut wheel) = world.query::<&mut WheelRaycast>().get_mut(world, entity) {
                                if rest_changed { wheel.rest_length = rest as f64; }
                                if k_changed { wheel.spring_k = k as f64; }
                                if d_changed { wheel.damping_c = d as f64; }
                                if r_changed { wheel.wheel_radius = radius as f64; }
                            }
                        }
                    }
                });
        }

        // ── Materials ────────────────────────────────────────────────
        // A material lives on the leaf MESH entities (wheel visuals, body
        // mesh), never on the logical ROOT a click selects — so the Inspector
        // always edits ONE concrete part, chosen by the *Part* dropdown or a
        // viewport drill-click (clicking a sub-part of the selected object).
        // There is no "whole object" aggregate: showing a wheel's shader as if
        // it were the rover's was misleading. The default part is the first one
        // WITHOUT a shader (the PBR body) so a rover, which has no shader of its
        // own, opens on its body with an "Add shader" picker front-and-centre.
        let parts = editable_parts(world, entity);
        if !parts.is_empty() {
            // Resolve the target part. A stale stored target (from a prior
            // selection) is ignored; the default is persisted so it can't flip
            // to another part after you, e.g., add a shader to the body.
            let stored = world
                .resource::<crate::InspectorTarget>()
                .part
                .filter(|p| parts.iter().any(|(e, _)| e == p));
            let mut target = stored.or_else(|| default_part(world, &parts));
            if stored.is_none() {
                if let Some(t) = target {
                    world.resource_mut::<crate::InspectorTarget>().part = Some(t);
                }
            }
            // Multi-part object → a dropdown to switch parts (may retarget).
            if parts.len() > 1 {
                target = parts_selector(ui, world, &parts, target);
            }

            if let Some(part) = target {
                // Shader picker — ADD a shader to this part (converting a PBR
                // part) or swap an existing one. Always shown, so a part with no
                // shader yet gets an "Add shader" affordance; after adding, the
                // Shader Parameters below become editable.
                shader_picker_for_part(ui, world, part);
                shader_tools_ui(ui, world, part);

                if let Some(holder) = first_shader_holder(world, part) {
                    egui::CollapsingHeader::new("Shader Parameters")
                        .default_open(true)
                        .show(ui, |ui| {
                            shader_parameters_section(ui, world, holder);
                        });
                }
                let std_handles = collect_std_handles(world, part);
                if !std_handles.is_empty() {
                    egui::CollapsingHeader::new("Material (PBR)")
                        .default_open(true)
                        .show(ui, |ui| {
                            material_pbr_section(ui, world, part, &std_handles);
                        });
                }
            }
        }

        // ── Modelica parameters component ───────────────────────────
        // Tunable Real parameters from the entity's Modelica model.
        // Edits dispatch a `ModelicaOp::SetParameter` through the
        // canonical op pipeline (span-patch + AST refresh + index
        // patch + journal) and fire `UpdateParameters` at the worker,
        // which recompiles and reseeds the stepper.
        let has_modelica = world
            .query::<&lunco_modelica::ModelicaModel>()
            .get(world, entity)
            .is_ok();
        if has_modelica {
            egui::CollapsingHeader::new("Modelica Parameters")
                .default_open(true)
                .show(ui, |ui| {
                    modelica_parameters_section(ui, world, entity);
                });
        }

        // ── Joint control ───────────────────────────────────────────
        // If this entity (or a child — the joint prim is usually nested,
        // e.g. /SolarTower/Hinge) carries a revolute joint (auto-exposed as the
        // `angle` co-sim port), expose it: the live measured angle, plus a
        // setpoint slider that writes the commanded `angle` input. This is the
        // "control the used model, particularly the joint" surface.
        if let Some(holder) = joint_angle_holder(world, entity) {
            egui::CollapsingHeader::new("Joint")
                .default_open(true)
                .show(ui, |ui| {
                    joint_control_section(ui, world, holder);
                });
        }

        // Delete button
        ui.separator();
        if ui.button("🗑 Delete Entity (Del)").clicked() {
            if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                undo.push(UndoAction::Spawned { entity });
            }
            if world.get_entity(entity).is_ok() {
                world.commands().entity(entity).despawn();
            }
            if let Some(mut selected) = world.get_resource_mut::<SelectedEntities>() {
                selected.entities.retain(|e| *e != entity);
            }
        }
    }

/// Live sun + ambient controls. Works on whatever directional light is
/// currently in the world — the binary's fallback sun or a scene-authored
/// UsdLux `DistantLight` — so it doubles as the runtime tuning surface for
/// values that will later be written back into the `.usda`.
///
/// The widgets only READ component state; every edit dispatches a
/// [`SetEnvironmentLight`] command, the same single mutation path the
/// HTTP/MCP API uses. UI, API, and scripts therefore can't drift apart in
/// behaviour — there is exactly one observer that writes lighting.
fn environment_section(ui: &mut egui::Ui, world: &mut World) {
    use bevy::light::{CascadeShadowConfig, DirectionalLight, GlobalAmbientLight};
    use lunco_environment::SetEnvironmentLight;

    // Skip render-layer-scoped lights (the USD preview viewport's sun) —
    // same rule as the horizon system's pick_sun; otherwise the panel shows
    // the preview light's state instead of the scene sun's. Also exclude the
    // earthshine fill (`Without<Earthshine>`), or the panel would bind to it
    // and the sun controls would edit the wrong light.
    let suns: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<DirectionalLight>,
            Without<bevy::camera::visibility::RenderLayers>,
            Without<lunco_environment::Earthshine>,
        )>()
        .iter(world)
        .collect();
    if suns.is_empty() && world.get_resource::<GlobalAmbientLight>().is_none() {
        return;
    }

    // Accumulate one command from whatever widgets changed this frame;
    // `None` fields keep their current value in the observer.
    let mut cmd = SetEnvironmentLight::default();
    let mut any_change = false;

    egui::CollapsingHeader::new("Environment")
        .default_open(true)
        .show(ui, |ui| {
            // The command applies to every directional light; render the
            // controls off the first sun's live state.
            if let Some(&entity) = suns.first() {
                if let Some(name) = world.get::<Name>(entity) {
                    ui.label(egui::RichText::new(name.as_str().to_string()).strong());
                }

                if let Some(tf) = world.get::<Transform>(entity) {
                    let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                    let mut yaw_deg = yaw.to_degrees();
                    let mut pitch_deg = pitch.to_degrees();
                    let yaw_changed = ui
                        .add(egui::Slider::new(&mut yaw_deg, -180.0..=180.0).text("Yaw (°)"))
                        .changed();
                    let pitch_changed = ui
                        .add(egui::Slider::new(&mut pitch_deg, -90.0..=90.0).text("Pitch (°)"))
                        .changed();
                    if yaw_changed {
                        cmd.sun_yaw = Some(yaw_deg.to_radians());
                    }
                    if pitch_changed {
                        cmd.sun_pitch = Some(pitch_deg.to_radians());
                    }
                    any_change |= yaw_changed || pitch_changed;
                }

                if let Some(light) = world.get::<DirectionalLight>(entity) {
                    let mut lux = light.illuminance;
                    let mut shadows = light.shadows_enabled;
                    let lin = light.color.to_linear();
                    let mut rgb = [lin.red, lin.green, lin.blue];
                    if ui
                        .add(
                            egui::Slider::new(&mut lux, 100.0..=200_000.0)
                                .text("Illuminance (lx)")
                                .logarithmic(true),
                        )
                        .changed()
                    {
                        cmd.illuminance = Some(lux);
                        any_change = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.color_edit_button_rgb(&mut rgb).changed() {
                            cmd.sun_color = Some(rgb);
                            any_change = true;
                        }
                        ui.label("Color");
                    });
                    if ui.checkbox(&mut shadows, "Cast shadows").changed() {
                        cmd.shadows_enabled = Some(shadows);
                        any_change = true;
                    }
                }

                // Shadow range. bounds[0] is the first cascade's far bound
                // (near-field sharpness), bounds.last() the total shadow
                // distance — smaller max ⇒ denser texels ⇒ crisper shadows.
                if let Some(cfg) = world.get::<CascadeShadowConfig>(entity) {
                    let mut first = cfg.bounds.first().copied().unwrap_or(40.0);
                    let mut max = cfg.bounds.last().copied().unwrap_or(1500.0);
                    if ui
                        .add(
                            egui::Slider::new(&mut first, 5.0..=200.0)
                                .text("Near shadow bound (m)")
                                .logarithmic(true),
                        )
                        .changed()
                    {
                        cmd.shadow_first_cascade_bound = Some(first);
                        any_change = true;
                    }
                    if ui
                        .add(
                            egui::Slider::new(&mut max, 50.0..=5000.0)
                                .text("Shadow max distance (m)")
                                .logarithmic(true),
                        )
                        .changed()
                    {
                        cmd.shadow_max_distance = Some(max);
                        any_change = true;
                    }
                }
                ui.separator();
            }

            if let Some(ambient) = world.get_resource::<GlobalAmbientLight>() {
                let mut b = ambient.brightness;
                if ui
                    .add(egui::Slider::new(&mut b, 0.0..=400.0).text("Ambient (cd/m²)"))
                    .changed()
                {
                    cmd.ambient_brightness = Some(b);
                    any_change = true;
                }
            }

            // Earthshine fill (the cool-blue shadowless light) — read off the
            // single earthshine entity. It is a fill light, so it belongs with
            // the environment lighting rather than the Camera section.
            let es_lux = world
                .query_filtered::<&DirectionalLight, With<lunco_environment::Earthshine>>()
                .iter(world)
                .next()
                .map(|l| l.illuminance);
            if let Some(mut lux) = es_lux {
                if ui
                    .add(egui::Slider::new(&mut lux, 0.0..=60.0).text("Earthshine (lx)"))
                    .changed()
                {
                    cmd.earthshine_illuminance = Some(lux);
                    any_change = true;
                }
            }
        });

    if any_change {
        world.trigger(cmd);
    }
}

/// Camera section — physical exposure and bloom. These live on the camera, not
/// the lights, so they are separated from [`environment_section`]. Mutates via
/// the same [`SetEnvironmentLight`] command (its handler carries the camera
/// arms), so all environment/camera edits share one mutation path.
fn camera_section(ui: &mut egui::Ui, world: &mut World) {
    use bevy::camera::Exposure;
    use bevy::post_process::bloom::Bloom;
    use lunco_environment::SetEnvironmentLight;

    let mut cmd = SetEnvironmentLight::default();
    let mut any_change = false;

    // Exposure (EV100): the physical counterpart to sun illuminance. Lower EV
    // ⇒ brighter image; ~15 = sunlit, 9.7 = Blender default. Read off the first
    // camera that carries an Exposure component.
    let cam_ev = world
        .query::<&Exposure>()
        .iter(world)
        .next()
        .map(|e| e.ev100);
    if let Some(mut ev) = cam_ev {
        if ui
            .add(egui::Slider::new(&mut ev, 5.0..=18.0).text("Exposure (EV100)"))
            .changed()
        {
            cmd.exposure_ev100 = Some(ev);
            any_change = true;
        }
    } else {
        ui.label("No camera Exposure component.");
    }

    // Bloom intensity (cameras with a Bloom component; airless ⇒ low).
    let cam_bloom = world
        .query::<&Bloom>()
        .iter(world)
        .next()
        .map(|b| b.intensity);
    if let Some(mut i) = cam_bloom {
        if ui
            .add(egui::Slider::new(&mut i, 0.0..=1.0).text("Bloom intensity"))
            .changed()
        {
            cmd.bloom_intensity = Some(i);
            any_change = true;
        }
    }

    if any_change {
        world.trigger(cmd);
    }
}

/// Live tuning for the procedural obstacle field. Sliders edit the
/// `ObstacleFieldSpec` resource directly; the field rebuilds only on slider
/// release / button press (a regen is one synchronous pass — backgrounding it is
/// the next phase), so dragging stays smooth.
fn obstacle_field_section(ui: &mut egui::Ui, world: &mut World) {
    let mut regen = false;

    {
        let Some(mut spec) = world.get_resource_mut::<ObstacleFieldSpec>() else {
            ui.label("Obstacle field plugin not active.");
            return;
        };

        ui.horizontal(|ui| {
            ui.label(format!("Seed {:#x}", spec.seed));
            if ui.button("🎲 Reseed").clicked() {
                // SplitMix64 step → a fresh, well-distributed seed.
                spec.seed = spec
                    .seed
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(0x2545_F491_4F6C_DD1D);
                regen = true;
            }
        });

        if ui
            .add(egui::Slider::new(&mut spec.region_half_extent, 50.0..=500.0).text("Region ½ (m)"))
            .drag_stopped()
        {
            regen = true;
        }
        if ui
            .add(egui::Slider::new(&mut spec.grid_resolution, 65u32..=513).text("Grid res"))
            .drag_stopped()
        {
            regen = true;
        }

        // Spatial pattern.
        let mut kind = match spec.pattern {
            Pattern::Uniform => 0usize,
            Pattern::PoissonDisk { .. } => 1,
            Pattern::Clustered { .. } => 2,
        };
        egui::ComboBox::from_label("Pattern")
            .selected_text(["Uniform", "Poisson disk", "Clustered"][kind])
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut kind, 0, "Uniform");
                ui.selectable_value(&mut kind, 1, "Poisson disk");
                ui.selectable_value(&mut kind, 2, "Clustered");
            });
        let chosen = match kind {
            0 => Pattern::Uniform,
            1 => match spec.pattern {
                p @ Pattern::PoissonDisk { .. } => p,
                _ => Pattern::PoissonDisk { min_spacing: 3.0 },
            },
            _ => match spec.pattern {
                p @ Pattern::Clustered { .. } => p,
                _ => Pattern::Clustered { clusters: 8, spread: 25.0 },
            },
        };
        if std::mem::discriminant(&spec.pattern) != std::mem::discriminant(&chosen) {
            spec.pattern = chosen;
            regen = true;
        }
        match &mut spec.pattern {
            Pattern::PoissonDisk { min_spacing } => {
                if ui
                    .add(egui::Slider::new(min_spacing, 0.5..=10.0).text("Min spacing (m)"))
                    .drag_stopped()
                {
                    regen = true;
                }
            }
            Pattern::Clustered { clusters, spread } => {
                if ui.add(egui::Slider::new(clusters, 1u32..=32).text("Clusters")).drag_stopped() {
                    regen = true;
                }
                if ui.add(egui::Slider::new(spread, 2.0..=80.0).text("Spread (m)")).drag_stopped() {
                    regen = true;
                }
            }
            Pattern::Uniform => {}
        }

        egui::CollapsingHeader::new("Craters").default_open(true).show(ui, |ui| {
            // Reborrow into a plain &mut so the disjoint field borrows below are
            // allowed (ResMut's DerefMut would otherwise re-borrow the whole spec).
            let s = &mut *spec;
            if ui.checkbox(&mut s.craters.enabled, "Enabled").changed() {
                regen = true;
            }
            for (val, range, label) in [
                (&mut s.craters.density, 0.0..=60.0, "Density /ha"),
                (&mut s.craters.depth_ratio, 0.0..=0.8, "Depth ratio"),
                (&mut s.craters.rim_height_ratio, 0.0..=1.5, "Wall height ratio"),
                (&mut s.craters.size.min, 0.5..=20.0, "Radius min"),
                (&mut s.craters.size.mode, 0.5..=20.0, "Radius mode"),
                (&mut s.craters.size.max, 0.5..=40.0, "Radius max"),
            ] {
                if ui.add(egui::Slider::new(val, range).text(label)).drag_stopped() {
                    regen = true;
                }
            }
        });

        egui::CollapsingHeader::new("Rocks").default_open(true).show(ui, |ui| {
            let s = &mut *spec;
            if ui.checkbox(&mut s.rocks.enabled, "Enabled").changed() {
                regen = true;
            }
            for (val, range, label) in [
                (&mut s.rocks.density, 0.0..=400.0, "Density /ha"),
                (&mut s.rocks.size.min, 0.05..=5.0, "Radius min"),
                (&mut s.rocks.size.mode, 0.05..=5.0, "Radius mode"),
                (&mut s.rocks.size.max, 0.05..=8.0, "Radius max"),
                (&mut s.rocks.dynamic_fraction, 0.0..=1.0, "Dynamic frac"),
            ] {
                if ui.add(egui::Slider::new(val, range).text(label)).drag_stopped() {
                    regen = true;
                }
            }
        });

        ui.separator();
        if ui.button("♻ Regenerate").clicked() {
            regen = true;
        }
        ui.label(egui::RichText::new("Field rebuilds on slider release.").small().weak());
    }

    if regen {
        if let Some(spec) = world.get_resource::<ObstacleFieldSpec>() {
            let spec_cloned = spec.clone();
            world.trigger(UpdateObstacleFieldSpec { spec: spec_cloned });
        }
    }
}

/// The selected entity plus all of its descendants. Materials live on leaf mesh
/// entities while selection targets the logical root, so the material sections
/// search this whole set.
fn subtree(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut q = world.query::<&Children>();
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let e = out[i];
        i += 1;
        if let Ok(children) = q.get(world, e) {
            out.extend(children.iter());
        }
    }
    out
}

/// Joint control over a revolute joint's `angle` port. Shows the live measured
/// angle (the joint twist, read through [`read_output_port`]) and a setpoint
/// slider that writes the commanded angle (the motor target, read through
/// [`read_input_port`]) via [`lunco_cosim::write_port`] — the same port the
/// angular motor chases.
///
/// Note: when a live wire drives this joint (e.g. the sun tracker's
/// `yaw -> angle`), `propagate_connections` rewrites the motor target every
/// tick, so a hand-set value is transient — it nudges the joint for one tick
/// and the wire reclaims it. For an *un-wired* joint the slider holds. A
/// latching hand-override (latest-wins until released) is the pending
/// `SetPort` ControlStream hold (see `lunco-cosim/src/ports.rs`).
fn joint_control_section(ui: &mut egui::Ui, world: &mut World, holder: Entity) {
    let measured = read_output_port(world, holder, JOINT_ANGLE_PORT).unwrap_or(0.0);
    let mut commanded = read_input_port(world, holder, JOINT_ANGLE_PORT).unwrap_or(0.0);
    let mut cq = world.query::<&lunco_cosim::SimConnection>();
    let wired = cq
        .iter(world)
        .any(|c| c.end_element == holder && c.end_connector == JOINT_ANGLE_PORT);

    ui.label(format!(
        "measured: {:.3} rad  ({:.1}°)",
        measured,
        measured.to_degrees()
    ));

    let r = ui.add(
        egui::Slider::new(&mut commanded, -std::f64::consts::PI..=std::f64::consts::PI)
            .text("setpoint (rad)"),
    );
    ui.label(format!("{:.1}°", commanded.to_degrees()));
    if r.changed() {
        lunco_cosim::write_port(world, holder, JOINT_ANGLE_PORT, commanded);
    }
    if wired {
        ui.label(
            egui::RichText::new("⚠ driven by a wire — setpoint is transient")
                .small()
                .weak(),
        );
    }
}

/// Distinct `StandardMaterial` handles anywhere in `root`'s subtree (deduped by
/// asset id), so editing recolors every part at once.
fn collect_std_handles(world: &mut World, root: Entity) -> Vec<Handle<StandardMaterial>> {
    let ents = subtree(world, root);
    let mut q = world.query::<&MeshMaterial3d<StandardMaterial>>();
    let mut handles: Vec<Handle<StandardMaterial>> = Vec::new();
    for e in ents {
        if let Ok(m) = q.get(world, e) {
            if !handles.iter().any(|h| h.id() == m.0.id()) {
                handles.push(m.0.clone());
            }
        }
    }
    handles
}

/// First entity in `root`'s subtree carrying a [`ShaderMaterial`], if any.
fn first_shader_holder(world: &mut World, root: Entity) -> Option<Entity> {
    let ents = subtree(world, root);
    let mut q = world.query::<&MeshMaterial3d<lunco_materials::ShaderMaterial>>();
    ents.into_iter().find(|e| q.get(world, *e).is_ok())
}

/// Material-bearing parts of `root`'s subtree — every entity carrying a
/// `ShaderMaterial` or `StandardMaterial` — each labelled by its leaf name
/// (`…/Wheel_FL` → `Wheel_FL`). The Inspector lists these in its *Parts*
/// selector so editing can be aimed at one wheel/body rather than the whole
/// component. Subtree (root-first) order; a single-mesh prop yields one entry.
fn editable_parts(world: &mut World, root: Entity) -> Vec<(Entity, String)> {
    let ents = subtree(world, root);
    let mut shaderq = world.query::<&MeshMaterial3d<lunco_materials::ShaderMaterial>>();
    let mut stdq = world.query::<&MeshMaterial3d<StandardMaterial>>();
    let mut nameq = world.query::<&Name>();
    let mut out = Vec::new();
    for e in ents {
        if shaderq.get(world, e).is_ok() || stdq.get(world, e).is_ok() {
            let label = nameq
                .get(world, e)
                .ok()
                .map(|n| n.as_str().rsplit(['/', '\\']).next().unwrap_or(n.as_str()).to_string())
                .unwrap_or_else(|| format!("{e:?}"));
            out.push((e, label));
        }
    }
    out
}

/// Default part to edit when nothing is explicitly targeted: the first part
/// WITHOUT a shader — i.e. the PBR body — so a rover opens on its body with an
/// "Add shader" picker rather than surfacing a wheel's shader. Falls back to
/// the first part when every part already has a shader.
fn default_part(world: &mut World, parts: &[(Entity, String)]) -> Option<Entity> {
    let mut shq = world.query::<&MeshMaterial3d<lunco_materials::ShaderMaterial>>();
    parts
        .iter()
        .map(|(e, _)| *e)
        .find(|e| shq.get(world, *e).is_err())
        .or_else(|| parts.first().map(|(e, _)| *e))
}

/// *Part* dropdown for a multi-part component: lists each [`editable_parts`]
/// entry (no aggregate), writes the choice into [`InspectorTarget`], and
/// returns the new target. `current` is the part shown when nothing is clicked.
fn parts_selector(
    ui: &mut egui::Ui,
    world: &mut World,
    parts: &[(Entity, String)],
    current: Option<Entity>,
) -> Option<Entity> {
    let cur_label = current
        .and_then(|c| parts.iter().find(|(e, _)| *e == c).map(|(_, l)| l.clone()))
        .unwrap_or_else(|| "—".to_string());

    let mut chosen: Option<Entity> = None;
    egui::ComboBox::from_label("Part")
        .selected_text(cur_label)
        .show_ui(ui, |ui| {
            for (e, label) in parts {
                if ui.selectable_label(current == Some(*e), label).clicked() {
                    chosen = Some(*e);
                }
            }
        });
    if let Some(c) = chosen {
        world.resource_mut::<crate::InspectorTarget>().part = Some(c);
        return Some(c);
    }
    current
}

/// Shader picker for a single part. Lists the [`ShaderCatalog`] entries and, on
/// pick, swaps the `.wgsl` on `part` directly — works by `Entity` (sub-parts
/// have no API id) and, on a plain `StandardMaterial` part, CONVERTS it to a
/// `ShaderMaterial` (so you can put a shader on a rover body). Uniform-
/// preserving when the part already has a `ShaderMaterial`.
fn shader_picker_for_part(ui: &mut egui::Ui, world: &mut World, part: Entity) {
    let entries = world
        .get_resource::<lunco_materials::ShaderCatalog>()
        .map(|c| c.entries.clone())
        .unwrap_or_default();
    if entries.is_empty() {
        return;
    }
    // Current shader path of this part, if it already uses a ShaderMaterial.
    let cur_path: Option<String> = world
        .query::<&MeshMaterial3d<lunco_materials::ShaderMaterial>>()
        .get(world, part)
        .ok()
        .map(|m| m.0.clone())
        .and_then(|h| {
            world
                .resource::<Assets<lunco_materials::ShaderMaterial>>()
                .get(&h)
                .map(|m| m.shader.id())
        })
        .and_then(|id| world.resource::<AssetServer>().get_path(id))
        // Full `AssetPath` string (incl. `twin://name/` source) so twin shaders
        // match their catalog entry, not just the bare `path()`.
        .map(|p| p.to_string());
    let cur = cur_path.unwrap_or_default();
    let cur_label = entries
        .iter()
        .find(|e| e.path == cur)
        .map(|e| e.label.clone())
        .unwrap_or_else(|| "— (none)".to_string());

    let mut chosen: Option<String> = None;
    egui::ComboBox::from_label("Shader")
        .selected_text(cur_label)
        .show_ui(ui, |ui| {
            for e in &entries {
                if ui.selectable_label(e.path == cur, &e.label).clicked() {
                    chosen = Some(e.path.clone());
                }
            }
        });
    if let Some(path) = chosen {
        if path != cur {
            swap_shader_on_entity(world, part, &path);
        }
    }
}

/// Bind shader `path` to `part`, building a fresh [`ShaderMaterial`] (carrying
/// over the previous one's uniforms if it had any) and removing the part's
/// `StandardMaterial` — the same uniform-preserving swap the
/// `SetObjectProperty { property: "shader" }` command performs, but addressed
/// by `Entity` so it reaches sub-parts that have no API id.
fn swap_shader_on_entity(world: &mut World, part: Entity, path: &str) {
    use lunco_materials::ShaderMaterial;
    let template = world
        .query::<&MeshMaterial3d<ShaderMaterial>>()
        .get(world, part)
        .ok()
        .map(|m| m.0.clone())
        .and_then(|h| world.resource::<Assets<ShaderMaterial>>().get(&h).cloned())
        .unwrap_or_default();
    let shader = world.resource::<AssetServer>().load(path.to_string());
    let handle = world
        .resource_mut::<Assets<ShaderMaterial>>()
        .add(lunco_materials::build_shader_material(shader, template));
    world
        .commands()
        .entity(part)
        .remove::<MeshMaterial3d<StandardMaterial>>()
        .insert(MeshMaterial3d(handle));

    // Propagate changes to USD
    if world.get::<UsdPrimPath>(part).is_some() {
        apply_usd_attribute_change(
            world,
            part,
            "primvars:materialType",
            "string",
            "\"shader\"".to_string(),
        );
        apply_usd_attribute_change(
            world,
            part,
            "primvars:shaderPath",
            "asset",
            format!("@{}@", path),
        );
    }
}

/// The full asset-path string of `part`'s current `ShaderMaterial` shader
/// (incl. any `twin://` source), or `None` if it isn't using one.
fn current_shader_path(world: &mut World, part: Entity) -> Option<String> {
    let h = world
        .query::<&MeshMaterial3d<lunco_materials::ShaderMaterial>>()
        .get(world, part)
        .ok()
        .map(|m| m.0.clone())?;
    let sid = world
        .resource::<Assets<lunco_materials::ShaderMaterial>>()
        .get(&h)
        .map(|m| m.shader.id())?;
    let p = world.resource::<AssetServer>().get_path(sid)?;
    Some(p.to_string())
}

/// "Shader Tools" — GUI front-end for the live shader-authoring commands
/// ([`crate::commands::CreateShader`] / `ImportShader` / `RescanShaders` /
/// `DeleteShader`). Create and Import additionally apply the result to `part`
/// **by `Entity`** (so it reaches sub-parts that have no API id). Commands are
/// fired with `world.trigger`, which runs their observers synchronously, so the
/// new catalog entry is visible the moment we go to apply it.
fn shader_tools_ui(ui: &mut egui::Ui, world: &mut World, part: Entity) {
    egui::CollapsingHeader::new("Shader Tools")
        .default_open(false)
        .show(ui, |ui| {
            let id = ui.make_persistent_id("shader_tools_state");
            #[derive(Clone, Default)]
            struct St {
                name: String,
                template: String,
                import: String,
            }
            let mut st: St = ui.memory_mut(|m| m.data.get_temp::<St>(id)).unwrap_or_default();
            if st.template.is_empty() {
                st.template = "solid".to_string();
            }

            // ── New from template ──
            ui.label("New shader from template:");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut st.name)
                        .hint_text("name")
                        .desired_width(110.0),
                );
                let cur_label = lunco_materials::shader_template_kinds()
                    .iter()
                    .find(|(k, _)| *k == st.template)
                    .map(|(_, l)| *l)
                    .unwrap_or("Solid");
                egui::ComboBox::from_id_salt("shader_template")
                    .selected_text(cur_label)
                    .show_ui(ui, |ui| {
                        for (k, l) in lunco_materials::shader_template_kinds() {
                            if ui.selectable_label(st.template == *k, *l).clicked() {
                                st.template = k.to_string();
                            }
                        }
                    });
            });
            if ui
                .add_enabled(
                    !st.name.trim().is_empty(),
                    egui::Button::new("Create & apply"),
                )
                .clicked()
            {
                create_and_apply(world, part, &st.name, &st.template);
                st.name.clear();
            }

            ui.separator();
            // ── Import from disk ──
            ui.label("Import .wgsl from disk:");
            ui.add(
                egui::TextEdit::singleline(&mut st.import)
                    .hint_text("/path/to/shader.wgsl")
                    .desired_width(220.0),
            );
            if ui
                .add_enabled(
                    !st.import.trim().is_empty(),
                    egui::Button::new("Import & apply"),
                )
                .clicked()
            {
                import_and_apply(world, part, st.import.trim());
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("Rescan twin folder")
                    .on_hover_text("Register any .wgsl dropped into the twin's shaders/ folder")
                    .clicked()
                {
                    world.trigger(crate::commands::RescanShaders {});
                }
                if let Some(path) = current_shader_path(world, part) {
                    if ui
                        .button("Delete current")
                        .on_hover_text(format!("Remove {path} (file + picker)"))
                        .clicked()
                    {
                        world.trigger(crate::commands::DeleteShader { path });
                    }
                }
            });

            ui.memory_mut(|m| m.data.insert_temp(id, st));
        });
}

/// Create a shader from `template` (registers it), then bind it to `part` by
/// `Entity`. Only applies if the command actually produced the catalog entry
/// (a rejected/invalid create leaves the part untouched).
fn create_and_apply(world: &mut World, part: Entity, name: &str, template: &str) {
    world.trigger(crate::commands::CreateShader {
        name: name.to_string(),
        template: template.to_string(),
        source: String::new(),
        target: 0,
    });
    let stem = crate::commands::sanitize_stem(name);
    apply_if_registered(world, part, &stem);
}

/// Import an external `.wgsl` (registers it), then bind it to `part`. Skips the
/// apply if the import was rejected (e.g. not a prop-pickable shader).
fn import_and_apply(world: &mut World, part: Entity, src_path: &str) {
    world.trigger(crate::commands::ImportShader {
        source_path: src_path.to_string(),
        name: String::new(),
        target: 0,
    });
    let stem = std::path::Path::new(src_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(crate::commands::sanitize_stem)
        .unwrap_or_default();
    if !stem.is_empty() {
        apply_if_registered(world, part, &stem);
    }
}

/// If a shader for `stem` is now registered (its predicted asset path is in the
/// catalog), swap `part` onto it.
fn apply_if_registered(world: &mut World, part: Entity, stem: &str) {
    let path = {
        let tr = world.get_resource::<lunco_assets::twin_source::TwinRoots>();
        crate::commands::shader_asset_path_for(tr, stem)
    };
    let registered = world
        .resource::<lunco_materials::ShaderCatalog>()
        .entries
        .iter()
        .any(|e| e.path == path);
    if registered {
        swap_shader_on_entity(world, part, &path);
    }
}

/// Editable PBR controls for the selected object's `StandardMaterial`s — the
/// default bevy material props/rovers carry unless a custom `ShaderMaterial`
/// was authored. Reads the first handle, applies edits to **all** of them
/// (so one slider recolors the whole rover). Mutates the live assets in place
/// for immediate feedback. Full photometric control: base color, alpha,
/// emissive, metallic, roughness, reflectance, unlit, double-sided.
fn material_pbr_section(
    ui: &mut egui::Ui,
    world: &mut World,
    part: Entity,
    handles: &[Handle<StandardMaterial>],
) {
    let Some(handle) = handles.first().cloned() else {
        return;
    };

    // Snapshot current values — no world borrow held while drawing widgets.
    let snap = {
        let mats = world.resource::<Assets<StandardMaterial>>();
        let Some(m) = mats.get(&handle) else {
            ui.label("Material still loading…");
            return;
        };
        let base = m.base_color.to_linear();
        let e = m.emissive;
        (
            [base.red, base.green, base.blue], // base rgb
            base.alpha,
            [e.red, e.green, e.blue], // emissive rgb
            m.metallic,
            m.perceptual_roughness,
            m.reflectance,
            m.unlit,
            m.double_sided,
        )
    };
    let (mut base, mut alpha, mut emissive, mut metallic, mut roughness, mut reflectance, mut unlit, mut double_sided) = snap;

    let mut changed = false;
    let mut base_changed = false;
    let mut emissive_changed = false;

    ui.horizontal(|ui| {
        let r = ui.color_edit_button_rgb(&mut base);
        changed |= r.changed();
        base_changed |= r.changed();
        ui.label("Base color");
    });
    let alpha_changed = ui.add(egui::Slider::new(&mut alpha, 0.0..=1.0).text("Alpha")).changed();
    changed |= alpha_changed;
    base_changed |= alpha_changed;
    ui.horizontal(|ui| {
        let r = ui.color_edit_button_rgb(&mut emissive);
        changed |= r.changed();
        emissive_changed |= r.changed();
        ui.label("Emissive");
    });
    let metallic_changed = ui.add(egui::Slider::new(&mut metallic, 0.0..=1.0).text("Metallic")).changed();
    changed |= metallic_changed;
    let roughness_changed = ui.add(egui::Slider::new(&mut roughness, 0.0..=1.0).text("Roughness")).changed();
    changed |= roughness_changed;
    let reflectance_changed = ui.add(egui::Slider::new(&mut reflectance, 0.0..=1.0).text("Reflectance")).changed();
    changed |= reflectance_changed;
    changed |= ui.checkbox(&mut unlit, "Unlit").changed();
    changed |= ui.checkbox(&mut double_sided, "Double-sided").changed();
    if handles.len() > 1 {
        ui.label(egui::RichText::new(format!("applies to {} parts", handles.len())).weak());
    }

    if changed {
        if let Some(mut mats) = world.get_resource_mut::<Assets<StandardMaterial>>() {
            for handle in handles {
                let Some(m) = mats.get_mut(handle) else { continue };
                m.base_color = Color::LinearRgba(LinearRgba::new(base[0], base[1], base[2], alpha));
                m.emissive = LinearRgba::new(emissive[0], emissive[1], emissive[2], 1.0);
                m.metallic = metallic;
                m.perceptual_roughness = roughness;
                m.reflectance = reflectance;
                m.unlit = unlit;
                m.double_sided = double_sided;
                m.alpha_mode = if alpha >= 1.0 { AlphaMode::Opaque } else { AlphaMode::Blend };
                m.cull_mode = if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) };
            }
        }

        // Propagate changes to USD. A bound shader prim (resolved via the same
        // material:binding → outputs:surface walk the renderer uses) receives the
        // edits as `inputs:*`; otherwise they're written as `primvars:*` directly
        // on the geometry prim. Only the destination differs per channel, so a
        // single `write` dispatcher keeps the two paths from drifting.
        if let Some(prim) = world.get::<UsdPrimPath>(part).cloned() {
            let shader_path = world
                .get_resource::<Assets<UsdStageAsset>>()
                .and_then(|stages| stages.get(&prim.stage_handle))
                .and_then(|stage| {
                    let mesh_sdf = SdfPath::new(&prim.path).ok()?;
                    resolve_bound_shader(&stage.reader, &mesh_sdf)
                })
                .map(|p| p.to_string());

            let mut write = |attr: &str, ty: &str, value: String| match &shader_path {
                Some(sp) => apply_usd_path_attribute_change(world, part, sp.clone(), attr, ty, value),
                None => apply_usd_attribute_change(world, part, attr, ty, value),
            };

            // displayColor lives directly on the geometry as an array primvar;
            // a shader carries it as the scalar `inputs:diffuseColor`.
            if base_changed {
                let value = format!("({}, {}, {})", base[0], base[1], base[2]);
                if shader_path.is_some() {
                    write("inputs:diffuseColor", "color3f", value);
                } else {
                    write(
                        "primvars:displayColor",
                        "color3f[]",
                        format!("[({}, {}, {})]", base[0], base[1], base[2]),
                    );
                }
            }
            if emissive_changed {
                let attr = if shader_path.is_some() { "inputs:emissiveColor" } else { "primvars:emissiveColor" };
                write(attr, "color3f", format!("({}, {}, {})", emissive[0], emissive[1], emissive[2]));
            }
            if metallic_changed {
                write("inputs:metallic", "float", format!("{:.3}", metallic));
            }
            if roughness_changed {
                write("inputs:roughness", "float", format!("{:.3}", roughness));
            }
            if reflectance_changed {
                write("inputs:reflectance", "float", format!("{:.3}", reflectance));
            }
        }
    }
}

/// Render named, range-bounded controls for the selected entity's
/// [`ShaderMaterial`](lunco_materials::ShaderMaterial) generic uniforms.
///
/// Labels, ranges, and defaults come from the manifest in `lunco-materials`
/// (keyed by the shader's file name), so this stays in sync with each
/// `.wgsl` header and needs no per-shader code here. A stored uniform of 0
/// means "unset" — the control shows the manifest default (matching the
/// shader's own fallback) until the user moves it. Edits mutate the live
/// material asset in place for immediate feedback, the same path the
/// Transform/Physics sections use.
fn shader_parameters_section(ui: &mut egui::Ui, world: &mut World, entity: Entity) {
    use lunco_materials::{ParamType, ParamValue, ShaderMaterial, UiKind};

    let Ok(handle) = world
        .query::<&MeshMaterial3d<ShaderMaterial>>()
        .get(world, entity)
        .map(|m| m.0.clone())
    else {
        return;
    };

    // Snapshot the reflected schema + each field's current display value.
    // Engine-filled fields are hidden. `mat.get` already falls back to the
    // field's reflected default.
    struct Row {
        name: String,
        label: String,
        ui: UiKind,
        ty: ParamType,
        scalar: f32,
        int: i32,
        color: [f32; 3],
    }
    let rows: Vec<Row> = {
        let mats = world.resource::<Assets<ShaderMaterial>>();
        let Some(mat) = mats.get(&handle) else {
            ui.label("Material still loading…");
            return;
        };
        mat.schema
            .fields
            .iter()
            .filter(|f| !matches!(f.ui, UiKind::Engine))
            .map(|f| {
                let floats = mat.get(&f.name).map(|v| v.as_floats()).unwrap_or_default();
                Row {
                    name: f.name.clone(),
                    label: f.label.clone(),
                    ui: f.ui.clone(),
                    ty: f.ty,
                    scalar: floats.first().copied().unwrap_or(0.0),
                    int: floats.first().copied().unwrap_or(0.0).round() as i32,
                    color: [
                        floats.first().copied().unwrap_or(0.5),
                        floats.get(1).copied().unwrap_or(0.5),
                        floats.get(2).copied().unwrap_or(0.5),
                    ],
                }
            })
            .collect()
    };

    // (The shader picker lives in `shader_picker_for_part`, rendered above this
    // section so it works on any targeted part — including converting a PBR
    // part — not just entities with an API id.)
    if rows.is_empty() {
        ui.label("No editable parameters.");
        return;
    }

    // Draw; collect edits as typed values (matching each field's WGSL type so
    // packing writes the right width). No world borrow held while drawing.
    let mut edits: Vec<(String, ParamValue)> = Vec::new();
    for mut row in rows {
        match row.ui {
            UiKind::Slider { min, max } => {
                if ui.add(egui::Slider::new(&mut row.scalar, min..=max).text(&row.label)).changed() {
                    edits.push((row.name, ParamValue::F32(row.scalar)));
                }
            }
            UiKind::Int { min, max } => {
                if ui.add(egui::Slider::new(&mut row.int, min..=max).text(&row.label)).changed() {
                    let v = match row.ty {
                        ParamType::U32 => ParamValue::U32(row.int.max(0) as u32),
                        ParamType::F32 => ParamValue::F32(row.int as f32),
                        _ => ParamValue::I32(row.int),
                    };
                    edits.push((row.name, v));
                }
            }
            UiKind::Color => {
                ui.horizontal(|ui| {
                    if ui.color_edit_button_rgb(&mut row.color).changed() {
                        let v = if row.ty == ParamType::Vec3 {
                            ParamValue::Vec3(row.color)
                        } else {
                            ParamValue::Vec4([row.color[0], row.color[1], row.color[2], 1.0])
                        };
                        edits.push((row.name, v));
                    }
                    ui.label(&row.label);
                });
            }
            UiKind::Free | UiKind::Engine => {
                ui.horizontal(|ui| {
                    if ui.add(egui::DragValue::new(&mut row.scalar).speed(0.01)).changed() {
                        edits.push((row.name, ParamValue::F32(row.scalar)));
                    }
                    ui.label(&row.label);
                });
            }
        }
    }

    // Apply to the live material asset (one re-upload).
    if !edits.is_empty() {
        let usd_prim_exists = world.get::<UsdPrimPath>(entity).is_some();
        if let Some(mut mats) = world.get_resource_mut::<Assets<ShaderMaterial>>() {
            if let Some(mat) = mats.get_mut(&handle) {
                for (name, v) in edits.iter() {
                    mat.set(name, *v);
                }
            }
        }

        // Propagate changes to USD
        if usd_prim_exists {
            for (name, v) in edits {
                let usd_name = if name.starts_with("primvars:") {
                    name.clone()
                } else {
                    format!("primvars:{}", name)
                };
                let (type_name, value_str) = match v {
                    ParamValue::F32(x) => ("float", format!("{:.3}", x)),
                    ParamValue::I32(x) => ("int", format!("{}", x)),
                    ParamValue::U32(x) => ("uint", format!("{}", x)),
                    ParamValue::Vec2(arr) => ("float2", format!("({}, {})", arr[0], arr[1])),
                    ParamValue::Vec3(arr) => {
                        let t = if name.to_lowercase().contains("color") || name.to_lowercase().contains("colour") {
                            "color3f"
                        } else {
                            "float3"
                        };
                        (t, format!("({}, {}, {})", arr[0], arr[1], arr[2]))
                    }
                    ParamValue::Vec4(arr) => ("float4", format!("({}, {}, {}, {})", arr[0], arr[1], arr[2], arr[3])),
                };
                apply_usd_attribute_change(world, entity, &usd_name, type_name, value_str);
            }
        }
    }
}

/// Render editable sliders for every tunable `parameter Real` in the
/// entity's Modelica model. On any change, dispatch a
/// [`ModelicaOp::SetParameter`] through the canonical op pipeline
/// (which span-patches the source, refreshes the AST cache, patches
/// the index, and journals) and signal the worker to recompile.
fn modelica_parameters_section(
    ui: &mut egui::Ui,
    world: &mut World,
    entity: Entity,
) {
    use lunco_modelica::state::ModelicaDocumentRegistry;
    // Use the canvas_diagram-level re-export, not the private `ops`
    // submodule path. Submodules of canvas_diagram (ops, projection,
    // panel, …) are crate-private encapsulation; the public surface
    // is the items re-exported at canvas_diagram's mod root.
    use lunco_modelica::ui::panels::canvas_diagram::apply_ops_public;
    use lunco_modelica::document::ModelicaOp;
    use lunco_modelica::{ModelicaChannels, ModelicaCommand, ModelicaModel};

    // Snapshot the current params so we can render stable sliders
    // without holding a mutable borrow across the UI.
    let (params, model_name) = match world.query::<&ModelicaModel>().get(world, entity) {
        Ok(m) => (m.parameters.clone(), m.model_name.clone()),
        Err(_) => return,
    };
    if params.is_empty() {
        ui.label(egui::RichText::new("(no tunable parameters)").weak().small());
        return;
    }

    // Sorted keys for a stable display order.
    let mut keys: Vec<String> = params.keys().cloned().collect();
    keys.sort();

    let mut changed_pair: Option<(String, f64)> = None;
    for key in &keys {
        let current = params.get(key).copied().unwrap_or(0.0);
        let mut v = current;
        ui.horizontal(|ui| {
            ui.label(format!("{key:14}"));
            if ui
                .add(
                    egui::DragValue::new(&mut v)
                        .speed(0.01)
                        .fixed_decimals(3),
                )
                .changed()
            {
                changed_pair = Some((key.clone(), v));
            }
        });
    }

    let Some((changed_key, new_value)) = changed_pair else { return };

    // Mirror the new value into ECS state for instant slider feedback;
    // bump session id so the worker treats this as a fresh stepping
    // generation.
    let mut session_id = 0u64;
    if let Ok(mut m) = world.query::<&mut ModelicaModel>().get_mut(world, entity) {
        if let Some(slot) = m.parameters.get_mut(&changed_key) {
            *slot = new_value;
        }
        m.session_id += 1;
        session_id = m.session_id;
        m.is_stepping = true;
    }

    // Resolve doc id + root class from the registry.
    let (doc_id, class_name) = {
        let registry = world.resource::<ModelicaDocumentRegistry>();
        let doc = registry.document_of(entity);
        let class = doc
            .and_then(|d| registry.host(d))
            .and_then(|h| {
                lunco_modelica::ast_extract::extract_model_name_from_ast(
                    h.document().syntax().ast(),
                )
            });
        (doc, class)
    };
    let (Some(doc_id), Some(class_name)) = (doc_id, class_name) else { return };

    // Dispatch through the canonical op pipeline. `param: ""` is the
    // sentinel for the component's primary binding (the `= expr` after
    // the name), which is what top-level `parameter Real k = 5;`
    // declarations need.
    apply_ops_public(
        world,
        doc_id,
        vec![ModelicaOp::SetParameter {
            class: class_name,
            component: changed_key,
            param: String::new(),
            value: format!("{new_value}"),
        }],
    );

    // Pull the freshly-patched source back out and signal the worker
    // to recompile. The op pipeline already updated the source +
    // generation; this just hands the worker the new bytes.
    let new_source = world
        .resource::<ModelicaDocumentRegistry>()
        .host(doc_id)
        .map(|h| h.document().source().to_string());
    if let (Some(new_source), Some(channels)) =
        (new_source, world.get_resource::<ModelicaChannels>())
    {
        let _ = channels.tx.send(ModelicaCommand::UpdateParameters {
            entity,
            session_id,
            model_name,
            source: new_source,
        });
    }
}

/// Dispatch a `UsdOp::SetAttribute` command to write changes back to the USD document for a specific prim path.
fn apply_usd_path_attribute_change(
    world: &mut World,
    entity: Entity,
    prim_path: String,
    name: &str,
    type_name: &str,
    value: String,
) {
    let Some(prim) = world.get::<UsdPrimPath>(entity).cloned() else {
        return;
    };
    let Some(asset_server) = world.get_resource::<AssetServer>() else {
        return;
    };
    let Some(asset_path) = asset_server.get_path(prim.stage_handle.id()) else {
        return;
    };
    let path_str = asset_path.path().to_string_lossy();

    let Some(usd_registry) = world.get_resource::<UsdDocumentRegistry>() else {
        return;
    };
    let doc_id = usd_registry.ids().find(|id| {
        if let Some(h) = usd_registry.host(*id) {
            match h.document().origin() {
                DocumentOrigin::File { path, .. } => {
                    path.to_string_lossy().ends_with(&*path_str)
                }
                _ => false,
            }
        } else {
            false
        }
    });

    let doc_id = doc_id.or_else(|| {
        world
            .get_resource::<UsdViewportState>()
            .and_then(|v| v.active_doc())
    });

    if let Some(doc) = doc_id {
        let op = UsdOp::SetAttribute {
            edit_target: LayerId::root(),
            path: prim_path,
            name: name.to_string(),
            type_name: type_name.to_string(),
            value,
        };
        world.trigger(ApplyUsdOp { doc, op });
    }
}

/// Dispatch a `UsdOp::SetAttribute` command to write changes back to the USD document.
fn apply_usd_attribute_change(
    world: &mut World,
    entity: Entity,
    name: &str,
    type_name: &str,
    value: String,
) {
    if let Some(prim) = world.get::<UsdPrimPath>(entity).cloned() {
        apply_usd_path_attribute_change(world, entity, prim.path, name, type_name, value);
    }
}

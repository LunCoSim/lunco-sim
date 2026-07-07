//! Inspector panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Provides editable sliders for transform, physics, and wheel parameters.
//!
//! **WP-8 reactive shape:** `render` takes a capability-narrowed
//! [`PanelCtx`] — no raw `&mut World`. Reads go through
//! [`PanelCtx::resource`]/[`PanelCtx::get`] (and, for query-derived data
//! like the scene sun / camera / joint, the change-driven
//! [`InspectorView`] view-model produced by [`populate_inspector_view`]);
//! every mutation is queued through [`PanelCtx::defer`] and applied after
//! the egui pass.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};
use lunco_mobility::WheelRaycast;
use lunco_cosim::{joint_angle_holder, JOINT_ANGLE_PORT};
use lunco_core::ports::PortRegistry;

use lunco_obstacle_field::{ObstacleFieldSpec, Pattern, plugin::UpdateObstacleFieldSpec};

use crate::{SelectedEntities, UndoStack, UndoAction};
use lunco_usd::document::{UsdOp, LayerId};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd::ui::viewport::UsdViewportState;
use lunco_doc::DocumentOrigin;
use lunco_usd_bevy::{UsdPrimPath, UsdStageAsset, SdfPath, resolve_bound_shader};

// ─────────────────────────────────────────────────────────────────────
// View-model (WP-8) — query-derived inspector state.
// ─────────────────────────────────────────────────────────────────────

/// Live scene-sun readout for the Environment section.
#[derive(Clone)]
pub struct SunReadout {
    pub name: String,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub illuminance: f32,
    pub shadows_enabled: bool,
    pub rgb: [f32; 3],
    pub shadow_first: Option<f32>,
    pub shadow_max: Option<f32>,
}

/// Live joint readout for the selected entity's `angle` port.
#[derive(Clone, Copy)]
pub struct JointReadout {
    pub holder: Entity,
    pub measured: f64,
    pub commanded: f64,
    pub wired: bool,
}

/// Change-driven view-model for the Inspector (WP-8). The Environment,
/// Camera, and Joint sections read query-derived world state that
/// [`PanelCtx`] deliberately can't gather during paint (no `query`, no
/// `&World`); [`populate_inspector_view`] flattens it here each frame and
/// the panel reads it via `ctx.resource`.
#[derive(Resource, Default)]
pub struct InspectorView {
    /// First scene sun (directional light), if any.
    pub sun: Option<SunReadout>,
    /// Global ambient brightness, if the resource exists.
    pub ambient_brightness: Option<f32>,
    /// Earthshine fill-light illuminance, if present.
    pub earthshine_lux: Option<f32>,
    /// First camera's exposure EV100, if any.
    pub exposure_ev100: Option<f32>,
    /// First camera's bloom intensity, if any.
    pub bloom_intensity: Option<f32>,
    /// Joint readout for the primary-selected entity, if it drives one.
    pub joint: Option<JointReadout>,
}

/// Producer for [`InspectorView`]. Exclusive (needs `&mut World` for the
/// scans + `joint_angle_holder`); runs in `Update` before the egui pass,
/// gated by [`inspector_inputs_changed`] so the world scans are skipped on
/// a quiescent scene. All reads are bounded single-entity lookups or small
/// scans the panel used to do in-paint.
pub fn populate_inspector_view(world: &mut World) {
    use bevy::camera::Exposure;
    use bevy::camera::visibility::RenderLayers;
    use bevy::light::{CascadeShadowConfig, DirectionalLight, GlobalAmbientLight};
    use bevy::post_process::bloom::Bloom;

    // ── Scene sun (skip preview / earthshine lights, same rule as the
    // horizon system's pick_sun).
    let suns: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<DirectionalLight>,
            Without<RenderLayers>,
            Without<lunco_environment::Earthshine>,
        )>()
        .iter(world)
        .collect();
    let sun = suns.first().copied().map(|e| {
        let name = world
            .get::<Name>(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        let (yaw_deg, pitch_deg) = world
            .get::<Transform>(e)
            .map(|tf| {
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                (yaw.to_degrees(), pitch.to_degrees())
            })
            .unwrap_or((0.0, 0.0));
        let (illuminance, shadows_enabled, rgb) = world
            .get::<DirectionalLight>(e)
            .map(|l| {
                let lin = l.color.to_linear();
                (l.illuminance, l.shadows_enabled, [lin.red, lin.green, lin.blue])
            })
            .unwrap_or((0.0, false, [1.0, 1.0, 1.0]));
        let (shadow_first, shadow_max) = world
            .get::<CascadeShadowConfig>(e)
            .map(|cfg| {
                (
                    Some(cfg.bounds.first().copied().unwrap_or(40.0)),
                    Some(cfg.bounds.last().copied().unwrap_or(1500.0)),
                )
            })
            .unwrap_or((None, None));
        SunReadout {
            name,
            yaw_deg,
            pitch_deg,
            illuminance,
            shadows_enabled,
            rgb,
            shadow_first,
            shadow_max,
        }
    });

    let ambient_brightness = world.get_resource::<GlobalAmbientLight>().map(|a| a.brightness);
    let earthshine_lux = world
        .query_filtered::<&DirectionalLight, With<lunco_environment::Earthshine>>()
        .iter(world)
        .next()
        .map(|l| l.illuminance);

    // ── Camera.
    let exposure_ev100 = world.query::<&Exposure>().iter(world).next().map(|e| e.ev100);
    let bloom_intensity = world.query::<&Bloom>().iter(world).next().map(|b| b.intensity);

    // ── Joint for the primary-selected entity.
    let selected = world
        .get_resource::<SelectedEntities>()
        .and_then(|s| s.primary());
    let joint = if let Some(entity) = selected {
        if let Some(holder) = joint_angle_holder(world, entity) {
            let registry = world.resource::<PortRegistry>().clone();
            let measured = registry.read_output_port(world, holder, JOINT_ANGLE_PORT).unwrap_or(0.0);
            let commanded = registry.read_input_port(world, holder, JOINT_ANGLE_PORT).unwrap_or(0.0);
            let mut cq = world.query::<&lunco_cosim::SimConnection>();
            let wired = cq
                .iter(world)
                .any(|c| c.end_element == holder && c.end_connector == JOINT_ANGLE_PORT);
            Some(JointReadout { holder, measured, commanded, wired })
        } else {
            None
        }
    } else {
        None
    };

    let mut view = world.resource_mut::<InspectorView>();
    view.sun = sun;
    view.ambient_brightness = ambient_brightness;
    view.earthshine_lux = earthshine_lux;
    view.exposure_ev100 = exposure_ev100;
    view.bloom_intensity = bloom_intensity;
    view.joint = joint;
}

/// Run condition for [`populate_inspector_view`]: skip the world scans on a
/// quiescent scene, the way the sibling [`super::entity_list::populate_entity_tree_view`]
/// gates on [`super::entity_list::scene_topology_changed`]. Runs when any
/// readout the Inspector shows could have changed — the selection moved, the
/// scene sun / camera exposure / bloom / ambient was edited, or a directional
/// light was removed (despawn) — and keeps running every frame while a joint
/// readout is live (`view.joint.is_some()`) so the measured angle stays fresh
/// during a sim. The `Local` flag forces one initial build (a freshly-added
/// system does not see pre-existing entities as `Changed`). On an idle scene
/// with nothing selected this returns `false` and every scan is skipped.
pub(crate) fn inspector_inputs_changed(
    mut first: Local<bool>,
    view: Res<InspectorView>,
    selection: Res<crate::SelectedEntities>,
    ambient: Option<Res<bevy::light::GlobalAmbientLight>>,
    lights: Query<
        (),
        (
            Or<(
                Changed<Transform>,
                Changed<bevy::light::DirectionalLight>,
                Changed<bevy::light::CascadeShadowConfig>,
            )>,
            With<bevy::light::DirectionalLight>,
        ),
    >,
    cameras: Query<
        (),
        Or<(
            Changed<bevy::camera::Exposure>,
            Changed<bevy::post_process::bloom::Bloom>,
        )>,
    >,
    mut removed_lights: RemovedComponents<bevy::light::DirectionalLight>,
) -> bool {
    // Drain the removal buffer every frame (so it doesn't accumulate) and note
    // whether a directional light despawned since last frame.
    let removed = removed_lights.read().count() > 0;
    let run = !*first
        || selection.is_changed()
        || ambient.is_some_and(|a| a.is_changed())
        || !lights.is_empty()
        || !cameras.is_empty()
        || removed
        || view.joint.is_some();
    *first = true;
    run
}

/// Inspector panel — editable entity parameters.
pub struct Inspector;

impl Panel for Inspector {
    fn id(&self) -> PanelId { PanelId("sandbox_inspector") }
    fn title(&self) -> String { "Inspector".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let mantle = ctx.resource_expect::<lunco_theme::Theme>().colors.mantle;
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| {
                // The Inspector stacks many sections (Environment, Transform,
                // Physics, Wheel, Shader, Material, Modelica) and can exceed the
                // panel height — scroll so the lower sections stay reachable.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| inspector_content(self, ui, ctx));
            });
    }
}

fn inspector_content(_panel: &mut Inspector, ui: &mut egui::Ui, ctx: &mut PanelCtx) {

        // Delete hotkey
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            let primary = ctx
                .resource::<SelectedEntities>()
                .and_then(|s| s.primary());
            if let Some(entity) = primary {
                ctx.defer(move |world| {
                    if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                        undo.push(UndoAction::Spawned { entity });
                    }
                    if world.get_entity(entity).is_ok() {
                        world.despawn(entity);
                    }
                    if let Some(mut selected) = world.get_resource_mut::<SelectedEntities>() {
                        selected.entities.retain(|e| *e != entity);
                    }
                });
                return;
            }
        }

        // Esc / Backspace deselection lives in the Bevy `handle_entity_selection`
        // system (the single mutation path), not here.

        ui.heading("Inspector");

        // ── Environment (sun + ambient) ──────────────────────────────
        egui::CollapsingHeader::new("Environment (Sun & Ambient)")
            .default_open(false)
            .show(ui, |ui| environment_section(ui, ctx));
        ui.separator();

        // ── Animation transport (play/pause/scrub/rate) ──────────────
        egui::CollapsingHeader::new("Animation")
            .default_open(false)
            .show(ui, |ui| animation_transport_section(ui, ctx));
        ui.separator();

        // ── Camera (exposure + post-process) ─────────────────────────
        egui::CollapsingHeader::new("Camera")
            .default_open(false)
            .show(ui, |ui| camera_section(ui, ctx));
        ui.separator();

        // ── Obstacle Field (procedural craters + rocks) ──────────────
        egui::CollapsingHeader::new("Obstacle Field (Craters & Rocks)")
            .default_open(true)
            .show(ui, |ui| obstacle_field_section(ui, ctx));
        ui.separator();

        // ── Terrain LOD (runtime streaming knobs) ────────────────────
        egui::CollapsingHeader::new("Terrain LOD")
            .default_open(true)
            .show(ui, |ui| terrain_lod_section(ui, ctx));
        ui.separator();

        // Get current selection
        let Some(entity) = ctx.resource::<SelectedEntities>().and_then(|s| s.primary()) else {
            ui.label("No entity selected.");
            ui.label("Press Shift+Left-click on an object to select it.");
            return;
        };

        ui.label(format!("ID: {entity:?}"));

        // Name (read-only)
        if let Some(name) = ctx.get::<Name>(entity).map(|n| n.as_str().to_string()) {
            ui.label(format!("Name: {name}"));
        }

        ui.separator();

        // ── Comms & Orbit (doc 43): geodetic anchor / Kepler orbit /
        //    antenna params + live link state ─────────────────────────
        comms_orbit_section(ui, ctx, entity);

        // ── Transform component ──────────────────────────────────────
        if ctx.get::<Transform>(entity).is_some() {
            egui::CollapsingHeader::new("Transform")
                .default_open(true)
                .show(ui, |ui| {
                    if let Some((old_tf, new_vals)) =
                        ctx.get::<Transform>(entity).map(|tf| {
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
                            let (old_t, old_r) = old_tf;
                            ctx.defer(move |world| {
                                if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                                    undo.push(UndoAction::TransformChanged {
                                        entity,
                                        old_translation: old_t,
                                        old_rotation: old_r,
                                    });
                                }
                                let new_t = Vec3::new(x, y, z);
                                if let Some(mut tf) = world.get_mut::<Transform>(entity) {
                                    tf.translation = new_t;
                                }
                                // CQ-510: on a physics body avian re-derives
                                // Transform from its f64 `Position` every tick,
                                // so writing only Transform silently no-ops.
                                // Mirror `MoveEntity`: seat `Position` and force
                                // Kinematic so the new pose is authoritative.
                                if let Some(mut pos) =
                                    world.get_mut::<avian3d::physics_transform::Position>(entity)
                                {
                                    pos.0 = new_t.as_dvec3();
                                }
                                if world.get::<avian3d::prelude::RigidBody>(entity).is_some() {
                                    world
                                        .entity_mut(entity)
                                        .insert(avian3d::prelude::RigidBody::Kinematic);
                                }
                            });
                        }
                    }
                });
        }

        // ── Physics component ────────────────────────────────────────
        let has_physics = ctx.get::<avian3d::prelude::RigidBody>(entity).is_some()
            || ctx.get::<avian3d::prelude::Mass>(entity).is_some()
            || ctx.get::<avian3d::prelude::LinearDamping>(entity).is_some()
            || ctx.get::<avian3d::prelude::AngularDamping>(entity).is_some();
        if has_physics {
            egui::CollapsingHeader::new("Physics")
                .default_open(false)
                .show(ui, |ui| {
                    if let Some(rb) = ctx.get::<avian3d::prelude::RigidBody>(entity).map(|rb| format!("{rb:?}")) {
                        ui.label(format!("Type: {rb}"));
                    }
                    if let Some(cur) = ctx.get::<avian3d::prelude::Mass>(entity).map(|c| c.0) {
                        let mut m = cur;
                        if ui.add(egui::Slider::new(&mut m, 0.1..=100000.0).text("Mass (kg)").logarithmic(true)).changed() {
                            ctx.defer(move |world| {
                                if let Some(mut mass) = world.get_mut::<avian3d::prelude::Mass>(entity) {
                                    mass.0 = m;
                                }
                            });
                        }
                    }
                    if let Some(cur) = ctx.get::<avian3d::prelude::LinearDamping>(entity).map(|c| c.0 as f32) {
                        let mut d = cur;
                        if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Linear Damping")).changed() {
                            ctx.defer(move |world| {
                                if let Some(mut damp) = world.get_mut::<avian3d::prelude::LinearDamping>(entity) {
                                    damp.0 = d as f64;
                                }
                            });
                        }
                    }
                    if let Some(cur) = ctx.get::<avian3d::prelude::AngularDamping>(entity).map(|c| c.0 as f32) {
                        let mut d = cur;
                        if ui.add(egui::Slider::new(&mut d, 0.0..=10.0).text("Angular Damping")).changed() {
                            ctx.defer(move |world| {
                                if let Some(mut damp) = world.get_mut::<avian3d::prelude::AngularDamping>(entity) {
                                    damp.0 = d as f64;
                                }
                            });
                        }
                    }
                });
        }

        // ── Wheel (Raycast) component ────────────────────────────────
        if ctx.get::<WheelRaycast>(entity).is_some() {
            egui::CollapsingHeader::new("Wheel (Raycast)")
                .default_open(false)
                .show(ui, |ui| {
                    if let Some((rest0, k0, d0, r0)) = ctx.get::<WheelRaycast>(entity).map(|w| {
                        (
                            w.rest_length as f32,
                            w.spring_k as f32,
                            w.damping_c as f32,
                            w.wheel_radius as f32,
                        )
                    }) {
                        let mut rest = rest0;
                        let mut k = k0;
                        let mut d = d0;
                        let mut radius = r0;

                        let rest_changed = ui.add(egui::Slider::new(&mut rest, 0.1..=2.0).text("Rest Length (m)")).changed();
                        let k_changed = ui.add(egui::Slider::new(&mut k, 100.0..=100000.0).text("Spring K (N/m)").logarithmic(true)).changed();
                        let d_changed = ui.add(egui::Slider::new(&mut d, 100.0..=10000.0).text("Damping C (N·s/m)").logarithmic(true)).changed();
                        let r_changed = ui.add(egui::Slider::new(&mut radius, 0.1..=2.0).text("Wheel Radius (m)")).changed();

                        if rest_changed || k_changed || d_changed || r_changed {
                            ctx.defer(move |world| {
                                if let Some(mut wheel) = world.get_mut::<WheelRaycast>(entity) {
                                    if rest_changed { wheel.rest_length = rest as f64; }
                                    if k_changed { wheel.spring_k = k as f64; }
                                    if d_changed { wheel.damping_c = d as f64; }
                                    if r_changed { wheel.wheel_radius = radius as f64; }
                                }
                            });
                        }
                    }
                });
        }

        // ── Materials ────────────────────────────────────────────────
        let parts = editable_parts(ctx, entity);
        if !parts.is_empty() {
            let stored = ctx
                .resource::<crate::InspectorTarget>()
                .and_then(|t| t.part)
                .filter(|p| parts.iter().any(|(e, _)| e == p));
            let mut target = stored.or_else(|| default_part(ctx, &parts));
            if stored.is_none() {
                if let Some(t) = target {
                    ctx.defer(move |world| {
                        world.resource_mut::<crate::InspectorTarget>().part = Some(t);
                    });
                }
            }
            // Multi-part object → a dropdown to switch parts (may retarget).
            if parts.len() > 1 {
                target = parts_selector(ui, ctx, &parts, target);
            }

            if let Some(part) = target {
                shader_picker_for_part(ui, ctx, part);
                shader_tools_ui(ui, ctx, part);

                // One subtree pass yields both the shader holder and the
                // distinct PBR material handles (CQ-204: was two independent
                // `subtree` walks of the same part — `first_shader_holder` +
                // `collect_std_handles`).
                let (std_handles, shader_holder) = part_materials(ctx, part);
                if let Some(holder) = shader_holder {
                    egui::CollapsingHeader::new("Shader Parameters")
                        .default_open(true)
                        .show(ui, |ui| {
                            shader_parameters_section(ui, ctx, holder);
                        });
                }
                if !std_handles.is_empty() {
                    egui::CollapsingHeader::new("Material (PBR)")
                        .default_open(true)
                        .show(ui, |ui| {
                            material_pbr_section(ui, ctx, part, &std_handles);
                        });
                }
            }
        }

        // ── Terrain shader mode (streamed DEM terrain) ──────────────
        if let Some(mode) = ctx.get::<lunco_terrain_surface::TerrainShaderMode>(entity).copied() {
            use lunco_terrain_surface::TerrainShaderMode as M;
            egui::CollapsingHeader::new("Terrain Shader")
                .default_open(true)
                .show(ui, |ui| {
                    let label = |m: M| match m {
                        M::Lit => "Lit (regolith)",
                        M::DebugLod => "Debug LOD (colours)",
                        M::Plain => "Plain (no shader)",
                    };
                    let mut sel = mode;
                    egui::ComboBox::from_label("Mode")
                        .selected_text(label(sel))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut sel, M::Lit, label(M::Lit));
                            ui.selectable_value(&mut sel, M::DebugLod, label(M::DebugLod));
                            ui.selectable_value(&mut sel, M::Plain, label(M::Plain));
                        });
                    if sel != mode {
                        ctx.defer(move |world| {
                            if let Some(mut m) =
                                world.get_mut::<lunco_terrain_surface::TerrainShaderMode>(entity)
                            {
                                *m = sel;
                            }
                        });
                    }
                });
        }

        // ── Modelica parameters component ───────────────────────────
        let has_modelica = ctx.get::<lunco_modelica::ModelicaModel>(entity).is_some();
        if has_modelica {
            egui::CollapsingHeader::new("Modelica Parameters")
                .default_open(true)
                .show(ui, |ui| {
                    modelica_parameters_section(ui, ctx, entity);
                });
        }

        // ── Joint control ───────────────────────────────────────────
        let joint = ctx.resource::<InspectorView>().and_then(|v| v.joint);
        if let Some(j) = joint {
            egui::CollapsingHeader::new("Joint")
                .default_open(true)
                .show(ui, |ui| {
                    joint_control_section(ui, ctx, j);
                });
        }

        // Delete button
        ui.separator();
        if ui.button("🗑 Delete Entity (Del)").clicked() {
            ctx.defer(move |world| {
                if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                    undo.push(UndoAction::Spawned { entity });
                }
                if world.get_entity(entity).is_ok() {
                    world.despawn(entity);
                }
                if let Some(mut selected) = world.get_resource_mut::<SelectedEntities>() {
                    selected.entities.retain(|e| *e != entity);
                }
            });
        }
    }

/// Live sun + ambient controls. Reads the change-driven [`InspectorView`]
/// snapshot and dispatches every edit through a single
/// [`SetEnvironmentLight`](lunco_environment::SetEnvironmentLight) command
/// — the same mutation path the HTTP/MCP API uses.
/// Animation transport for the USD animation-preview domain (doc 19 — T7).
/// Reads the singleton [`lunco_time::AnimationPreview`]'s [`lunco_time::Playback`]
/// and drives it through the [`lunco_time::ControlAnimation`] command (the same
/// authority the API/MCP use), so play/pause/scrub/rate touch only animation —
/// never the physics clock.
fn animation_transport_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_time::{AnimationPreview, ControlAnimation, Playback, TransportMode};

    let Some(domain) = ctx.resource::<AnimationPreview>().map(|p| p.domain) else {
        ui.label("Animation spine not active.");
        return;
    };
    let Some(pb) = ctx.get::<Playback>(domain).copied() else {
        ui.label("No animation timeline yet.");
        return;
    };
    let playing = matches!(pb.mode, TransportMode::Playing);

    ui.horizontal(|ui| {
        if ui.button(if playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
            ctx.trigger(ControlAnimation { playing: Some(!playing), ..Default::default() });
        }
        if ui.button("⏮ Rewind").clicked() {
            ctx.trigger(ControlAnimation { seek_secs: Some(0.0), ..Default::default() });
        }
    });

    // Scrub the playhead (seconds) over the bound clips' authored span (set by
    // `bind_animated_to_preview`); fall back to a default window when no clip has
    // bound yet (so the bar is still usable). Pausing first lets the slider hold.
    let range = if pb.bounded() { pb.start..=pb.end } else { 0.0..=120.0 };
    let mut head = pb.head;
    if ui
        .add(egui::Slider::new(&mut head, range).text("Time (s)"))
        .changed()
    {
        ctx.trigger(ControlAnimation { seek_secs: Some(head), ..Default::default() });
    }

    // Playback rate (1× = realtime). 0 freezes without changing the play flag.
    let mut rate = pb.rate;
    if ui
        .add(egui::Slider::new(&mut rate, 0.0..=10.0).text("Rate ×"))
        .changed()
    {
        ctx.trigger(ControlAnimation { rate: Some(rate), ..Default::default() });
    }

    ui.label("Animation only — the physics clock is the toolbar ⏸.");
}

fn environment_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_environment::SetEnvironmentLight;

    let sun = ctx.resource::<InspectorView>().and_then(|v| v.sun.clone());
    let ambient = ctx.resource::<InspectorView>().and_then(|v| v.ambient_brightness);
    let earthshine = ctx.resource::<InspectorView>().and_then(|v| v.earthshine_lux);
    if sun.is_none() && ambient.is_none() && earthshine.is_none() {
        return;
    }

    let mut cmd = SetEnvironmentLight::default();
    let mut any_change = false;

    egui::CollapsingHeader::new("Environment")
        .default_open(true)
        .show(ui, |ui| {
            if let Some(s) = &sun {
                if !s.name.is_empty() {
                    ui.label(egui::RichText::new(&s.name).strong());
                }

                let mut yaw_deg = s.yaw_deg;
                let mut pitch_deg = s.pitch_deg;
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

                let mut lux = s.illuminance;
                let mut shadows = s.shadows_enabled;
                let mut rgb = s.rgb;
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

                if let (Some(f0), Some(m0)) = (s.shadow_first, s.shadow_max) {
                    let mut first = f0;
                    let mut max = m0;
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

            if let Some(b0) = ambient {
                let mut b = b0;
                if ui
                    .add(egui::Slider::new(&mut b, 0.0..=400.0).text("Ambient (cd/m²)"))
                    .changed()
                {
                    cmd.ambient_brightness = Some(b);
                    any_change = true;
                }
            }

            if let Some(es0) = earthshine {
                let mut lux = es0;
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
        ctx.defer(move |world| {
            world.trigger(cmd);
        });
    }
}

/// Camera section — physical exposure and bloom. Reads the
/// [`InspectorView`] snapshot; mutates via the same
/// [`SetEnvironmentLight`](lunco_environment::SetEnvironmentLight) command.
fn camera_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_environment::SetEnvironmentLight;

    let exposure = ctx.resource::<InspectorView>().and_then(|v| v.exposure_ev100);
    let bloom = ctx.resource::<InspectorView>().and_then(|v| v.bloom_intensity);

    let mut cmd = SetEnvironmentLight::default();
    let mut any_change = false;

    if let Some(ev0) = exposure {
        let mut ev = ev0;
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

    if let Some(i0) = bloom {
        let mut i = i0;
        if ui
            .add(egui::Slider::new(&mut i, 0.0..=1.0).text("Bloom intensity"))
            .changed()
        {
            cmd.bloom_intensity = Some(i);
            any_change = true;
        }
    }

    if any_change {
        ctx.defer(move |world| {
            world.trigger(cmd);
        });
    }
}

/// Live tuning for the procedural obstacle field. Edits the
/// `ObstacleFieldSpec` resource via [`PanelCtx::resource_scope`] (the
/// narrow mutate-during-paint surface); the field rebuilds only on slider
/// release / button press.
/// Runtime LOD knobs for streamed DEM terrain — detail-vs-distance + load
/// smoothness, applied live (no rebuild). Edits the global `TerrainLodConfig`.
fn terrain_lod_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_terrain_surface::TerrainLodConfig;
    if ctx.resource::<TerrainLodConfig>().is_none() {
        ui.label("No streaming terrain in this scene.");
        return;
    }
    ctx.resource_scope(|_ctx, cfg: &mut TerrainLodConfig| {
        ui.add(egui::Slider::new(&mut cfg.pixel_error, 0.5..=16.0).text("Pixel error (px)"))
            .on_hover_text(
                "Screen-space error at which a tile refines (canonical viewport). \
                 Lower = finer tiles wherever the surface earns it (rims, peaks).",
            );
        ui.add(egui::Slider::new(&mut cfg.max_depth, 1u8..=9).text("Max LOD depth"))
            .on_hover_text("Deepest refinement = closest-up detail.");
        ui.add(egui::Slider::new(&mut cfg.bakes_per_frame, 1usize..=32).text("Bakes / frame"))
            .on_hover_text("1 = smoothest frame-time, slowest fill. Higher = faster load, bigger spikes.");
    });
}

fn obstacle_field_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    let mut regen_spec: Option<ObstacleFieldSpec> = None;

    let had = ctx.resource_scope(|_ctx, spec: &mut ObstacleFieldSpec| {
        let mut regen = false;

        ui.horizontal(|ui| {
            ui.label(format!("Seed {:#x}", spec.seed));
            if ui.button("🎲 Reseed").clicked() {
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
            // Keep the size distribution valid: min ≤ mode ≤ max. If the sliders
            // invert (e.g. min > mode) the log-normal sampler clamps EVERY crater to
            // the high end → a dense field of oversized overlapping basins + rims that
            // reads as jagged spike noise from altitude (the "craters look worse").
            s.craters.size.min = s.craters.size.min.min(s.craters.size.mode);
            s.craters.size.max = s.craters.size.max.max(s.craters.size.mode);
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
            // Same validity clamp as craters: min ≤ mode ≤ max.
            s.rocks.size.min = s.rocks.size.min.min(s.rocks.size.mode);
            s.rocks.size.max = s.rocks.size.max.max(s.rocks.size.mode);
        });

        ui.separator();
        if ui.button("♻ Regenerate").clicked() {
            regen = true;
        }
        ui.label(egui::RichText::new("Field rebuilds on slider release.").small().weak());

        if regen {
            regen_spec = Some(spec.clone());
        }
    });

    if had.is_none() {
        ui.label("Obstacle field plugin not active.");
        return;
    }

    if let Some(spec) = regen_spec {
        ctx.defer(move |world| {
            world.trigger(UpdateObstacleFieldSpec { spec });
        });
    }
}

/// The selected entity plus all of its descendants (subtree walk via
/// [`PanelCtx::get`]).
fn subtree(ctx: &PanelCtx, root: Entity) -> Vec<Entity> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let e = out[i];
        i += 1;
        if let Some(children) = ctx.get::<Children>(e) {
            out.extend(children.iter());
        }
    }
    out
}

/// Joint control over a revolute joint's `angle` port. Reads the
/// [`InspectorView`] snapshot; the setpoint write is deferred through
/// [`lunco_cosim::write_port`].
fn joint_control_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, j: JointReadout) {
    let measured = j.measured;
    let mut commanded = j.commanded;
    let holder = j.holder;

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
        ctx.defer(move |world| {
            let registry = world.resource::<PortRegistry>().clone();
            registry.write_port(world, holder, JOINT_ANGLE_PORT, commanded);
        });
    }
    if j.wired {
        ui.label(
            egui::RichText::new("⚠ driven by a wire — setpoint is transient")
                .small()
                .weak(),
        );
    }
}

/// Walk `root`'s subtree once, returning its distinct `StandardMaterial`
/// handles and the first `ShaderMaterial`-bearing entity. Replaces the
/// former `collect_std_handles` + `first_shader_holder`, which each ran an
/// independent `subtree` walk of the same root (CQ-204).
fn part_materials(
    ctx: &PanelCtx,
    root: Entity,
) -> (Vec<Handle<StandardMaterial>>, Option<Entity>) {
    let mut handles: Vec<Handle<StandardMaterial>> = Vec::new();
    let mut shader_holder: Option<Entity> = None;
    for e in subtree(ctx, root) {
        if let Some(m) = ctx.get::<MeshMaterial3d<StandardMaterial>>(e) {
            if !handles.iter().any(|h| h.id() == m.0.id()) {
                handles.push(m.0.clone());
            }
        }
        if shader_holder.is_none()
            && ctx
                .get::<MeshMaterial3d<lunco_materials::ShaderMaterial>>(e)
                .is_some()
        {
            shader_holder = Some(e);
        }
    }
    (handles, shader_holder)
}

/// Material-bearing parts of `root`'s subtree, each labelled by its leaf name.
fn editable_parts(ctx: &PanelCtx, root: Entity) -> Vec<(Entity, String)> {
    let ents = subtree(ctx, root);
    let mut out = Vec::new();
    for e in ents {
        let has_shader =
            ctx.get::<MeshMaterial3d<lunco_materials::ShaderMaterial>>(e).is_some();
        let has_std = ctx.get::<MeshMaterial3d<StandardMaterial>>(e).is_some();
        if has_shader || has_std {
            let label = ctx
                .get::<Name>(e)
                .map(|n| n.as_str().rsplit(['/', '\\']).next().unwrap_or(n.as_str()).to_string())
                .unwrap_or_else(|| format!("{e:?}"));
            out.push((e, label));
        }
    }
    out
}

/// Default part to edit: the first part WITHOUT a shader (the PBR body).
fn default_part(ctx: &PanelCtx, parts: &[(Entity, String)]) -> Option<Entity> {
    parts
        .iter()
        .map(|(e, _)| *e)
        .find(|e| ctx.get::<MeshMaterial3d<lunco_materials::ShaderMaterial>>(*e).is_none())
        .or_else(|| parts.first().map(|(e, _)| *e))
}

/// *Part* dropdown for a multi-part component. Writes the choice into
/// [`InspectorTarget`](crate::InspectorTarget) (deferred) and returns the
/// new target.
fn parts_selector(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
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
        ctx.defer(move |world| {
            world.resource_mut::<crate::InspectorTarget>().part = Some(c);
        });
        return Some(c);
    }
    current
}

/// Shader picker for a single part. Lists the [`ShaderCatalog`] entries and,
/// on pick, defers a `.wgsl` swap on `part`.
fn shader_picker_for_part(ui: &mut egui::Ui, ctx: &mut PanelCtx, part: Entity) {
    let entries = ctx
        .resource::<lunco_materials::ShaderCatalog>()
        .map(|c| c.entries.clone())
        .unwrap_or_default();
    if entries.is_empty() {
        return;
    }
    let cur = current_shader_path(ctx, part).unwrap_or_default();
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
            ctx.defer(move |world| swap_shader_on_entity(world, part, &path));
        }
    }
}

/// Bind shader `path` to `part`, building a fresh [`ShaderMaterial`]
/// (carrying over the previous one's uniforms) and removing the part's
/// `StandardMaterial`. Runs inside a deferred closure (`&mut World`).
fn swap_shader_on_entity(world: &mut World, part: Entity, path: &str) {
    use lunco_materials::ShaderMaterial;
    let template = world
        .get::<MeshMaterial3d<ShaderMaterial>>(part)
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
/// (read via [`PanelCtx`]), or `None` if it isn't using one.
fn current_shader_path(ctx: &PanelCtx, part: Entity) -> Option<String> {
    let h = ctx
        .get::<MeshMaterial3d<lunco_materials::ShaderMaterial>>(part)
        .map(|m| m.0.clone())?;
    let sid = ctx
        .resource::<Assets<lunco_materials::ShaderMaterial>>()?
        .get(&h)
        .map(|m| m.shader.id())?;
    let p = ctx.resource::<AssetServer>()?.get_path(sid)?;
    Some(p.to_string())
}

/// "Shader Tools" — GUI front-end for the live shader-authoring commands.
/// Create / Import apply the result to `part` by `Entity`. Commands are
/// deferred (they run their observers via `world.trigger`).
fn shader_tools_ui(ui: &mut egui::Ui, ctx: &mut PanelCtx, part: Entity) {
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
                let name = st.name.clone();
                let template = st.template.clone();
                ctx.defer(move |world| create_and_apply(world, part, &name, &template));
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
                let src = st.import.trim().to_string();
                ctx.defer(move |world| import_and_apply(world, part, &src));
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .button("Rescan twin folder")
                    .on_hover_text("Register any .wgsl dropped into the twin's shaders/ folder")
                    .clicked()
                {
                    ctx.defer(|world| {
                        world.trigger(crate::commands::RescanShaders {});
                    });
                }
                if let Some(path) = current_shader_path(ctx, part) {
                    if ui
                        .button("Delete current")
                        .on_hover_text(format!("Remove {path} (file + picker)"))
                        .clicked()
                    {
                        ctx.defer(move |world| {
                            world.trigger(crate::commands::DeleteShader { path });
                        });
                    }
                }
            });

            ui.memory_mut(|m| m.data.insert_temp(id, st));
        });
}

/// Create a shader from `template` (registers it), then bind it to `part`.
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

/// Import an external `.wgsl` (registers it), then bind it to `part`.
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

/// If a shader for `stem` is now registered, swap `part` onto it.
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

/// Editable PBR controls for the selected object's `StandardMaterial`s.
/// Reads a snapshot via [`PanelCtx`]; the asset + USD writes are deferred.
fn material_pbr_section(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    part: Entity,
    handles: &[Handle<StandardMaterial>],
) {
    let Some(handle) = handles.first().cloned() else {
        return;
    };

    // Snapshot current values — no world borrow held while drawing widgets.
    let snap = {
        let Some(mats) = ctx.resource::<Assets<StandardMaterial>>() else {
            ui.label("Material still loading…");
            return;
        };
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
        let handles = handles.to_vec();
        ctx.defer(move |world| {
            if let Some(mut mats) = world.get_resource_mut::<Assets<StandardMaterial>>() {
                for handle in &handles {
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

            // Propagate changes to USD.
            if let Some(prim) = world.get::<UsdPrimPath>(part).cloned() {
                let mesh_sdf = SdfPath::new(&prim.path).ok();
                let id = prim.stage_handle.id();
                // Ph0′ canonical-only: resolve the bound shader off the LIVE
                // canonical stage (source of truth), built on demand from the
                // asset's recipe. Fetch the recipe first (immutable `Assets`
                // borrow), drop it, then reach for the separate `CanonicalStages`
                // non-send resource.
                let recipe = world
                    .get_resource::<Assets<UsdStageAsset>>()
                    .and_then(|stages| stages.get(&prim.stage_handle))
                    .and_then(|a| a.recipe.clone());
                if let Some(mut canonical) =
                    world.get_non_send_resource_mut::<lunco_usd_bevy::CanonicalStages>()
                {
                    if canonical.get(id).is_none() {
                        if let Some(r) = recipe.as_ref() {
                            canonical.get_or_build(id, r);
                        }
                    }
                }
                let shader_path = mesh_sdf
                    .as_ref()
                    .and_then(|mesh_sdf| {
                        let canonical =
                            world.get_non_send_resource::<lunco_usd_bevy::CanonicalStages>()?;
                        let view = canonical.get(id)?.view();
                        resolve_bound_shader(&view, mesh_sdf)
                    })
                    .map(|p| p.to_string());

                let mut write = |attr: &str, ty: &str, value: String| match &shader_path {
                    Some(sp) => apply_usd_path_attribute_change(world, part, sp.clone(), attr, ty, value),
                    None => apply_usd_attribute_change(world, part, attr, ty, value),
                };

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
        });
    }
}

/// Render named, range-bounded controls for the selected entity's
/// [`ShaderMaterial`](lunco_materials::ShaderMaterial) generic uniforms.
/// Reads a snapshot via [`PanelCtx`]; the live-asset + USD writes deferred.
fn shader_parameters_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_materials::{ParamType, ParamValue, ShaderMaterial, UiKind};

    let Some(handle) = ctx
        .get::<MeshMaterial3d<ShaderMaterial>>(entity)
        .map(|m| m.0.clone())
    else {
        return;
    };

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
        let Some(mats) = ctx.resource::<Assets<ShaderMaterial>>() else {
            ui.label("Material still loading…");
            return;
        };
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

    if rows.is_empty() {
        ui.label("No editable parameters.");
        return;
    }

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

    if !edits.is_empty() {
        let usd_prim_exists = ctx.get::<UsdPrimPath>(entity).is_some();
        ctx.defer(move |world| {
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
                            let name_lc = name.to_lowercase();
                            let t = if name_lc.contains("color") || name_lc.contains("colour") {
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
        });
    }
}

/// Render editable sliders for every tunable `parameter Real` in the
/// entity's Modelica model. Reads params via [`PanelCtx::get`]; the op
/// dispatch + recompile signal run in a deferred `&mut World` closure.
fn modelica_parameters_section(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    entity: Entity,
) {
    use lunco_modelica::ModelicaModel;

    // Snapshot the current params so we can render stable sliders.
    let (params, model_name) = match ctx.get::<ModelicaModel>(entity) {
        Some(m) => (m.parameters.clone(), m.model_name.clone()),
        None => return,
    };
    if params.is_empty() {
        ui.label(egui::RichText::new("(no tunable parameters)").weak().small());
        return;
    }

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

    ctx.defer(move |world| {
        use lunco_modelica::state::ModelicaDocumentRegistry;
        use lunco_modelica::ui::panels::canvas_diagram::apply_ops_public;
        use lunco_modelica::document::ModelicaOp;
        use lunco_modelica::{ModelicaChannels, ModelicaCommand, ModelicaModel};

        // Mirror the new value into ECS state for instant slider feedback;
        // bump session id so the worker treats this as a fresh generation.
        let mut session_id = 0u64;
        if let Some(mut m) = world.get_mut::<ModelicaModel>(entity) {
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
    });
}

/// Dispatch a `UsdOp::SetAttribute` for a specific prim path. Runs inside a
/// deferred `&mut World` closure.
/// Comms & Orbit (doc 43): position ground stations (geodetic anchor) and
/// satellites (Kepler elements) realistically, tune antenna range/mask, and
/// watch live link state. Edits update the live component (the USD bridge
/// runs once per prim) AND persist as journaled `SetAttribute` ops.
fn comms_orbit_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_celestial::{CommsAntenna, CommsLinkState, GeodeticAnchor, KeplerOrbit};

    let anchor = ctx.get::<GeodeticAnchor>(entity).copied();
    let orbit = ctx.get::<KeplerOrbit>(entity).copied();
    let antenna = ctx.get::<CommsAntenna>(entity).cloned();
    if anchor.is_none() && orbit.is_none() && antenna.is_none() {
        return;
    }

    egui::CollapsingHeader::new("Comms & Orbit")
        .default_open(true)
        .show(ui, |ui| {
            if let Some(a) = anchor {
                ui.label("Ground anchor (lat/lon °, height m):");
                let mut lat = a.geodetic.lat_deg;
                let mut lon = a.geodetic.lon_deg;
                let mut height = a.geodetic.height_m;
                let mut body = a.body;
                let changed = ui
                    .horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut lat).speed(0.01).range(-90.0..=90.0).prefix("lat "))
                            .changed()
                            | ui.add(egui::DragValue::new(&mut lon).speed(0.01).range(-180.0..=180.0).prefix("lon "))
                                .changed()
                            | ui.add(egui::DragValue::new(&mut height).speed(1.0).prefix("h "))
                                .changed()
                    })
                    .inner
                    | ui.add(egui::DragValue::new(&mut body).prefix("body NAIF ")).changed();
                if changed {
                    ctx.defer(move |world| {
                        if let Some(mut c) = world.get_mut::<GeodeticAnchor>(entity) {
                            c.body = body;
                            c.geodetic.lat_deg = lat;
                            c.geodetic.lon_deg = lon;
                            c.geodetic.height_m = height;
                        }
                        apply_usd_attribute_change(world, entity, "lunco:anchor:lat", "double", format!("{lat}"));
                        apply_usd_attribute_change(world, entity, "lunco:anchor:lon", "double", format!("{lon}"));
                        apply_usd_attribute_change(world, entity, "lunco:anchor:height", "double", format!("{height}"));
                        apply_usd_attribute_change(world, entity, "lunco:anchor:body", "int", format!("{body}"));
                    });
                }
            }

            if let Some(o) = orbit {
                ui.label("Kepler orbit (a m, e, angles °):");
                let mut a_m = o.elements.semi_major_axis_m;
                let mut e = o.elements.eccentricity;
                let mut inc = o.elements.inclination_deg;
                let mut raan = o.elements.raan_deg;
                let mut argp = o.elements.arg_periapsis_deg;
                let mut m0 = o.elements.mean_anomaly_deg;
                let changed = ui
                    .horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut a_m).speed(10_000.0).prefix("a ")).changed()
                            | ui.add(egui::DragValue::new(&mut e).speed(0.005).range(0.0..=0.95).prefix("e "))
                                .changed()
                            | ui.add(egui::DragValue::new(&mut inc).speed(0.1).range(-180.0..=180.0).prefix("i "))
                                .changed()
                    })
                    .inner
                    | ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut raan).speed(0.1).prefix("Ω ")).changed()
                            | ui.add(egui::DragValue::new(&mut argp).speed(0.1).prefix("ω ")).changed()
                            | ui.add(egui::DragValue::new(&mut m0).speed(0.1).prefix("M₀ ")).changed()
                    })
                    .inner;
                if changed {
                    ctx.defer(move |world| {
                        if let Some(mut c) = world.get_mut::<KeplerOrbit>(entity) {
                            c.elements.semi_major_axis_m = a_m;
                            c.elements.eccentricity = e;
                            c.elements.inclination_deg = inc;
                            c.elements.raan_deg = raan;
                            c.elements.arg_periapsis_deg = argp;
                            c.elements.mean_anomaly_deg = m0;
                        }
                        apply_usd_attribute_change(world, entity, "lunco:orbit:semiMajorAxisM", "double", format!("{a_m}"));
                        apply_usd_attribute_change(world, entity, "lunco:orbit:eccentricity", "double", format!("{e}"));
                        apply_usd_attribute_change(world, entity, "lunco:orbit:inclinationDeg", "double", format!("{inc}"));
                        apply_usd_attribute_change(world, entity, "lunco:orbit:raanDeg", "double", format!("{raan}"));
                        apply_usd_attribute_change(world, entity, "lunco:orbit:argPeriapsisDeg", "double", format!("{argp}"));
                        apply_usd_attribute_change(world, entity, "lunco:orbit:meanAnomalyDeg", "double", format!("{m0}"));
                    });
                }
            }

            if let Some(ant) = antenna {
                ui.label("Antenna:");
                let mut max_range = ant.max_range_m;
                let mut min_elev = ant.min_elevation_deg;
                let changed = ui
                    .horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut max_range).speed(1_000.0).prefix("range≤ "))
                            .changed()
                            | ui.add(
                                egui::DragValue::new(&mut min_elev)
                                    .speed(0.1)
                                    .range(-10.0..=89.0)
                                    .prefix("elev≥ "),
                            )
                            .changed()
                    })
                    .inner;
                if changed {
                    ctx.defer(move |world| {
                        if let Some(mut c) = world.get_mut::<CommsAntenna>(entity) {
                            c.max_range_m = max_range;
                            c.min_elevation_deg = min_elev;
                        }
                        apply_usd_attribute_change(world, entity, "lunco:comms:maxRangeM", "double", format!("{max_range}"));
                        apply_usd_attribute_change(world, entity, "lunco:comms:minElevationDeg", "double", format!("{min_elev}"));
                    });
                }
            }

            if let Some(state) = ctx.get::<CommsLinkState>(entity) {
                ui.separator();
                ui.label("Links:");
                for peer in &state.peers {
                    let (icon, color) = if peer.connected {
                        ("●", egui::Color32::from_rgb(0x4c, 0xaf, 0x50))
                    } else {
                        ("○", egui::Color32::from_rgb(0xe5, 0x73, 0x73))
                    };
                    let mut text = format!("{icon} {} — {:.0} km", peer.peer, peer.range_m / 1000.0);
                    if let Some(e) = peer.elevation_deg {
                        text.push_str(&format!(", elev {e:.1}°"));
                    }
                    if let Some(b) = &peer.occluded_by {
                        text.push_str(&format!(" (behind {b})"));
                    }
                    ui.colored_label(color, text);
                }
                match state.earth_hops {
                    Some(0) => ui.label("Earth route: this IS an Earth station"),
                    Some(h) => ui.label(format!("Earth route: connected ({h} hop{})", if h == 1 { "" } else { "s" })),
                    None => ui.label("Earth route: no path"),
                };
            }
        });
    ui.separator();
}

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

/// Dispatch a `UsdOp::SetAttribute` to write changes back to the USD
/// document. Runs inside a deferred `&mut World` closure.
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

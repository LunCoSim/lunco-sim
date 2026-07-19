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
use lunco_mobility::{WheelRaycast, Suspension};
use lunco_cosim::{joint_angle_holder, JOINT_ANGLE_PORT};
use lunco_core::ports::PortRegistry;
// Appearance INTENT. The Material (PBR) section edits this component, not the
// material asset — see `material_pbr_section`.
use lunco_materials::ShaderLook;
use lunco_render::PbrLook;

use lunco_obstacle_field::{ObstacleFieldSpec, Pattern, plugin::UpdateObstacleFieldSpec};

use crate::SelectedEntities;
// Doc resolution + material-binding walk: headless-safe, shared verbatim with the
// command layer (which is why they don't live in this panel — see `doc_resolve`).
use crate::doc_resolve::{bound_shader_prim, resolve_doc_for_entity};
use lunco_usd::document::{UsdOp, LayerId};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd_bevy::UsdPrimPath;

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
    pub shadow_maps_enabled: bool,
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
        let (illuminance, shadow_maps_enabled, rgb) = world
            .get::<DirectionalLight>(e)
            .map(|l| {
                let lin = l.color.to_linear();
                (l.illuminance, l.shadow_maps_enabled, [lin.red, lin.green, lin.blue])
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
            shadow_maps_enabled,
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

/// Delete `entity` from the scene — the single delete path for both the Del
/// hotkey and the Delete button.
///
/// Authors the removal into the active document's runtime layer FIRST (so the
/// delete is a journaled, undoable, networked document op — the editor keeps no
/// private history), then performs the live despawn for immediate feedback and
/// drops it from the selection. A non-document entity (a palette spawn the doc
/// doesn't own) simply isn't authored — it just despawns.
// NOTE: there is no local `delete_entity` helper any more. It did the same three things
// the typed `commands::DeleteEntity` verb does (author the `RemovePrim`, despawn, drop
// the selection), so it was a second delete path that the command bus — and hence the
// API, the journal and networked peers — never saw. The Inspector triggers the command.

fn inspector_content(_panel: &mut Inspector, ui: &mut egui::Ui, ctx: &mut PanelCtx) {

        // Delete hotkey
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            let primary = ctx
                .resource::<SelectedEntities>()
                .and_then(|s| s.primary());
            if let Some(entity) = primary {
                ctx.defer(move |world| {
                    // The typed verb — despawns, drops the selection, AND authors the
                    // `RemovePrim`, so the delete persists, journals, replicates, and
                    // undoes (Ctrl+Z).
                    world.trigger(crate::commands::DeleteEntity {
                        target: entity,
                        intent: lunco_core::EditIntent::Persistent,
                    });
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

        // ── Terrain Overlay (slope-hazard analysis VIEW) ─────────────
        egui::CollapsingHeader::new("Terrain Overlay")
            .default_open(true)
            .show(ui, |ui| terrain_overlay_section(ui, ctx));
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

        // ── USD parameters: data-driven bounded sliders for attributes that
        //    author a `customData {min,max,unit}` UI hint. ────────────────
        usd_parameters_section(ui, ctx, entity);

        // ── Mount: snap an attached part onto the socket it declares, re-deriving
        //    its placement + joint anchor from the mount frames (doc 48 §3.1). ──
        mount_section(ui, ctx, entity);

        // ── Transform component ──────────────────────────────────────
        // The sliders author a **document op**, they do not poke ECS: a committed
        // edit fires `MoveEntity`, whose observers both move the body (physics
        // seat + kinematic pulse — the old hand-copied CQ-510 block, now in ONE
        // place) and author `UsdOp::SetTranslate` into the runtime layer. So an
        // Inspector move survives reload, journals, syncs, and is undone by the
        // same Ctrl+Z as a gizmo drag. Committed = drag released or value typed —
        // per-frame firing during a drag would push one op per frame.
        if ctx.get::<Transform>(entity).is_some() {
            egui::CollapsingHeader::new("Transform")
                .default_open(true)
                .show(ui, |ui| {
                    // GRID-ABSOLUTE, not `Transform.translation`: on a
                    // grid-direct prim the raw local is only the cell
                    // remainder, so the sliders showed a number that agreed with
                    // neither the authored USD nor the object's actual place —
                    // and committing it fed that short value to `MoveEntity`,
                    // teleporting the object one cell. This is the same frame
                    // the gizmo authors and the same one `MoveEntity` expects.
                    if let Some(t) = grid_absolute_of(ctx, entity).map(|p| p.as_vec3()) {
                        let (mut x, mut y, mut z) = (t.x, t.y, t.z);
                        // `DragValue`, not a ±1000 `Slider`: a grid-absolute
                        // coordinate is unbounded (a moonbase prim sits well
                        // outside ±1000 m of the grid origin), and a slider would
                        // CLAMP it — merely showing the panel and nudging one axis
                        // would have hauled the object back inside the range.
                        let rx = ui.add(egui::DragValue::new(&mut x).speed(0.1).prefix("X: "));
                        let ry = ui.add(egui::DragValue::new(&mut y).speed(0.1).prefix("Y: "));
                        let rz = ui.add(egui::DragValue::new(&mut z).speed(0.1).prefix("Z: "));
                        // Author ONCE, on release — not on every `changed()` frame, which
                        // would flood the journal with an op per mouse-move for a single
                        // drag. Same rule as the gizmo's drag-end authoring.
                        let committed = [&rx, &ry, &rz]
                            .iter()
                            .any(|r| r.drag_stopped() || (r.changed() && !r.dragged()));
                        if committed {
                            let new_t = Vec3::new(x, y, z);
                            ctx.defer(move |world| {
                                // Route through the typed `MoveEntity` verb rather than
                                // poking `Transform` here. It already owns the
                                // physics-aware pose seat (CQ-510: writing only
                                // `Transform` silently no-ops on a body, because avian
                                // re-derives it from the f64 `Position` each tick), and
                                // its persister authors the `SetTranslate` — so the edit
                                // journals, replicates, persists, and undoes. The
                                // hand-rolled copy that used to live here did none of
                                // that.
                                let Some(gid) =
                                    world.get::<lunco_core::GlobalEntityId>(entity).copied()
                                else {
                                    warn!(
                                        "INSPECTOR: {entity:?} has no GlobalEntityId — not movable"
                                    );
                                    return;
                                };
                                world.trigger(crate::commands::MoveEntity {
                                    entity_id: gid.get(),
                                    translation: new_t,
                                });
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
                    if let Some(r0) = ctx.get::<WheelRaycast>(entity).map(|w| w.wheel_radius as f32) {
                        let mut radius = r0;
                        let r_changed = ui.add(egui::Slider::new(&mut radius, 0.1..=2.0).text("Wheel Radius (m)")).changed();
                        if r_changed {
                            ctx.defer(move |world| {
                                if let Some(mut wheel) = world.get_mut::<WheelRaycast>(entity) {
                                    wheel.wheel_radius = radius as f64;
                                }
                            });
                        }
                    }
                });
        }

        // ── Suspension component ─────────────────────────────────────
        if ctx.get::<Suspension>(entity).is_some() {
            egui::CollapsingHeader::new("Suspension")
                .default_open(false)
                .show(ui, |ui| {
                    if let Some((rest0, k0, d0)) = ctx.get::<Suspension>(entity).map(|s| {
                        (
                            s.rest_length as f32,
                            s.spring_k as f32,
                            s.damping_c as f32,
                        )
                    }) {
                        let mut rest = rest0;
                        let mut k = k0;
                        let mut d = d0;

                        let rest_changed = ui.add(egui::Slider::new(&mut rest, 0.1..=2.0).text("Rest Length (m)")).changed();
                        let k_changed = ui.add(egui::Slider::new(&mut k, 100.0..=100000.0).text("Spring K (N/m)").logarithmic(true)).changed();
                        let d_changed = ui.add(egui::Slider::new(&mut d, 100.0..=10000.0).text("Damping C (N·s/m)").logarithmic(true)).changed();

                        if rest_changed || k_changed || d_changed {
                            ctx.defer(move |world| {
                                if let Some(mut susp) = world.get_mut::<Suspension>(entity) {
                                    if rest_changed { susp.rest_length = rest as f64; }
                                    if k_changed { susp.spring_k = k as f64; }
                                    if d_changed { susp.damping_c = d as f64; }
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
                let (pbr_parts, shader_holder) = part_materials(ctx, part);
                if let Some(holder) = shader_holder {
                    egui::CollapsingHeader::new("Shader Parameters")
                        .default_open(true)
                        .show(ui, |ui| {
                            shader_parameters_section(ui, ctx, holder);
                        });
                }
                if !pbr_parts.is_empty() {
                    egui::CollapsingHeader::new("Material (PBR)")
                        .default_open(true)
                        .show(ui, |ui| {
                            material_pbr_section(ui, ctx, part, &pbr_parts);
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
                world.trigger(crate::commands::DeleteEntity {
                    target: entity,
                    intent: lunco_core::EditIntent::Persistent,
                });
            });
        }
    }

/// Live sun + ambient controls. Reads the change-driven [`InspectorView`]
/// snapshot and dispatches every edit through a single
/// Bounded sliders for the selected prim's `customData`-ranged attributes,
/// from the [`UsdParamView`](crate::ui::usd_params::UsdParamView) view-model. An
/// asset that authors `customData {min,max,unit}` on a scalar gets a clamped
/// slider here without any hand-coded range; edits write back through the same
/// `ApplyUsdOp(SetAttribute)` path as every other Inspector control.
/// Grid-absolute translation of `entity` — `cell × edge + local`, the frame USD
/// authors `xformOp:translate` in and the frame `MoveEntity` takes.
///
/// The `PanelCtx` (one-component-at-a-time) spelling of
/// [`lunco_core::coords::grid_absolute`], which needs `Query`s the panel doesn't
/// have. Same rule: no parent `Grid` ⇒ no cell ⇒ the local translation already
/// IS the authored value.
fn grid_absolute_of(ctx: &PanelCtx, entity: Entity) -> Option<bevy::math::DVec3> {
    let tf = ctx.get::<Transform>(entity)?;
    let Some(grid) = ctx
        .get::<ChildOf>(entity)
        .and_then(|c| ctx.get::<big_space::prelude::Grid>(c.parent()))
    else {
        return Some(tf.translation.as_dvec3());
    };
    let cell = ctx
        .get::<big_space::prelude::CellCoord>(entity)
        .copied()
        .unwrap_or_default();
    Some(grid.grid_position_double(&cell, tf))
}

fn usd_parameters_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    let params: Vec<crate::ui::usd_params::UsdParam> =
        match ctx.resource::<crate::ui::usd_params::UsdParamView>() {
            Some(v) if v.entity == Some(entity) && !v.params.is_empty() => v.params.clone(),
            _ => return,
        };
    egui::CollapsingHeader::new("🎚 Parameters")
        .default_open(true)
        .show(ui, |ui| {
            let mut edits: Vec<(String, String, String)> = Vec::new();
            for p in &params {
                let mut v = p.value;
                let text = if p.unit.is_empty() {
                    p.label.clone()
                } else {
                    format!("{} ({})", p.label, p.unit)
                };
                if ui
                    .add(egui::Slider::new(&mut v, p.min..=p.max).text(text))
                    .changed()
                {
                    edits.push((p.name.clone(), p.type_name.clone(), format!("{v}")));
                }
            }
            for (name, type_name, value) in edits {
                ctx.defer(move |world| {
                    apply_usd_attribute_change(world, entity, &name, &type_name, value);
                });
            }
        });
}

/// Map a socket's `lunco:mount:joint` token (+ optional axis) to the typed
/// [`AttachJoint`](lunco_usd::attach::AttachJoint) the attach lowering wants.
/// Unknown tokens fall back to `Fixed` (the safe, axis-free default).
#[cfg(not(target_arch = "wasm32"))]
fn attach_joint_from(joint: &str, axis: Option<&str>) -> lunco_usd::attach::AttachJoint {
    use lunco_usd::attach::{Axis, AttachJoint};
    let axis = match axis {
        Some("Y") => Axis::Y,
        Some("Z") => Axis::Z,
        _ => Axis::X,
    };
    match joint {
        "revolute" => AttachJoint::Revolute { axis },
        "prismatic" => AttachJoint::Prismatic { axis },
        _ => AttachJoint::Fixed,
    }
}

/// The 🔩 Mount section — one row per socket the selected host advertises.
///
/// - A socket **holding a part** offers **⟳ Snap**: re-author that part's transform
///   + joint anchor from the mount frames (`realign_component_ops`) so both follow
///   the socket. All frames are on the live stage.
/// - An **empty socket** that names a default asset (`lunco:mount:asset`) offers
///   **⊕ Attach**: the *new-attach* flow — compose the not-yet-loaded asset, read its
///   plug frame, `from_mount` it onto the socket, and reference + joint it in via
///   `AttachComponent`.
///
/// Reads the pre-resolved [`UsdMountView`](crate::ui::usd_mount::UsdMountView) (the
/// socket frame math ran in the producer; it needs the `!Send` stage).
fn mount_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_usd::attach::realign_component_ops;

    let (host_path, items) = match ctx.resource::<crate::ui::usd_mount::UsdMountView>() {
        Some(v) if v.entity == Some(entity) && !v.items.is_empty() => {
            (v.host_path.clone(), v.items.clone())
        }
        _ => return,
    };

    egui::CollapsingHeader::new("🔩 Mount")
        .default_open(true)
        .show(ui, |ui| {
            let mut snap: Option<(String, String, [f64; 3], [f64; 3])> = None;
            // (asset, child name, host, joint token, axis, socket frame)
            let mut attach: Option<(String, String, String, String, Option<String>, Transform)> =
                None;
            for item in &items {
                ui.horizontal(|ui| {
                    let joint = match &item.axis {
                        Some(ax) => format!("{} {}", item.joint, ax),
                        None => item.joint.clone(),
                    };
                    ui.label(format!("🔌 {} ({}, {joint})", item.socket, item.accepts));
                });
                match (&item.part_path, &item.part_leaf, item.placement, item.rotate_deg) {
                    (Some(part), Some(leaf), Some(placement), Some(rotate)) => {
                        ui.horizontal(|ui| {
                            let btn = egui::Button::new(format!("⟳ Snap {leaf}"));
                            let resp = ui.add_enabled(!item.aligned, btn);
                            if resp.clicked() {
                                snap = Some((part.clone(), item.joint_path.clone(), placement, rotate));
                            }
                            if item.aligned {
                                ui.weak("aligned");
                            } else {
                                ui.weak(format!(
                                    "→ ({:.2}, {:.2}, {:.2})",
                                    placement[0], placement[1], placement[2]
                                ));
                            }
                        });
                    }
                    // Empty socket with a suggested asset → offer a new-attach.
                    _ => match &item.attach_asset {
                        Some(asset) => {
                            let leaf = asset.rsplit('/').next().unwrap_or(asset);
                            ui.horizontal(|ui| {
                                if ui.button(format!("⊕ Attach {leaf}")).clicked() {
                                    attach = Some((
                                        asset.clone(),
                                        item.socket.clone(),
                                        host_path.clone(),
                                        item.joint.clone(),
                                        item.axis.clone(),
                                        item.socket_frame,
                                    ));
                                }
                                ui.weak("empty");
                            });
                        }
                        None => {
                            ui.weak("  (empty)");
                        }
                    },
                }
            }
            if let Some((part, joint, placement, rotate)) = snap {
                ctx.defer(move |world| {
                    let ops = realign_component_ops(
                        lunco_usd::document::LayerId::root(),
                        part,
                        joint,
                        placement,
                        rotate,
                    );
                    apply_usd_ops(world, entity, ops);
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some((asset, name, host, joint_tok, axis, socket_frame)) = attach {
                ctx.defer(move |world| {
                    attach_component_at_socket(
                        world,
                        entity,
                        host,
                        name,
                        asset,
                        attach_joint_from(&joint_tok, axis.as_deref()),
                        socket_frame,
                    );
                });
            }
            #[cfg(target_arch = "wasm32")]
            let _ = &attach;
        });
}

/// Perform a new-attach: read the asset's plug frame off its (not-yet-loaded) file,
/// `from_mount` it onto `socket_frame`, and dispatch [`AttachComponent`] to the
/// host's document. Runs in a deferred `&mut World` closure (the asset composition
/// does file I/O — a one-shot click, never per frame). Native-only.
#[cfg(not(target_arch = "wasm32"))]
fn attach_component_at_socket(
    world: &mut World,
    entity: Entity,
    host_path: String,
    name: String,
    asset: String,
    joint: lunco_usd::attach::AttachJoint,
    socket_frame: Transform,
) {
    use lunco_usd::attach::AttachSpec;
    // Ask `lunco-assets` where the reference lives — do NOT assume the shipped
    // library. A component authored by an open Twin is `twin://<name>/…`, which
    // has no path under `assets/` at all; joining one produced a path that never
    // existed and the attach was skipped with a "no plug frame" warning.
    let schemes = world.get_resource::<lunco_assets::SchemeRegistry>().cloned().unwrap_or_default();
    let Some(fs_path) = schemes.local_path(&asset) else {
        bevy::log::warn!("[mount] `{asset}` resolves to no local file; attach skipped");
        return;
    };
    let Some(plug) = lunco_usd_bevy::mount::read_asset_plug_frame(&fs_path) else {
        bevy::log::warn!("[mount] no plug frame in asset `{asset}` ({}); attach skipped", fs_path.display());
        return;
    };
    let Some(doc) = resolve_doc_for_entity(world, entity) else {
        return;
    };
    let spec = AttachSpec::from_mount(
        LayerId::root(),
        host_path,
        name,
        asset,
        joint,
        socket_frame,
        plug,
    );
    world.trigger(lunco_usd::commands::AttachComponent { doc, spec });
}

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
                let mut shadows = s.shadow_maps_enabled;
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
                    cmd.shadow_maps_enabled = Some(shadows);
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

/// Slope-hazard analysis overlay controls — the render VIEW of the terrain slope
/// field. Edits the global `TerrainOverlayParams`; the tile shader colourises live
/// (no re-bake). Read a Copy, edit locally, write back ONLY on a real change so the
/// live-sync system stays change-driven instead of firing every frame.
fn terrain_overlay_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    use lunco_terrain_surface::overlay::TerrainOverlayParams;
    let Some(cur) = ctx.resource::<TerrainOverlayParams>().copied() else {
        ui.label("No streaming terrain in this scene.");
        return;
    };
    let mut p = cur;
    ui.checkbox(&mut p.enabled, "Slope hazard overlay")
        .on_hover_text("Colour the terrain by steepness: green traversable → red impassable.");
    ui.add_enabled_ui(p.enabled, |ui| {
        ui.add(egui::Slider::new(&mut p.safe_deg, 0.0..=45.0).text("Safe ≤ (°)"))
            .on_hover_text("Slopes at/below this stay green.");
        ui.add(egui::Slider::new(&mut p.cliff_deg, 0.0..=45.0).text("Cliff ≥ (°)"))
            .on_hover_text("The critical angle: slopes at/above this go red. Tunes live, no re-bake.");
        ui.add(egui::Slider::new(&mut p.opacity, 0.0..=1.0).text("Opacity"));
        // Keep the band ordered so the ramp never inverts.
        if p.safe_deg > p.cliff_deg {
            p.safe_deg = p.cliff_deg;
        }
        draw_slope_legend(ui, p.safe_deg, p.cliff_deg);
    });
    if p != cur {
        ctx.resource_scope(|_c, r: &mut TerrainOverlayParams| *r = p);
    }
}

/// A green→amber→red gradient bar over slope angle `[0°, 45°]`, coloured by the SAME
/// transfer the shader runs (`TransferFn::SlopeHazard` — the one Transfer plane, see
/// `docs/architecture/terrain-layered-rendering.md`), so the legend swatch and the
/// terrain pixel agree by construction. Tick marks at the safe/cliff angles.
fn draw_slope_legend(ui: &mut egui::Ui, safe_deg: f32, cliff_deg: f32) {
    use lunco_terrain_surface::TransferFn;
    const MAX_DEG: f32 = 45.0;
    let hazard = TransferFn::SlopeHazard {
        safe_rad: safe_deg.to_radians(),
        cliff_rad: cliff_deg.to_radians(),
    };
    let w = ui.available_width().min(240.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 16.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let n = 96usize;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let deg = t * MAX_DEG;
        let c = hazard.sample(deg.to_radians());
        let col = egui::Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
        let x0 = rect.left() + rect.width() * t;
        let x1 = rect.left() + rect.width() * ((i + 1) as f32 / n as f32);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            col,
        );
    }
    // Tick marks at the two critical angles.
    for deg in [safe_deg, cliff_deg] {
        let x = rect.left() + rect.width() * (deg / MAX_DEG).clamp(0.0, 1.0);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, egui::Color32::WHITE),
        );
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("0°").weak().small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(format!("{MAX_DEG:.0}°")).weak().small());
        });
    });
    ui.label(
        egui::RichText::new(format!("safe ≤ {safe_deg:.0}°   ·   cliff ≥ {cliff_deg:.0}°"))
            .weak()
            .small(),
    );
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

/// Walk `root`'s subtree once, returning its PBR-surface **entities** and the
/// first [`ShaderLook`]-bearing entity. Replaces the former
/// `collect_std_handles` + `first_shader_holder`, which each ran an
/// independent `subtree` walk of the same root (CQ-204).
///
/// Surfaces are addressed by ENTITY and classified by their appearance **intent**
/// ([`PbrLook`] / [`ShaderLook`]), never by a bound material: the material is
/// derived from the intent (`lunco-render-bevy` re-binds on `Changed<…Look>`), it is
/// *shared* across every entity with the same look — so an in-place asset write would
/// bleed onto all of them — and naming it would drag `bevy_pbr` into this crate.
fn part_materials(ctx: &PanelCtx, root: Entity) -> (Vec<Entity>, Option<Entity>) {
    let mut parts: Vec<Entity> = Vec::new();
    let mut shader_holder: Option<Entity> = None;
    for e in subtree(ctx, root) {
        if ctx.get::<PbrLook>(e).is_some() {
            parts.push(e);
        }
        if shader_holder.is_none() && ctx.get::<ShaderLook>(e).is_some() {
            shader_holder = Some(e);
        }
    }
    (parts, shader_holder)
}

/// Material-bearing parts of `root`'s subtree, each labelled by its leaf name.
fn editable_parts(ctx: &PanelCtx, root: Entity) -> Vec<(Entity, String)> {
    let ents = subtree(ctx, root);
    let mut out = Vec::new();
    for e in ents {
        let has_shader = ctx.get::<ShaderLook>(e).is_some();
        let has_std = ctx.get::<PbrLook>(e).is_some();
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
        .find(|e| ctx.get::<ShaderLook>(*e).is_none())
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

/// Point `part`'s [`ShaderLook`] at shader `path`, carrying over the params it
/// already had (the render binder swaps the material). Runs inside a deferred
/// closure (`&mut World`).
fn swap_shader_on_entity(world: &mut World, part: Entity, path: &str) {
    let mut look = world.get::<ShaderLook>(part).cloned().unwrap_or_default();
    look.shader = path.to_string();
    world
        .commands()
        .entity(part)
        // The `PbrLook` intent must go. Leaving it would have the PBR binder keep
        // re-inserting its own material alongside the shader one — two materials on
        // one mesh, drawn twice.
        .remove::<PbrLook>()
        .try_insert(look);
    // …and the material that binder ALREADY bound, or the same double-draw happens
    // once, statically. (Removed reflectively — this crate may not name `bevy_pbr`.)
    crate::commands::drop_bound_pbr_material(world, part);

    // Propagate to USD — onto the `Shader` prim of the `Material` this geometry is
    // bound to. A shader is not a property of a mesh: it belongs to the material, and
    // the material is what the mesh binds. So the edit goes where the shader lives.
    //
    // The TYPE is the schema's, and writer and reader must agree on it, not just on the
    // name: an `asset` reads back as `Value::AssetPath`, and a loader asking for a
    // `String` gets `None`.
    if let Some(shader_prim) = bound_shader_prim_path(world, part) {
        apply_usd_path_attribute_change(
            world,
            part,
            shader_prim,
            "info:wgsl:sourceAsset",
            "asset",
            format!("@{}@", path),
        );
    } else {
        warn!(
            "[inspector] this prim is bound to no material, so there is no shader to \
             repoint. Bind it to a `Material` (with a `Shader` child) and the picker \
             will edit that."
        );
    }
}

/// The USD path of the `Shader` prim behind `part`'s bound material, if it has one:
/// `rel material:binding` → `Material` → `outputs:surface.connect` → `Shader`.
///
/// The same two hops the loader makes (`lunco_usd_sim::shader::bound_shader_prim`), so
/// the Inspector edits exactly the prim the renderer read.
fn bound_shader_prim_path(world: &mut World, part: Entity) -> Option<String> {
    use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdRead};

    let prim = world.get::<UsdPrimPath>(part)?.clone();
    let stage_id = prim.stage_handle.id();
    let sdf = SdfPath::new(&prim.path).ok()?;

    let mut canonical = world.get_non_send_resource_mut::<CanonicalStages>()?;
    let cs = canonical.get(stage_id)?;
    let view = cs.view();

    let material = view.rel_target(&sdf, "material:binding")?;
    let material = SdfPath::new(material.as_str()).ok()?;
    let surface = view.connection_source(&material, "outputs:surface")?;
    let (shader, _) = surface.rsplit_once('.')?;
    Some(shader.to_string())
}

/// The asset path of `part`'s current shader (read via [`PanelCtx`]), or `None` if
/// it isn't using one. The path IS the intent — no material lookup.
fn current_shader_path(ctx: &PanelCtx, part: Entity) -> Option<String> {
    let look = ctx.get::<ShaderLook>(part)?;
    (!look.shader.is_empty()).then(|| look.shader.clone())
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

/// Editable PBR controls for the selected object's surfaces.
///
/// Edits the [`PbrLook`] **intent** component, not the material asset: the render
/// binder re-materialises on `Changed<PbrLook>`. This is not merely tidier — it is
/// required for correctness, because the binder shares one material across every
/// entity with the same look, so the old `Assets::get_mut(handle)` write would now
/// bleed onto unrelated entities that happen to look alike. (It is also what keeps
/// this crate off `bevy_pbr`.) A surface with no `PbrLook` is not listed as a part,
/// so there is nothing here to fall back to.
///
/// Reads a snapshot via [`PanelCtx`]; the component + USD writes are deferred.
fn material_pbr_section(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    part: Entity,
    parts: &[Entity],
) {
    let Some(&first) = parts.first() else {
        return;
    };

    // Snapshot current values — no world borrow held while drawing widgets.
    let Some(look) = ctx.get::<PbrLook>(first) else {
        ui.label("Material still loading…");
        return;
    };
    let snap = {
        let b = look.base_color;
        let e = look.emissive;
        (
            [b.red, b.green, b.blue],
            b.alpha,
            [e.red, e.green, e.blue],
            look.metallic,
            look.perceptual_roughness,
            look.ior,
            look.double_sided,
        )
    };
    let (mut base, mut alpha, mut emissive, mut metallic, mut roughness, mut ior, mut double_sided) = snap;

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
    // Index of refraction — `UsdPreviewSurface`'s `inputs:ior`, and the ONLY specular
    // knob. This slider used to say "Reflectance" and author a private
    // `inputs:reflectance` that no other DCC reads; IOR is the standard spelling of the
    // same physical quantity. 1.0 = vacuum (no Fresnel), 1.5 = glass and most
    // silicates, 2.33 = where Bevy's derived reflectance saturates.
    //
    // There is no "Unlit" checkbox: `PbrLook::unlit` is render-only intent for overlay
    // geometry (trajectory lines, brush rings, labels) with no USD equivalent, so a
    // checkbox here could only edit a value that silently reverted on reload.
    let ior_changed = ui.add(egui::Slider::new(&mut ior, 1.0..=2.33).text("IOR")).changed();
    changed |= ior_changed;
    changed |= ui.checkbox(&mut double_sided, "Double-sided").changed();
    if parts.len() > 1 {
        ui.label(egui::RichText::new(format!("applies to {} parts", parts.len())).weak());
    }

    if changed {
        let parts = parts.to_vec();
        ctx.defer(move |world| {
            for e in &parts {
                // Intent only — the binder re-materialises it (and, sharing by
                // look, gives this entity its own handle if the edit made it
                // unique).
                let Some(mut look) = world.get_mut::<PbrLook>(*e) else { continue };
                look.base_color = LinearRgba::new(base[0], base[1], base[2], alpha);
                look.emissive = LinearRgba::new(emissive[0], emissive[1], emissive[2], 1.0);
                look.metallic = metallic;
                look.perceptual_roughness = roughness;
                look.ior = ior;
                look.double_sided = double_sided;
                look.alpha = if alpha >= 1.0 {
                    lunco_render::SurfaceAlpha::Opaque
                } else {
                    lunco_render::SurfaceAlpha::Blend
                };
            }

            // Propagate changes to USD.
            if let Some(prim) = world.get::<UsdPrimPath>(part).cloned() {
                // Shared with `SetObjectProperty` (commands.rs): both edit a look, so
                // both must agree on where the look LIVES.
                let shader_path = bound_shader_prim(world, &prim);

                // Where do shader inputs go? Onto a `Shader` prim, always —
                // `inputs:*` is the UsdShade namespace and is meaningless on a
                // Gprim. If this mesh has no material yet, BUILD one rather than
                // scribbling shader inputs onto the geometry (which is what this
                // used to do, and which no other DCC would ever read back).
                //
                // The whole thing — Scope + Material + Shader + binding + every
                // changed input — lands as ONE journal change set, so it is one
                // undo unit: undo removes the material it created, not just the
                // last slider you touched.
                let (mut ops, shader) = match &shader_path {
                    Some(sp) => (Vec::new(), sp.clone()),
                    None => {
                        let Some(prim) = world.get::<UsdPrimPath>(part).cloned() else {
                            return;
                        };
                        let Some((ops, shader)) =
                            lunco_usd::material::ensure_preview_surface_ops(&prim.path)
                        else {
                            return;
                        };
                        (ops, shader)
                    }
                };
                // A freshly-created material must reproduce what is on screen
                // right now, not snap to UsdPreviewSurface's defaults — so seed
                // every input, not only the one the user just dragged.
                let fresh = shader_path.is_none();
                let root = LayerId::root();
                let mut set = |attr: &str, ty: &str, value: String| {
                    ops.push(UsdOp::SetAttribute {
                        edit_target: root.clone(),
                        path: shader.clone(),
                        name: attr.to_string(),
                        type_name: ty.to_string(),
                        value,
                    });
                };

                if base_changed || fresh {
                    set(
                        "inputs:diffuseColor",
                        "color3f",
                        format!("({}, {}, {})", base[0], base[1], base[2]),
                    );
                }
                if emissive_changed || fresh {
                    set(
                        "inputs:emissiveColor",
                        "color3f",
                        format!("({}, {}, {})", emissive[0], emissive[1], emissive[2]),
                    );
                }
                if metallic_changed || fresh {
                    set("inputs:metallic", "float", format!("{:.3}", metallic));
                }
                if roughness_changed || fresh {
                    set("inputs:roughness", "float", format!("{:.3}", roughness));
                }
                if ior_changed || fresh {
                    set("inputs:ior", "float", format!("{:.3}", ior));
                }

                if let Some(doc) = resolve_doc_for_entity(world, part) {
                    lunco_usd::commands::apply_ops_as_change_set(
                        world,
                        doc,
                        "Edit material",
                        ops,
                    );
                }
            }
        });
    }
}

/// Reflected schemas keyed by shader asset, so each loaded WGSL source is
/// parsed once — not once per frame while the shader section is open. The
/// `(ptr, len)` pair fingerprints the source `Cow`; a hot-reload swaps the
/// allocation, so the entry re-parses. This crate cannot name the render
/// side's `ShaderSchemas` cache (the Cargo.toml render gate), so it keeps
/// its own.
#[derive(Resource, Default)]
struct ShaderSchemaCache {
    #[allow(clippy::type_complexity)]
    map: std::collections::HashMap<
        bevy::asset::AssetId<bevy::shader::Shader>,
        ((usize, usize), Option<std::sync::Arc<lunco_materials::ParamSchema>>),
    >,
}

/// The reflected [`ParamSchema`](lunco_materials::ParamSchema) of a shader asset
/// path — parsed from the loaded WGSL source, so the editor derives its widgets from
/// the *shader* rather than from a material (or a hardcoded table).
///
/// `None` while the shader is still loading, or if it declares no `Material` struct.
fn shader_schema_of(
    ctx: &mut PanelCtx,
    path: &str,
) -> Option<std::sync::Arc<lunco_materials::ParamSchema>> {
    if path.is_empty() {
        return None;
    }
    let handle = ctx.resource::<AssetServer>()?.load::<bevy::shader::Shader>(path.to_string());
    let id = handle.id();
    let cached = ctx.resource_scope(|ctx, cache: &mut ShaderSchemaCache| {
        let shaders = ctx.resource::<Assets<bevy::shader::Shader>>()?;
        let src = match &shaders.get(id)?.source {
            bevy::shader::Source::Wgsl(s) => s.as_ref(),
            _ => return None,
        };
        let key = (src.as_ptr() as usize, src.len());
        if let Some((k, schema)) = cache.map.get(&id) {
            if *k == key {
                return schema.clone();
            }
        }
        let schema = lunco_materials::ParamSchema::parse(src).map(std::sync::Arc::new);
        cache.map.insert(id, (key, schema.clone()));
        schema
    });
    match cached {
        Some(schema) => schema,
        None => {
            ctx.defer(|world| {
                world.init_resource::<ShaderSchemaCache>();
            });
            None
        }
    }
}

/// Render named, range-bounded controls for the selected entity's [`ShaderLook`]
/// parameters.
///
/// The rows are DERIVED — from the shader's own `ParamSchema` (reflected out of its
/// WGSL `Material` struct), never from a hand-written list; the current values come
/// from `ShaderLook::values` (falling back to each field's declared default). An
/// edit mutates the component and the binder re-materialises on `Changed<ShaderLook>`,
/// so nothing here touches a material asset.
///
/// Reads a snapshot via [`PanelCtx`]; the component + USD writes are deferred.
fn shader_parameters_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, entity: Entity) {
    use lunco_materials::{ParamType, ParamValue, UiKind};

    let Some(look) = ctx.get::<ShaderLook>(entity).cloned() else {
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
        // The schema is a property of the ASSET: read it from the loaded WGSL
        // source. `None` = the shader hasn't loaded yet.
        let Some(schema) = shader_schema_of(ctx, &look.shader) else {
            ui.label("Shader still loading…");
            return;
        };
        schema
            .fields
            .iter()
            .filter(|f| !matches!(f.ui, UiKind::Engine))
            .map(|f| {
                let floats = look
                    .values
                    .get(&f.name)
                    .copied()
                    .or(f.default)
                    .map(|v| v.as_floats())
                    .unwrap_or_default();
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
            if let Some(mut look) = world.get_mut::<ShaderLook>(entity) {
                for (name, v) in edits.iter() {
                    look.values.insert(name.clone(), *v);
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
    use lunco_celestial::{GeodeticAnchor, KeplerOrbit};

    let anchor = ctx.get::<GeodeticAnchor>(entity).copied();
    let orbit = ctx.get::<KeplerOrbit>(entity).copied();
    if anchor.is_none() && orbit.is_none() {
        return;
    }

    egui::CollapsingHeader::new("Anchor & Orbit")
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

        });
    ui.separator();
}

/// Apply a sequence of typed [`UsdOp`]s to `entity`'s backing document, in order —
/// each journals and inverts on its own. Used by the mount snap, which re-authors a
/// part's transform + joint anchor as four ops.
fn apply_usd_ops(world: &mut World, entity: Entity, ops: Vec<UsdOp>) {
    let Some(doc) = resolve_doc_for_entity(world, entity) else {
        return;
    };
    for op in ops {
        world.trigger(ApplyUsdOp { doc, op });
    }
}

fn apply_usd_path_attribute_change(
    world: &mut World,
    entity: Entity,
    prim_path: String,
    name: &str,
    type_name: &str,
    value: String,
) {
    if let Some(doc) = resolve_doc_for_entity(world, entity) {
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

//! Inspector panel — WorkbenchPanel implementation.
//!
//! Migrates the old standalone egui window to use bevy_workbench docking.
//! Provides editable sliders for transform, physics, and wheel parameters.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};
use lunco_mobility::WheelRaycast;

use crate::{SelectedEntity, UndoStack, UndoAction};

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
            .show(ui, |ui| inspector_content(self, ui, world));
    }
}

fn inspector_content(_panel: &mut Inspector, ui: &mut egui::Ui, world: &mut World) {

        // Delete hotkey
        if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
            if let Some(entity) = world.get_resource::<SelectedEntity>().and_then(|s| s.entity) {
                if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                    undo.push(UndoAction::Spawned { entity });
                }
                if world.get_entity(entity).is_ok() {
                    world.commands().entity(entity).despawn();
                }
                if let Some(mut selected) = world.get_resource_mut::<SelectedEntity>() {
                    selected.entity = None;
                }
                return;
            }
        }

        // Esc / Backspace deselection lives in the Bevy `handle_entity_selection`
        // system (the single mutation path), not here — mutating the World
        // mid-egui-render fought the next frame's selection + shader swap.

        ui.heading("Inspector");

        // ── Environment (sun + ambient) ──────────────────────────────
        // Always visible — a directional light has no clickable geometry,
        // so click-selection can never reach it. Edits write the LIVE
        // light components/resources directly; they are session-transient
        // (persisting back into the scene layer is the save-scene
        // workstream).
        environment_section(ui, world);
        ui.separator();

        // ── Terrain shader (always visible) ──────────────────────────
        // Like the sun, the terrain is the "world": its regolith shader is
        // the scene's dominant surface and the terrain is `Ground`
        // (deliberately excluded from click-selection so ground clicks
        // deselect / drive the camera). So expose its shader params here
        // unconditionally — no selection needed.
        terrain_shader_section(ui, world);

        // Get current selection
        let Some(entity) = world.get_resource::<SelectedEntity>().and_then(|s| s.entity) else {
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

        // ── Shader material parameters ───────────────────────────────
        // Named, range-bounded controls for the selected entity's
        // `ShaderMaterial` (regolith/wheel/balloon/…), driven by the
        // manifest in `lunco-materials`. Edits mutate the live material
        // asset in place — same immediate-feedback path as Transform.
        let has_shader_mat = world
            .query::<&MeshMaterial3d<lunco_materials::ShaderMaterial>>()
            .get(world, entity)
            .is_ok();
        if has_shader_mat {
            egui::CollapsingHeader::new("Shader Parameters")
                .default_open(true)
                .show(ui, |ui| {
                    shader_parameters_section(ui, world, entity, true);
                });
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

        // Delete button
        ui.separator();
        if ui.button("🗑 Delete Entity (Del)").clicked() {
            if let Some(mut undo) = world.get_resource_mut::<UndoStack>() {
                undo.push(UndoAction::Spawned { entity });
            }
            if world.get_entity(entity).is_ok() {
                world.commands().entity(entity).despawn();
            }
            if let Some(mut selected) = world.get_resource_mut::<SelectedEntity>() {
                selected.entity = None;
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
    // the preview light's state instead of the scene sun's.
    let suns: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<DirectionalLight>,
            Without<bevy::camera::visibility::RenderLayers>,
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
        });

    if any_change {
        world.trigger(cmd);
    }
}

/// Always-visible terrain shader controls. Auto-finds the horizon terrain
/// (the `HorizonShadowTerrain` carrying a [`ShaderMaterial`]) and renders its
/// named params inline — the terrain is `Ground`, so click-selection can't
/// reach it; this is its editing home, mirroring the always-on sun controls.
fn terrain_shader_section(ui: &mut egui::Ui, world: &mut World) {
    let terrain = world
        .query_filtered::<Entity, (
            With<lunco_core::HorizonShadowTerrain>,
            With<MeshMaterial3d<lunco_materials::ShaderMaterial>>,
        )>()
        .iter(world)
        .next();
    let Some(entity) = terrain else { return };
    egui::CollapsingHeader::new("Terrain Shader")
        .default_open(true)
        .show(ui, |ui| {
            shader_parameters_section(ui, world, entity, false);
        });
    ui.separator();
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
fn shader_parameters_section(ui: &mut egui::Ui, world: &mut World, entity: Entity, show_picker: bool) {
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
    let (shader_file, current_path, rows): (Option<String>, Option<String>, Vec<Row>) = {
        let mats = world.resource::<Assets<ShaderMaterial>>();
        let Some(mat) = mats.get(&handle) else {
            ui.label("Material still loading…");
            return;
        };
        let full = world
            .resource::<AssetServer>()
            .get_path(mat.shader.id())
            .map(|p| p.path().to_string_lossy().into_owned());
        let file = full
            .as_ref()
            .map(|s| s.rsplit(['/', '\\']).next().unwrap_or(s).to_string());
        let rows = mat
            .schema
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
            .collect();
        (file, full, rows)
    };

    // Shader picker — swap which `.wgsl` this prop renders with. Dispatches
    // `SetObjectProperty { property: "shader" }`, the same uniform-preserving
    // swap path USD authoring uses. Hidden for the terrain section (terrain is
    // not a swappable prop and isn't in the API registry), which falls back to
    // a plain "Shader: …" label.
    let picked = if show_picker {
        let api_id = world
            .get_resource::<lunco_api::registry::ApiEntityRegistry>()
            .and_then(|r| r.api_id_for(entity))
            .map(|g| g.get());
        let entries = world
            .get_resource::<lunco_materials::ShaderCatalog>()
            .map(|c| c.entries.clone())
            .unwrap_or_default();
        match (api_id, entries.is_empty()) {
            (Some(id), false) => Some((id, entries)),
            _ => None,
        }
    } else {
        None
    };
    if let Some((api_id, entries)) = picked {
        let cur = current_path.clone().unwrap_or_default();
        let cur_label = entries
            .iter()
            .find(|e| e.path == cur)
            .map(|e| e.label.clone())
            .or_else(|| shader_file.clone())
            .unwrap_or_else(|| "—".into());
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
                world.trigger(crate::commands::SetObjectProperty {
                    entity_id: api_id,
                    property: "shader".into(),
                    value: path,
                });
            }
        }
    } else if let Some(f) = &shader_file {
        ui.label(egui::RichText::new(format!("Shader: {f}")).weak());
    }
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
        if let Some(mut mats) = world.get_resource_mut::<Assets<ShaderMaterial>>() {
            if let Some(mat) = mats.get_mut(&handle) {
                for (name, v) in edits {
                    mat.set(&name, v);
                }
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
    use lunco_modelica::ui::ModelicaDocumentRegistry;
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

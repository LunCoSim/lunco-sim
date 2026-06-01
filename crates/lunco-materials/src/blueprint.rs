//! Blueprint Material
//!
//! Blueprint grid shader + material type + USD post-sync system.
//! The canonical definition lives here — `lunco-celestial` imports it.

use bevy::prelude::*;
use bevy::pbr::{MaterialExtension, MaterialPlugin, ExtendedMaterial};
use bevy::render::render_resource::AsBindGroup;
#[cfg(target_arch = "wasm32")]
use bevy::asset::load_internal_asset;
use bevy::shader::{Shader, ShaderRef};
use std::marker::PhantomData;
use uuid::Uuid;

use openusd::sdf::Path as SdfPath;

/// Asset path of the blueprint shader, relative to the `assets/` root. Used on
/// native so editing the `.wgsl` hot-reloads.
#[cfg(not(target_arch = "wasm32"))]
const BLUEPRINT_SHADER_PATH: &str = "shaders/blueprint_extension.wgsl";

/// UUID for the blueprint shader. **Load-bearing on wasm**: the `embed-assets`
/// build has no filesystem, so `lunco-celestial`'s `EmbeddedAssetsPlugin`
/// registers the shader source to this const handle. Native loads by path
/// instead (see [`BlueprintExtension::fragment_shader`]).
const BLUEPRINT_SHADER_UUID: Uuid = Uuid::from_u128(0x1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d);
pub const BLUEPRINT_SHADER_HANDLE: Handle<Shader> = Handle::Uuid(BLUEPRINT_SHADER_UUID, PhantomData);

/// Blueprint extension uniforms.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct BlueprintExtension {
    #[uniform(100)]
    pub high_color: LinearRgba,
    #[uniform(100)]
    pub low_color: LinearRgba,
    #[uniform(100)]
    pub high_line_color: LinearRgba,
    #[uniform(100)]
    pub low_line_color: LinearRgba,
    #[uniform(100)]
    pub subdivisions: Vec2,
    #[uniform(100)]
    pub fade_range: Vec2,
    #[uniform(100)]
    pub grid_scale: f32,
    #[uniform(100)]
    pub line_width: f32,
    #[uniform(100)]
    pub transition: f32,
    #[uniform(100)]
    pub body_radius: f32,
    #[uniform(100)]
    pub major_grid_spacing: f32,
    #[uniform(100)]
    pub minor_grid_spacing: f32,
    #[uniform(100)]
    pub major_line_width: f32,
    #[uniform(100)]
    pub minor_line_width: f32,
    #[uniform(100)]
    pub minor_line_fade: f32,
    #[uniform(100)]
    pub surface_color: LinearRgba,
}

impl Default for BlueprintExtension {
    fn default() -> Self {
        Self {
            high_color: LinearRgba::WHITE,
            low_color: LinearRgba::WHITE,
            high_line_color: LinearRgba::new(1.0, 1.0, 1.0, 1.0),
            low_line_color: LinearRgba::new(1.0, 1.0, 1.0, 1.0),
            subdivisions: Vec2::new(24.0, 12.0),
            fade_range: Vec2::new(0.2, 0.6),
            grid_scale: 10.0,
            line_width: 2.0,
            transition: 0.0,
            body_radius: 1737_000.0,
            major_grid_spacing: 1.0,
            minor_grid_spacing: 0.5,
            major_line_width: 0.75,
            minor_line_width: 0.4,
            minor_line_fade: 0.3,
            surface_color: LinearRgba::new(0.2, 0.2, 0.2, 1.0),
        }
    }
}

impl MaterialExtension for BlueprintExtension {
    fn fragment_shader() -> ShaderRef {
        // Native: load by path → hot-reloadable. Wasm: the embedded const handle.
        #[cfg(not(target_arch = "wasm32"))]
        { BLUEPRINT_SHADER_PATH.into() }
        #[cfg(target_arch = "wasm32")]
        { BLUEPRINT_SHADER_HANDLE.into() }
    }
}

/// Blueprint material type.
pub type BlueprintMaterial = ExtendedMaterial<StandardMaterial, BlueprintExtension>;

/// Plugin that registers the blueprint shader, material, and USD post-sync system.
pub struct BlueprintMaterialPlugin;

impl Plugin for BlueprintMaterialPlugin {
    fn build(&self, app: &mut App) {
        // Native loads the shader from `assets/` by path (hot-reload). Only wasm
        // (no filesystem) needs the source embedded to the const handle.
        #[cfg(target_arch = "wasm32")]
        load_internal_asset!(
            app,
            BLUEPRINT_SHADER_HANDLE,
            "../../../assets/shaders/blueprint_extension.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<BlueprintMaterial>::default());
        // Observer-driven, NOT a per-frame poll. Fires once per prim, the
        // moment `sync_usd_visuals` inserts `UsdVisualSynced`. See the doc
        // comment on `apply_blueprint_material` for why this shape makes the
        // §7.5 perf bugs structurally impossible.
        app.add_observer(apply_blueprint_material);
    }
}

/// Applies [`BlueprintMaterial`] to a USD prim the instant its visuals are
/// instantiated, if the prim declares `primvars:materialType = "BlueprintGrid"`.
///
/// **Observer-driven, not a per-frame poll** — fires exactly once per prim,
/// triggered by `sync_usd_visuals` inserting `UsdVisualSynced`. This shape
/// removes the `Without<Marker>` poll gate (so no per-frame full-scene re-scan)
/// and borrows the reader (so no whole-stage deep clone) — the two recurring
/// §7.5 regressions become structurally impossible.
///
/// Needs no headless guard: the observer only fires when `UsdVisualSynced` is
/// added, which only happens once `Assets<UsdStageAsset>` exists.
fn apply_blueprint_material(
    trigger: On<Add, lunco_usd_bevy::UsdVisualSynced>,
    q: Query<&lunco_usd_bevy::UsdPrimPath>,
    stages: Res<Assets<lunco_usd_bevy::UsdStageAsset>>,
    mut materials: ResMut<Assets<BlueprintMaterial>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = q.get(entity) else { return };
    let Some(stage) = stages.get(&prim_path.stage_handle) else { return };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return };
    let reader = &*stage.reader;

    let mat_type: Option<String> = reader.prim_attribute_value(&sdf_path, "primvars:materialType");
    if mat_type.as_deref() != Some("BlueprintGrid") {
        return;
    }

    let bp_mat = create_blueprint_material(reader, &sdf_path, &mut materials);
    // Replace the default StandardMaterial so the prim renders with one material.
    commands
        .entity(entity)
        .remove::<MeshMaterial3d<StandardMaterial>>()
        .insert(MeshMaterial3d(bp_mat));
}

/// Creates a BlueprintMaterial from USD primvars attributes.
fn create_blueprint_material(
    reader: &openusd::usda::TextReader,
    sdf_path: &SdfPath,
    materials: &mut ResMut<Assets<BlueprintMaterial>>,
) -> Handle<BlueprintMaterial> {
    let surface_color = reader.prim_attribute_value::<Vec<f64>>(sdf_path, "primvars:gridSurfaceColor")
        .unwrap_or_else(|| vec![0.2, 0.2, 0.2]);
    let major_spacing = reader.prim_attribute_value::<f64>(sdf_path, "primvars:gridMajorSpacing")
        .unwrap_or(1.0) as f32;
    let minor_spacing = reader.prim_attribute_value::<f64>(sdf_path, "primvars:gridMinorSpacing")
        .unwrap_or(0.5) as f32;
    let major_width = reader.prim_attribute_value::<f64>(sdf_path, "primvars:gridMajorWidth")
        .unwrap_or(1.0) as f32;
    let minor_width = reader.prim_attribute_value::<f64>(sdf_path, "primvars:gridMinorWidth")
        .unwrap_or(0.5) as f32;
    let minor_fade = reader.prim_attribute_value::<f64>(sdf_path, "primvars:gridMinorFade")
        .unwrap_or(0.15) as f32;

    let r = surface_color.get(0).copied().unwrap_or(0.2) as f32;
    let g = surface_color.get(1).copied().unwrap_or(0.2) as f32;
    let b = surface_color.get(2).copied().unwrap_or(0.2) as f32;

    let bp_ext = BlueprintExtension {
        high_color: LinearRgba::new(0.5, 0.5, 0.5, 1.0),
        low_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
        high_line_color: LinearRgba::new(r + 0.05, g + 0.05, b + 0.05, 1.0),
        low_line_color: LinearRgba::new(r + 0.05, g + 0.05, b + 0.05, 1.0),
        surface_color: LinearRgba::new(r, g, b, 1.0),
        grid_scale: 1.0,
        line_width: 2.0,
        subdivisions: Vec2::new(10.0, 10.0),
        transition: 0.85,
        major_grid_spacing: major_spacing,
        minor_grid_spacing: minor_spacing,
        major_line_width: major_width,
        minor_line_width: minor_width,
        minor_line_fade: minor_fade,
        ..Default::default()
    };

    materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(r, g, b),
            perceptual_roughness: 0.9,
            ..default()
        },
        extension: bp_ext,
    })
}

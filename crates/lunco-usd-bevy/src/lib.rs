//! # LunCoSim USD → Bevy Visual Sync
//!
//! Responsible for spawning child entities for USD prims and attaching visual components
//! (meshes, materials, transforms). This is the **first** plugin in the USD processing
//! pipeline — it must run before the Avian physics and Sim simulation plugins.
//!
//! ## How It Works
//!
//! 1. The asset loader (`UsdLoader`) reads a `.usda` file, parses it, and resolves all
//!    external references (e.g., wheel component files) via `UsdComposer::flatten()`.
//! 2. The `sync_usd_visuals` system iterates over all entities with `UsdPrimPath` that
//!    haven't been processed yet (`Without<UsdVisualSynced>`).
//! 3. For each prim, it creates a mesh based on the prim type (`Cube`, `Cylinder`, `Sphere`)
//!    using explicit dimensions from the USD file.
//! 4. It spawns child entities for each prim child, pre-populating their transforms so
//!    physics systems see them in the correct positions.
//!
//! ## Coordinate Systems
//!
//! USD uses Y-up, +Z-forward. Bevy uses Y-up, -Z-forward. The USD files store rotation
//! in degrees via `xformOp:rotateXYZ`. This system converts them to radians and applies
//! them as Bevy quaternions.
//!
//! ## Mesh Dimensions
//!
//! Bevy's `Cuboid::new()` and `Collider::cuboid()` take **full dimensions**, not
//! half-extents. The USD files store full dimensions (`width`, `height`, `depth`),
//! so no scaling is needed.
//!
//! ## Why Not Use the Observer?
//!
//! The `On<Add, UsdPrimPath>` observer fires when the entity is spawned, but the USD
//! asset may not be loaded yet (async loading). The `sync_usd_visuals` system runs in
//! the `Update` schedule and retries every frame until the asset is available, then
//! marks the entity with `UsdVisualSynced` to prevent re-processing.

use bevy::prelude::*;
use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use openusd::usda::TextReader;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use lunco_usd_composer::UsdComposer;
use big_space::prelude::CellCoord;
use std::sync::Arc;

/// Bevy plugin for USD visual synchronization.
///
/// Registers the `UsdStageAsset` type, the USD asset loader, and the `sync_usd_visuals`
/// system that processes USD prims into Bevy entities with meshes and transforms.
pub struct UsdBevyPlugin;

impl Plugin for UsdBevyPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<UsdStageAsset>()
            .register_asset_loader(UsdLoader)
            .register_type::<UsdPrimPath>()
            .add_observer(on_usd_prim_added)
            // `sync_usd_visuals` runs only on frames where a stage's
            // `LoadedWithDependencies` event was emitted. Idle frames
            // skip it entirely (run-condition short-circuits).
            .add_systems(
                Update,
                (
                    sync_usd_visuals.run_if(bevy::ecs::schedule::common_conditions::on_message::<AssetEvent<UsdStageAsset>>),
                    hide_glb_placeholder_meshes,
                ),
            );
    }
}

/// A Bevy Asset representing a loaded USD Stage.
///
/// Contains a flattened USD reader with all external references resolved.
/// Created by the `UsdLoader` asset loader when a `.usda` file is loaded.
#[derive(Asset, TypePath, Clone)]
pub struct UsdStageAsset {
    /// Flattened USD reader with all references resolved.
    pub reader: Arc<TextReader>,
}

#[derive(Default, TypePath)]
pub struct UsdLoader;

impl AssetLoader for UsdLoader {
    type Asset = UsdStageAsset;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        // Read raw bytes from the .usda file
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let data = String::from_utf8(bytes)?;

        // Parse the USD text format
        let mut parser = openusd::usda::parser::Parser::new(&data);
        let data_map = parser.parse().map_err(|e| anyhow::anyhow!("USD Parse Error: {}", e))?;
        let reader = TextReader::from_data(data_map);

        // Resolve external references. Two conventions are supported:
        //  * `/`-prefixed (legacy): composer walks up to the `assets/`
        //    root and resolves the path against it.
        //  * Layer-relative `@../../foo.usda@` (USD-spec / Pixar form,
        //    portable to Blender / usdview / Houdini): resolved against
        //    the `.usda`'s **own parent directory**.
        //
        // Both forms need `base_dir` to be the layer's parent directory
        // — the composer's `flatten_recursive` joins relative paths
        // against it. Earlier this passed just `assets/`, which made
        // `../../vessels/...` resolve to `assets/../../vessels/...`
        // (out of the workspace). Joining the load-context-relative
        // parent onto `assets/` fixes it.
        let reader = if let Some(parent) = load_context.path().path().parent() {
            let asset_root = std::path::Path::new("assets");
            // On native, base_dir lives under `assets/` so the composer's
            // filesystem reads see real on-disk paths; on wasm (no fs)
            // base_dir stays asset-source-relative and sublayers are
            // pre-fetched through the AssetServer below.
            let base_dir = if asset_root.exists() {
                asset_root.join(parent)
            } else {
                parent.to_path_buf()
            };

            #[cfg(target_arch = "wasm32")]
            {
                // Pre-fetch every transitively-referenced .usda via the
                // AssetServer (the only filesystem-like API available on
                // wasm). The composer then runs synchronously off the
                // in-memory map.
                use lunco_usd_composer::{collect_sublayer_paths, normalize_path};
                let mut fetched: std::collections::HashMap<std::path::PathBuf, openusd::usda::TextReader> = std::collections::HashMap::new();
                let mut queue: Vec<(std::path::PathBuf, openusd::usda::TextReader)> = Vec::new();
                queue.push((base_dir.clone(), reader.clone()));
                while let Some((cur_dir, cur_reader)) = queue.pop() {
                    for sub in collect_sublayer_paths(&cur_reader, &cur_dir) {
                        let key = normalize_path(&sub);
                        if fetched.contains_key(&key) { continue; }
                        let bytes = load_context.read_asset_bytes(key.clone()).await
                            .map_err(|e| anyhow::anyhow!("Failed to fetch sublayer {}: {}", key.display(), e))?;
                        let text = String::from_utf8(bytes)
                            .map_err(|e| anyhow::anyhow!("Sublayer {} is not UTF-8: {}", key.display(), e))?;
                        let mut sub_parser = openusd::usda::parser::Parser::new(&text);
                        let sub_data = sub_parser.parse().map_err(|e| anyhow::anyhow!("USD Parse Error in {}: {}", key.display(), e))?;
                        let sub_reader = openusd::usda::TextReader::from_data(sub_data);
                        let parent_dir = key.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf();
                        fetched.insert(key.clone(), sub_reader.clone());
                        queue.push((parent_dir, sub_reader));
                    }
                }
                let mut fetcher = |p: &std::path::Path| -> anyhow::Result<openusd::usda::TextReader> {
                    let key = normalize_path(p);
                    fetched.get(&key)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("Sublayer not pre-fetched: {}", key.display()))
                };
                UsdComposer::flatten_with_fetcher(&reader, &base_dir, &mut fetcher)
                    .map_err(|e| anyhow::anyhow!("USD Composition Error: {}", e))?
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                UsdComposer::flatten(&reader, &base_dir).map_err(|e| anyhow::anyhow!("USD Composition Error: {}", e))?
            }
        } else {
            reader
        };

        Ok(UsdStageAsset {
            reader: Arc::new(reader),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["usda"]
    }
}

/// Marks an entity as representing a USD prim path.
///
/// This component is added to every entity that corresponds to a USD prim. The system
/// uses it to look up the prim's attributes from the loaded USD stage.
///
/// # Fields
/// - `stage_handle`: Handle to the loaded `UsdStageAsset`
/// - `path`: USD prim path (e.g., `/SandboxRover` or `/SandboxRover/Wheel_FL`)
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdPrimPath {
    /// Handle to the loaded USD stage asset.
    pub stage_handle: Handle<UsdStageAsset>,
    /// USD prim path within the stage (e.g., `/SandboxRover/Wheel_FL`).
    pub path: String,
}

impl Default for UsdPrimPath {
    fn default() -> Self {
        Self {
            stage_handle: Handle::default(),
            path: "/".to_string(),
        }
    }
}

/// Marker component indicating that an entity has been processed by `sync_usd_visuals`.
///
/// Prevents the system from re-processing the same entity on subsequent frames.
#[derive(Component)]
pub struct UsdVisualSynced;

/// Marker placed on a USD scene root that exists purely to render a
/// preview thumbnail. Plugins that activate simulation side-effects on
/// USD prims (avatar cameras, vehicle FSW, wheel physics) should walk
/// each candidate prim's `ChildOf` ancestry and bail if any ancestor
/// carries this marker — preview-only stages must show geometry but
/// must not spawn cameras into the window or insert physics bodies
/// into the live world.
#[derive(Component, Default, Debug, Clone, Copy)]
pub struct UsdPreviewOnly;

/// Attached to a scene-root entity to tell the USD instantiator where to
/// place top-level USD prims. When this component is present, each
/// direct USD child spawns as a `GridAnchor` parented to the target
/// Grid — *not* as a Bevy child of this entity.
///
/// This is what enforces the architectural rule: top-level USD prims
/// (rovers, balls, terrain) become Grid-direct entities so big_space's
/// `propagate_high_precision` runs on them; their own descendants
/// remain plain-`Transform` children of their USD parent's Bevy entity.
#[derive(Component, Debug, Clone, Copy)]
pub struct LoadIntoGrid(pub Entity);

/// Marker placed on an entity whose `UsdPrimPath` was added before the
/// referenced `UsdStageAsset` finished loading. `on_stage_loaded`
/// processes it once the asset becomes available.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdAwaitingStage;

/// Translates a single USD prim into Bevy/big_space/avian components on
/// `entity`. The caller has already verified that the stage is loaded.
///
/// **Steady-state cost: zero** — this is invoked exactly once per entity,
/// either by `on_usd_prim_added` (entity spawned after stage loaded) or
/// by `on_stage_loaded` (entity spawned before stage loaded; drained
/// from the `UsdAwaitingStage` queue when the asset becomes ready). No
/// per-frame polling.
///
/// 1. Looks up the prim's attributes from the loaded USD stage.
/// 2. Creates a mesh based on prim type (Cube, Cylinder, Sphere).
/// 3. Applies the prim's transform (position + rotation + scale).
/// 4. Spawns child entities for each prim child, applying the natural
///    anchor rule via `LoadIntoGrid` (top-level → `GridAnchor`).
/// 5. Marks the entity with `UsdVisualSynced` to prevent re-processing.
///
/// Custom materials (solar panels, blueprint grids, etc.) are applied
/// by independent material plugins in `lunco-materials` that observe
/// the `UsdVisualSynced` insertion.
#[allow(clippy::too_many_arguments)]
fn instantiate_usd_prim(
    entity: Entity,
    prim_path: &UsdPrimPath,
    existing_vis: Option<&Visibility>,
    existing_tf: Option<&Transform>,
    load_into_grid: Option<&LoadIntoGrid>,
    commands: &mut Commands,
    stages: &Assets<UsdStageAsset>,
    asset_server: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { return; };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return; };

        // Borrow — `stage.reader` is `Arc<TextReader>`; deep-cloning it copies
        // the whole stage `HashMap`. Every read below is `&self`.
        let reader = &*stage.reader;

        // Skip inactive prims
        if let Ok(val) = reader.get(&sdf_path, "active") {
            if let Value::Bool(active) = &*val {
                if !*active {
                    commands.entity(entity).insert(UsdVisualSynced);
                    return;
                }
            }
        }

        // Get prim type (Cube, Cylinder, Sphere, etc.)
        let prim_type = if let Ok(val) = reader.get(&sdf_path, "typeName") {
            if let Value::Token(ty) = &*val {
                Some(ty.clone())
            } else {
                None
            }
        } else {
            None
        };

        // Visibility — honour standard USD `token visibility`.
        // `invisible` suppresses mesh creation entirely (used for
        // collider-only Cube prims hidden behind a glTF visual, and
        // raycast wheel cylinders that have no visible representation).
        let invisible = matches!(
            get_attribute_as_string(reader, &sdf_path, "visibility").as_deref(),
            Some("invisible")
        );

        // Placeholder for an async-loading glTF payload. Authors set
        // `bool lunco:placeholder = true` on a Cube prim that lives as
        // a sibling of an `Xform "Visual" (payload = @lunco-lib://...@)`.
        // Third-party USD tools render it (they don't know our
        // attribute or the `lunco-lib://` scheme); our pipeline starts
        // it `Visibility::Hidden` so the user doesn't see a brief
        // tan-cube flash before the photoreal glTF replaces it. Mesh
        // is still built — visibility is the toggle. (Future: reveal
        // on `AssetServer::load_state(...).is_failed()`.)
        let is_placeholder = reader
            .prim_attribute_value::<bool>(&sdf_path, "lunco:placeholder")
            .unwrap_or(false);

        // **Placeholder + payload pattern**: when `lunco:resolvedAsset`
        // is present, we still build the primitive Cube/Sphere/Cylinder
        // mesh so the prim has a fallback visual until the glTF Scene
        // finishes loading. Once Bevy reports the Scene asset loaded,
        // `hide_glb_placeholder_meshes` (below) hides the primitive
        // Mesh3d so the photoreal glTF replaces it cleanly.
        //
        // Authors size the placeholder Cube ≈ glTF bbox; mismatched
        // scales briefly show a tan border around the rover during
        // loading and as fallback when the asset is missing.

        // Create mesh based on prim type and **spec-compliant** USD
        // attributes:
        //   * `Cube`     : `double size` (default 2.0) — UsdGeomCube
        //   * `Sphere`   : `double radius` (default 1.0) — UsdGeomSphere
        //   * `Cylinder` : `double radius`, `double height` — UsdGeomCylinder
        // Authors compose non-uniform dimensions via `xformOp:scale`
        // — exactly how Pixar USD / Houdini / Blender expect it.
        //
        // **Legacy fallback**: `width`/`height`/`depth` on Cube prims is
        // still accepted so older `.usda` files keep working during the
        // migration. New authoring should use `size` + `xformOp:scale`.
        let mesh_handle: Option<Handle<Mesh>> = if invisible {
            None
        } else {
            match prim_type.as_deref() {
                Some("Cube") => {
                    let size = reader.prim_attribute_value::<f64>(&sdf_path, "size").unwrap_or(2.0) as f32;
                    if let (Some(width), Some(height), Some(depth)) = (
                        reader.prim_attribute_value::<f64>(&sdf_path, "width"),
                        reader.prim_attribute_value::<f64>(&sdf_path, "height"),
                        reader.prim_attribute_value::<f64>(&sdf_path, "depth"),
                    ) {
                        // Legacy form — width/height/depth are *full*
                        // extents and bake into the mesh directly.
                        Some(meshes.add(Cuboid::new(width as f32, height as f32, depth as f32)))
                    } else {
                        // Spec form: unit-ish Cube; xformOp:scale handles
                        // non-uniform dimensions (set on Transform below).
                        Some(meshes.add(Cuboid::new(size, size, size)))
                    }
                }
                Some("Sphere") => {
                    let radius = reader
                        .prim_attribute_value::<f64>(&sdf_path, "radius")
                        .unwrap_or(1.0) as f32;
                    // Lat-long (UV) sphere, NOT an icosphere: a UV sphere has a
                    // clean rectangular UV unwrap (uv.x = longitude, uv.y =
                    // pole-to-pole latitude), which our ShaderMaterial checker
                    // (e.g. shaders/balloon.wgsl) needs to tile across the whole
                    // surface. An icosphere's UVs are distorted/seamed and leave
                    // large uncovered-looking patches.
                    Some(meshes.add(Sphere::new(radius).mesh().uv(48, 32)))
                }
                Some("Cylinder") => {
                    let radius = reader
                        .prim_attribute_value::<f64>(&sdf_path, "radius")
                        .unwrap_or(1.0) as f32;
                    let height = reader
                        .prim_attribute_value::<f64>(&sdf_path, "height")
                        .unwrap_or(2.0) as f32;
                    Some(meshes.add(Cylinder::new(radius, height)))
                }
                _ => None,
            }
        };

        // Apply standard PBR material with USD color
        if let Some(ref m) = mesh_handle {
            apply_standard_material(reader, &sdf_path, m, materials, &mut commands.entity(entity));
        }

        // glTF / external-mesh branch.
        //
        // The composer writes `lunco:resolvedAsset` onto any prim whose
        // `payload`/`references` point at a non-USD binary (`.glb`,
        // `.gltf`, `.obj`, `.stl`). We hand the URI to Bevy's
        // `AssetServer` directly — the registered asset sources
        // (`lunco-lib://` for shipped fixtures, default `assets://` for
        // in-tree paths) handle the lookup.
        //
        // - `lunco:assetMode = "mesh"` (default `"scene"`): pull a
        //   single primitive out of the glTF and attach as `Mesh3d`.
        //   Used when the prim should also drive a physics collider —
        //   stays compatible with `lunco-usd-avian` mesh-collider
        //   pipelines.
        // - `lunco:assetMode = "scene"`: load the full glTF scene and
        //   attach as a `SceneRoot` child. Preserves hierarchy,
        //   materials, and lights at the cost of being opaque to the
        //   USD prim-path tree.
        if let Some(asset_uri) = get_attribute_as_string(reader, &sdf_path, "lunco:resolvedAsset") {
            let mode = get_attribute_as_string(reader, &sdf_path, "lunco:assetMode")
                .unwrap_or_else(|| "scene".to_string());
            let label = get_attribute_as_string(reader, &sdf_path, "lunco:assetLabel");

            match mode.as_str() {
                "mesh" => {
                    let label = label.unwrap_or_else(|| "Mesh0/Primitive0".to_string());
                    let path = format!("{asset_uri}#{label}");
                    let mesh_h: Handle<Mesh> = asset_server.load(&path);
                    // Single-mesh path keeps `lunco-usd-avian` collider
                    // construction unchanged — the entity ends up with
                    // a `Mesh3d` exactly like the Cube/Sphere branches.
                    apply_standard_material(
                        reader,
                        &sdf_path,
                        &mesh_h,
                        materials,
                        &mut commands.entity(entity),
                    );
                }
                _ => {
                    let label = label.unwrap_or_else(|| "Scene0".to_string());
                    let path = format!("{asset_uri}#{label}");
                    let scene_h: Handle<Scene> = asset_server.load(&path);
                    // Mark the entity so `hide_glb_placeholder_meshes`
                    // can drop the placeholder Mesh3d once this Scene
                    // finishes loading. The marker is harmless if the
                    // entity has no Mesh3d (e.g. `def Xform` without a
                    // primitive fallback).
                    commands.entity(entity)
                        .insert(SceneRoot(scene_h))
                        .insert(GlbPlaceholder);
                }
            }
        }

        // Transform (position and rotation)
        // Preserve any existing transform set by the spawning code (e.g., rover position).
        // Only override position/rotation if the USD prim has explicit NON-ZERO values.
        // A zero translation in USD means "no offset" — it shouldn't overwrite a spawn position.
        let mut transform = existing_tf.cloned().unwrap_or_default();
        if let Some(v) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:translate") {
            // Only apply USD translation if it's non-zero (to avoid overwriting spawn positions)
            if v.length_squared() > 1e-6 {
                transform.translation = v;
            }
        }
        if let Some(v) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:rotateXYZ") {
            // USD stores rotation in degrees; convert to radians for Bevy
            // Only apply USD rotation if it's non-zero (to preserve existing spawn rotation)
            let is_zero = v.x.abs() < 1e-6 && v.y.abs() < 1e-6 && v.z.abs() < 1e-6;
            if !is_zero {
                let rx = v.x.to_radians();
                let ry = v.y.to_radians();
                let rz = v.z.to_radians();
                transform.rotation = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
            }
        }
        // UsdGeomCylinder.axis token (X|Y|Z, default Z). Compose the
        // axis-induced rotation onto the entity Transform so a Y-axis
        // Bevy `Cylinder` mesh appears along the authored axis without
        // an explicit `xformOp:rotateXYZ` hack. Goes after rotateXYZ so
        // it applies on top of any user-authored rotation.
        if matches!(prim_type.as_deref(), Some("Cylinder")) {
            let axis = match reader.get(&sdf_path, "axis") {
                Ok(v) => match &*v {
                    Value::Token(t) | Value::String(t) => Some(t.clone()),
                    _ => None,
                },
                Err(_) => None,
            }
            .or_else(|| get_attribute_as_string(reader, &sdf_path, "axis"))
            .unwrap_or_else(|| "Z".to_string());
            let axis_rot = match axis.as_str() {
                "X" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::X)),
                "Z" => Some(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
                _ => None,
            };
            if let Some(q) = axis_rot {
                transform.rotation = transform.rotation * q;
            }
            info!("[usd-bevy] {} cylinder axis={} rot={:?}", sdf_path.as_str(), axis, transform.rotation);
        }
        // `xformOp:scale` (UsdGeomXformable) — non-uniform scaling
        // composed with translate + rotate. Spec-compliant `Cube`
        // prims rely on this to express width/height/depth without
        // the legacy `width`/`height`/`depth` attributes.
        if let Some(v) = get_attribute_as_vec3(reader, &sdf_path, "xformOp:scale") {
            let nonzero = v.x.abs() > 1e-6 || v.y.abs() > 1e-6 || v.z.abs() > 1e-6;
            if nonzero {
                transform.scale = v;
            }
        }

        // Honour `token visibility = "invisible"` and the
        // `lunco:placeholder = true` author flag — both apply as
        // `Visibility::Hidden`. Children inherit unless they
        // override their own visibility (Placeholder Cubes have no
        // children, so propagation is a no-op).
        let final_vis = if invisible || is_placeholder {
            Visibility::Hidden
        } else {
            existing_vis.cloned().unwrap_or(Visibility::Inherited)
        };

        commands.entity(entity).insert((
            transform,
            UsdVisualSynced,
            final_vis,
            InheritedVisibility::default(),
            ViewVisibility::default(),
        ));

        // Spawn children with their transforms pre-populated so physics sees them correctly.
        // This is critical for wheel positions — they must be at the correct offsets from
        // the chassis center before the suspension system runs.
        for child_path in reader.prim_children(&sdf_path) {
            if let Ok(val) = reader.get(&child_path, "active") {
                if let Value::Bool(active) = &*val {
                    if !*active { continue; }
                }
            }

            // Pre-read child transform from USD
            let mut child_tf = Transform::default();
            if let Some(v) = get_attribute_as_vec3(reader, &child_path, "xformOp:translate") {
                child_tf.translation = v;
            }
            if let Some(v) = get_attribute_as_vec3(reader, &child_path, "xformOp:rotateXYZ") {
                let rx = v.x.to_radians();
                let ry = v.y.to_radians();
                let rz = v.z.to_radians();
                child_tf.rotation = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);
            }

            let base_components = (
                Name::new(child_path.to_string()),
                UsdPrimPath {
                    stage_handle: prim_path.stage_handle.clone(),
                    path: child_path.to_string(),
                },
                child_tf,
                GlobalTransform::default(),
                Visibility::Visible,
                InheritedVisibility::VISIBLE,
                ViewVisibility::default(),
            );

            // Top-level USD prims (children of a scene root tagged with
            // `LoadIntoGrid`) become Grid-direct anchors so big_space's
            // `propagate_high_precision` updates their GlobalTransform.
            // Anything deeper stays as plain `Transform` children of
            // their USD parent's Bevy entity.
            // ChildOf must be set atomically with UsdPrimPath so that
            // observers triggered by the spawn (on_usd_prim_added →
            // instantiate_usd_prim → UsdVisualSynced → process_usd_avian_prims)
            // see the established parentage. Setting ChildOf later via
            // add_child queues a separate command applied AFTER the
            // observer cascade, causing `q_child_of.get(entity).is_err()`
            // to take the root-collider branch and silently mark
            // collider-child prims (e.g. Chassis) as RigidBody::Static.
            // Bevy's relationship system fans the reverse `Children`
            // edge from ChildOf automatically.
            if let Some(LoadIntoGrid(grid)) = load_into_grid {
                commands.spawn((
                    base_components,
                    CellCoord::default(),
                    lunco_core::GridAnchor,
                    ChildOf(*grid),
                ));
            } else {
                commands.spawn((base_components, ChildOf(entity)));
            }
        }
    }
}

/// Observer: fires the moment a new `UsdPrimPath` is added to an entity.
/// If the referenced `UsdStageAsset` is already loaded, the prim is
/// instantiated immediately. Otherwise the entity is tagged
/// `UsdAwaitingStage` and waits for `sync_usd_visuals` to drain it once
/// the asset becomes ready.
///
/// This is the **happy path** in steady state — once a scene is loaded,
/// any newly-spawned `UsdPrimPath` entity (API command, attach
/// operation, recursive child spawn) is processed in the same frame
/// without per-frame polling.
fn on_usd_prim_added(
    trigger: On<Add, UsdPrimPath>,
    q: Query<
        (&UsdPrimPath, Option<&Visibility>, Option<&Transform>, Option<&LoadIntoGrid>),
        Without<UsdVisualSynced>,
    >,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let entity = trigger.entity;
    let Ok((prim_path, vis, tf, load_into)) = q.get(entity) else { return; };

    if stages.get(&prim_path.stage_handle).is_none() {
        commands.entity(entity).insert(UsdAwaitingStage);
        return;
    }

    instantiate_usd_prim(
        entity,
        prim_path,
        vis,
        tf,
        load_into,
        &mut commands,
        &stages,
        &asset_server,
        &mut meshes,
        &mut materials,
    );
}

/// Drains the `UsdAwaitingStage` queue when a stage finishes loading.
/// Each entity whose `UsdPrimPath.stage_handle` matches the newly-loaded
/// asset gets processed exactly once.
///
/// Registered with `run_if(on_message::<AssetEvent<UsdStageAsset>>())`
/// so the system body executes only on frames where an asset event
/// actually fires — zero per-frame cost in steady state.
///
/// **Name retained for compatibility**: downstream systems
/// (`lunco-materials`, `lunco-usd-sim`, `lunco-usd-avian`) order
/// themselves with `.after(sync_usd_visuals)` to ensure they see
/// USD-spawned components. The deferred-stage path now goes through
/// this system; the eager path goes through the `on_usd_prim_added`
/// observer (which fires synchronously during command application, so
/// downstream `.after()` ordering covers it too).
pub fn sync_usd_visuals(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    q: Query<
        (Entity, &UsdPrimPath, Option<&Visibility>, Option<&Transform>, Option<&LoadIntoGrid>),
        (With<UsdAwaitingStage>, Without<UsdVisualSynced>),
    >,
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    use bevy::asset::AssetId;
    let mut loaded: Vec<AssetId<UsdStageAsset>> = Vec::new();
    for event in ev.read() {
        if let AssetEvent::LoadedWithDependencies { id } = event {
            loaded.push(*id);
        }
    }
    if loaded.is_empty() { return; }

    for (entity, prim_path, vis, tf, load_into) in q.iter() {
        if loaded.iter().any(|id| prim_path.stage_handle.id() == *id) {
            commands.entity(entity).remove::<UsdAwaitingStage>();
            instantiate_usd_prim(
                entity,
                prim_path,
                vis,
                tf,
                load_into,
                &mut commands,
                &stages,
                &asset_server,
                &mut meshes,
                &mut materials,
            );
        }
    }
}

/// Applies a standard PBR material to an entity, using USD prim attributes.
fn apply_standard_material(
    reader: &TextReader,
    sdf_path: &SdfPath,
    mesh_handle: &Handle<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    entity_cmd: &mut EntityCommands,
) {
    // Get color from primvars:displayColor attribute
    let color = get_attribute_as_vec3(reader, sdf_path, "primvars:displayColor")
        .map(|v| Color::srgb(v.x, v.y, v.z))
        .unwrap_or(Color::WHITE);

    entity_cmd.insert((
        Mesh3d(mesh_handle.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: color,
            ..default()
        }))
    ));
}

/// Reads a 3-component vector attribute from a USD prim.
///
/// Handles all common USD vector types:
/// - `color3f` → `Value::Vec3f`
/// - `double3` → `Value::Vec3d`
/// - `float3` → `Value::Vec3f`
/// - `Vec<f32>` / `Vec<f64>` array forms
///
/// Returns `None` if the attribute doesn't exist or can't be converted.
/// Reads a string-typed attribute from a USD prim.
///
/// Accepts every reasonable string-shaped USD value:
/// - `Value::String` — authored as `string foo = "..."`.
/// - `Value::Token` — authored as `token foo = "..."` (also the
///   parser's choice for several `lunco:*` attributes).
/// - `Value::AssetPath` — authored as `asset foo = @...@`. This
///   is what the composer emits for the synthesised
///   `lunco:resolvedAsset` so user-facing attributes carry the
///   correct USD type.
///
/// `prim_attribute_value::<String>` covers `String`/`Token` only,
/// so we go through `reader.get` for the attribute path directly
/// to also catch `AssetPath`.
fn get_attribute_as_string(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<String> {
    let attr_path = path.append_property(attr).ok()?;
    let val = reader.get(&attr_path, "default").ok()?;
    match &*val {
        Value::String(s) | Value::Token(s) | Value::AssetPath(s) => Some(s.clone()),
        _ => None,
    }
}

fn get_attribute_as_vec3(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<Vec3> {
    // Handle fixed-size array types first (Vec3f, Vec3d)
    if let Some(v) = reader.prim_attribute_value::<[f32; 3]>(path, attr) {
        return Some(Vec3::new(v[0], v[1], v[2]));
    }
    if let Some(v) = reader.prim_attribute_value::<[f64; 3]>(path, attr) {
        return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32));
    }
    // Handle Vec forms as fallback
    if let Some(v) = reader.prim_attribute_value::<Vec<f32>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0], v[1], v[2])); }
    }
    if let Some(v) = reader.prim_attribute_value::<Vec<f64>>(path, attr) {
        if v.len() >= 3 { return Some(Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32)); }
    }
    None
}

/// Marker inserted on prim entities that own both a primitive Cube
/// fallback mesh **and** a glTF [`SceneRoot`]. Used by
/// [`hide_glb_placeholder_meshes`] to find these entities cheaply.
#[derive(Component)]
pub struct GlbPlaceholder;

/// Removes the primitive Cube/Sphere/Cylinder fallback mesh once its
/// sibling [`SceneRoot`] reports its glTF [`Scene`] asset fully loaded.
///
/// **Pattern**: a USD prim authored as `def Cube "Foo" (payload =
/// @lunco-lib://...@)` carries two visuals — a placeholder Cuboid
/// (always built) and a `SceneRoot` for the glTF (set when the
/// composer synthesises `lunco:resolvedAsset`). Rendering both during
/// the async load gives a smooth "placeholder → photoreal" transition.
/// Once the Scene asset is `LoadedWithDependencies`, we drop the
/// `Mesh3d` + material so only the glTF remains.
///
/// **Why remove rather than hide**: setting `Visibility::Hidden` on
/// the parent entity propagates to descendants — including the
/// SceneRoot's spawned children — and would hide the glTF too.
/// Removing only the `Mesh3d` / `MeshMaterial3d` components leaves
/// the parent's transform and SceneRoot intact.
///
/// On asset failure (file missing, network error) Bevy never emits
/// `LoadedWithDependencies`, so the placeholder stays — the
/// no-glTF fallback case the pattern was designed for.
fn hide_glb_placeholder_meshes(
    mut commands: Commands,
    mut events: MessageReader<AssetEvent<Scene>>,
    scene_roots: Query<(Entity, &SceneRoot, Option<&ChildOf>), With<GlbPlaceholder>>,
    children: Query<&Children>,
    has_mesh: Query<(), With<Mesh3d>>,
) {
    // Bevy 0.18 renamed events to messages — `MessageReader` reads
    // `AssetEvent`s the same way `EventReader` did in earlier
    // versions.
    //
    // The placeholder pattern lays out two distinct USD authorings,
    // both supported here:
    //   1. **Same-entity** — `def Cube` with a `lunco-lib://` payload.
    //      Mesh3d (Cuboid) and SceneRoot live on the same entity.
    //   2. **Sibling** — `def Xform { def Cube "Placeholder"; def Xform
    //      "Visual" (payload = ...); }`. Recommended for prims whose
    //      parent scale must not propagate to the glTF children. Mesh3d
    //      is on a sibling under the same parent Xform.
    //
    // Both shapes are handled: drop Mesh3d on the SceneRoot entity
    // itself (no-op if absent) **and** on any sibling that has one.
    for ev in events.read() {
        for (e, root, parent) in scene_roots.iter() {
            if !ev.is_loaded_with_dependencies(root.0.id()) { continue; }

            // Same-entity placeholder
            commands.entity(e)
                .remove::<Mesh3d>()
                .remove::<MeshMaterial3d<StandardMaterial>>();

            // Sibling placeholder. `Children::iter()` yields `Entity`
            // by value via the relationship-target trait — not `&Entity`.
            if let Some(parent) = parent {
                if let Ok(siblings) = children.get(parent.0) {
                    for sib in siblings.iter() {
                        if sib != e && has_mesh.get(sib).is_ok() {
                            commands.entity(sib)
                                .remove::<Mesh3d>()
                                .remove::<MeshMaterial3d<StandardMaterial>>();
                        }
                    }
                }
            }
        }
    }
}

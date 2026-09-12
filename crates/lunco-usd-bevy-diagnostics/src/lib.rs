//! USD visual asset failure and placeholder diagnostics.
//!
//! This production package owns the optional visual placeholder shown when an
//! external glTF asset fails. Render-free stage-load failure state belongs to
//! the USD scene lifecycle package.
//! It is installed by the aggregate USD plugin, but the visual USD projector
//! does not depend on this package: ordinary projection changes therefore do
//! not rebuild diagnostic code.

use bevy::prelude::*;
use lunco_render::{PbrLook, PbrTextures, SurfaceAlpha};
use lunco_usd_bevy_core::{
    canonical::CanonicalStages,
    read::{get_attribute_as_vec3, UsdRead},
    UsdStageAsset,
};
use lunco_usd_bevy_scene::{GlbPlaceholder, PlaceholderAssetUri, UsdPrimPath};
use openusd::sdf::Path as SdfPath;

/// Installs optional visual placeholder diagnostics.
pub struct UsdDiagnosticsPlugin;

impl Plugin for UsdDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        lunco_settings::ensure_download_settings(app);
        app.init_resource::<DiagnosticLabelFont>()
            .init_resource::<DiagnosticLabelConfig>()
            .add_systems(Startup, load_diagnostic_label_font)
            .add_systems(
                Update,
                (
                    hide_glb_placeholder_meshes,
                    poll_diagnostic_label_font,
                    reveal_placeholder_on_failure,
                    bake_pending_labels,
                ),
            );
    }
}

/// Marker for entities spawned as diagnostic stubs when asset loading fails.
#[derive(Component)]
pub struct DiagnosticStub;

/// Marker for the textured quad that displays the failed asset's filename.
#[derive(Component)]
pub struct DiagnosticStubLabel;

/// Attached to a freshly-spawned [`DiagnosticStub`] that still needs its
/// filename baked onto its faces. A separate pass ([`bake_pending_labels`])
/// does the baking once [`DiagnosticLabelFont`] is available — this decouples
/// *when the asset fails* from *when the font is ready*, which matters on web
/// where the font arrives asynchronously over HTTP.
#[derive(Component)]
pub struct PendingDiagnosticLabel {
    /// Full label text (prefix + file name).
    pub text: String,
    /// World size of the diagnostic box, for fitting the label per face.
    pub box_size: Vec3,
}

/// Tunable appearance of the failed-asset diagnostic stub. Insert your own
/// before [`UsdDiagnosticsPlugin`] builds (or mutate the resource at runtime) to
/// override any field — nothing here is a hard-coded magic constant.
#[derive(Resource, Clone, Debug)]
pub struct DiagnosticLabelConfig {
    /// Glyph height used when rasterising the label, in texture pixels
    /// (higher = crisper text, larger texture).
    pub font_px: f32,
    /// Transparent border around the text, in texture pixels.
    pub padding_px: f32,
    /// Text colour, RGB 0-255.
    pub text_color: [u8; 3],
    /// Backdrop colour painted behind the text, RGBA 0-255.
    pub bg_color: [u8; 4],
    /// Fraction (0..1) of each box face the label may cover.
    pub face_coverage: f32,
    /// Colour of the semi-transparent diagnostic box itself.
    pub box_color: Color,
    /// String prepended to the file name (e.g. `"Missing: "`).
    pub prefix: String,
    /// `true` → label on all six faces; `false` → only the +Z front face.
    pub all_faces: bool,
    /// Seconds a placeholder may wait for its glTF scene before the stub is
    /// shown. Covers web, where a 404 may never report a clean `is_failed()`.
    pub grace_secs: f32,
}

impl Default for DiagnosticLabelConfig {
    fn default() -> Self {
        Self {
            font_px: 64.0,
            padding_px: 24.0,
            text_color: [255, 255, 255],
            bg_color: [20, 0, 0, 140],
            face_coverage: 0.85,
            box_color: Color::srgba(1.0, 0.0, 0.0, 0.7),
            prefix: "Missing: ".to_string(),
            all_faces: true,
            grace_secs: 8.0,
        }
    }
}

/// Caches the DejaVu Sans face used to bake filename labels into textures, so
/// the `.ttf` is loaded at most once (not per failed asset). `None` until the
/// font is loaded (native: read from storage at startup; web: fetched over
/// HTTP). If it never loads, stubs still show the red box, just without text.
#[derive(Resource, Default)]
pub struct DiagnosticLabelFont(pub Option<std::sync::Arc<ab_glyph::FontVec>>);

/// Holds the receiver from [`lunco_assets::font::load_dejavu_sans_bytes`]
/// until the bytes land. The same channel mechanism works on native (bytes
/// ready immediately) and web (bytes fetched async), so the plugin has no
/// platform branches. Removed once the font installs.
#[derive(Resource)]
struct DiagnosticFontLoad(std::sync::Mutex<std::sync::mpsc::Receiver<Vec<u8>>>);

/// Parses raw `.ttf` bytes into [`DiagnosticLabelFont`].
fn install_diagnostic_font(font: &mut DiagnosticLabelFont, bytes: Vec<u8>) {
    match ab_glyph::FontVec::try_from_vec(bytes) {
        Ok(f) => font.0 = Some(std::sync::Arc::new(f)),
        Err(e) => warn!("[usd-bevy-diagnostics] diagnostic label font parse failed: {e}"),
    }
}

/// Startup: kick off the DejaVu Sans load via `lunco-assets` (which owns the
/// native-read / web-fetch procedure) and stash the receiver for
/// [`poll_diagnostic_label_font`] to drain.
fn load_diagnostic_label_font(
    mut commands: Commands,
    settings: Res<lunco_settings::DownloadSettings>,
) {
    let rx = lunco_assets::font::load_dejavu_sans_bytes(&settings);
    commands.insert_resource(DiagnosticFontLoad(std::sync::Mutex::new(rx)));
}

/// Drains the font-load channel and installs the face once the bytes arrive
/// (frame 1 on native, whenever the fetch lands on web). Uniform across
/// platforms; removes the loader resource when done.
fn poll_diagnostic_label_font(
    load: Option<Res<DiagnosticFontLoad>>,
    mut font: ResMut<DiagnosticLabelFont>,
    mut commands: Commands,
) {
    if font.0.is_some() {
        return;
    }
    let Some(load) = load else { return };
    let received = load.0.lock().ok().and_then(|rx| rx.try_recv().ok());
    if let Some(bytes) = received {
        info!(
            "[usd-bevy-diagnostics] diagnostic label font loaded ({} bytes)",
            bytes.len()
        );
        install_diagnostic_font(&mut font, bytes);
        commands.remove_resource::<DiagnosticFontLoad>();
    }
}

/// CPU-rasterises `text` into an RGBA [`Image`] per [`DiagnosticLabelConfig`]:
/// coloured glyphs on a configurable backdrop. Baked once per failed asset —
/// no camera, no render pass, no per-frame work. `None` if `text` is empty.
fn rasterize_label(
    text: &str,
    font: &ab_glyph::FontVec,
    cfg: &DiagnosticLabelConfig,
) -> Option<Image> {
    use ab_glyph::{point, Font, PxScale, ScaleFont};
    // The POD texture descriptors, straight from `wgpu-types` — the same types
    // `bevy_image` itself takes. NOT `bevy::render::render_resource`, which is a
    // `bevy_render` re-export and would drag wgpu + naga into this crate.
    use bevy::asset::RenderAssetUsages;
    use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

    if text.is_empty() {
        return None;
    }
    let px = cfg.font_px.max(1.0);
    let pad = cfg.padding_px.max(0.0);
    let scaled = font.as_scaled(PxScale::from(px));

    // Measure advance width (with kerning) for the whole string.
    let mut width = 0.0_f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let gid = font.glyph_id(c);
        if let Some(p) = prev {
            width += scaled.kern(p, gid);
        }
        width += scaled.h_advance(gid);
        prev = Some(gid);
    }
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let img_w = (width + pad * 2.0).ceil().max(1.0) as usize;
    let img_h = (ascent - descent + pad * 2.0).ceil().max(1.0) as usize;

    // Configurable backdrop so the text reads over the box behind the quad.
    let mut buf = vec![0u8; img_w * img_h * 4];
    for px4 in buf.chunks_mut(4) {
        px4.copy_from_slice(&cfg.bg_color);
    }

    // Draw each glyph in the configured text colour, coverage-blended.
    let [tr, tg, tb] = cfg.text_color;
    let tc = [tr as u16, tg as u16, tb as u16];
    let mut caret = point(pad, pad + ascent);
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let gid = font.glyph_id(c);
        if let Some(p) = prev {
            caret.x += scaled.kern(p, gid);
        }
        let glyph = gid.with_scale_and_position(PxScale::from(px), caret);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let x = bb.min.x as i32 + gx as i32;
                let y = bb.min.y as i32 + gy as i32;
                if x < 0 || y < 0 || x as usize >= img_w || y as usize >= img_h {
                    return;
                }
                let idx = (y as usize * img_w + x as usize) * 4;
                let a = (cov * 255.0) as u16;
                for k in 0..3 {
                    let bg = buf[idx + k] as u16;
                    buf[idx + k] = ((tc[k] * a + bg * (255 - a)) / 255) as u8;
                }
                buf[idx + 3] = buf[idx + 3].max((cov * 255.0) as u8);
            });
        }
        caret.x += scaled.h_advance(gid);
        prev = Some(gid);
    }

    Some(Image::new(
        Extent3d {
            width: img_w as u32,
            height: img_h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        buf,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    ))
}

/// Bakes the filename texture onto every (or just the front) face of each
/// pending diagnostic stub, once the label font is available. Runs each frame
/// but only touches stubs that still carry [`PendingDiagnosticLabel`].
fn bake_pending_labels(
    mut commands: Commands,
    cfg: Res<DiagnosticLabelConfig>,
    font: Res<DiagnosticLabelFont>,
    pending: Query<(Entity, &PendingDiagnosticLabel)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(font) = font.0.as_ref() else { return };
    for (stub, pending) in pending.iter() {
        let Some(image) = rasterize_label(&pending.text, font, &cfg) else {
            commands.entity(stub).remove::<PendingDiagnosticLabel>();
            continue;
        };
        let aspect = (image.width() as f32 / image.height().max(1) as f32).max(0.01);
        let tex = images.add(image);
        // One look shared across all faces — the binder's content-keyed cache
        // gives every face the same material handle, as the hand-shared
        // `label_mat` did. `double_sided` == the old `cull_mode: None`
        // (readable from either side).
        let label_look = PbrLook {
            // WHITE, explicitly: `PbrLook::default()`'s base colour is mid-grey,
            // which would tint the baked glyphs 50% dark. `StandardMaterial`'s
            // default (what this used to build) is white.
            base_color: LinearRgba::WHITE,
            textures: PbrTextures {
                base_color: Some(tex),
                ..default()
            },
            alpha: SurfaceAlpha::Blend,
            unlit: true,
            double_sided: true,
            ..default()
        };
        let s = pending.box_size;
        let (hx, hy, hz) = (s.x / 2.0, s.y / 2.0, s.z / 2.0);
        let eps = 0.01;
        use std::f32::consts::{FRAC_PI_2, PI};
        // Each face: outward offset + a rotation that turns the default
        // +Z-facing `Rectangle` to face outward, plus the face's
        // (horizontal, vertical) extent for sizing.
        let faces: &[(Vec3, Quat, f32, f32)] = if cfg.all_faces {
            &[
                (Vec3::new(0.0, 0.0, hz + eps), Quat::IDENTITY, s.x, s.y), // +Z
                (
                    Vec3::new(0.0, 0.0, -hz - eps),
                    Quat::from_rotation_y(PI),
                    s.x,
                    s.y,
                ), // -Z
                (
                    Vec3::new(hx + eps, 0.0, 0.0),
                    Quat::from_rotation_y(FRAC_PI_2),
                    s.z,
                    s.y,
                ), // +X
                (
                    Vec3::new(-hx - eps, 0.0, 0.0),
                    Quat::from_rotation_y(-FRAC_PI_2),
                    s.z,
                    s.y,
                ), // -X
                (
                    Vec3::new(0.0, hy + eps, 0.0),
                    Quat::from_rotation_x(-FRAC_PI_2),
                    s.x,
                    s.z,
                ), // +Y
                (
                    Vec3::new(0.0, -hy - eps, 0.0),
                    Quat::from_rotation_x(FRAC_PI_2),
                    s.x,
                    s.z,
                ), // -Y
            ]
        } else {
            &[(Vec3::new(0.0, 0.0, hz + eps), Quat::IDENTITY, s.x, s.y)]
        };
        let cover = cfg.face_coverage.clamp(0.05, 1.0);
        commands.entity(stub).with_children(|p| {
            for &(offset, rot, fw, fh) in faces {
                // Fit the label inside the face, keeping the texture aspect.
                let mut qw = (fw * cover).max(0.1);
                let mut qh = qw / aspect;
                if qh > fh * cover {
                    qh = (fh * cover).max(0.05);
                    qw = qh * aspect;
                }
                p.spawn((
                    Name::new("DiagnosticStubLabel"),
                    DiagnosticStubLabel,
                    Mesh3d(meshes.add(Rectangle::new(qw, qh))),
                    label_look.clone(),
                    Transform::from_translation(offset).with_rotation(rot),
                ));
            }
        });
        commands.entity(stub).remove::<PendingDiagnosticLabel>();
    }
}

/// Removes the primitive Cube/Sphere/Cylinder fallback mesh once its
/// sibling [`WorldAssetRoot`] reports its glTF [`WorldAsset`] asset fully loaded.
fn hide_glb_placeholder_meshes(
    mut commands: Commands,
    // `Option<...>` so the system no-ops (instead of panicking on param
    // validation) in minimal apps that never `init_asset::<WorldAsset>()` — e.g.
    // headless tests that add `UsdDiagnosticsPlugin` without the full scene pipeline.
    // Production always registers `WorldAsset`, so behaviour there is unchanged.
    events: Option<MessageReader<AssetEvent<WorldAsset>>>,
    scene_roots: Query<(Entity, &WorldAssetRoot, Option<&ChildOf>), With<GlbPlaceholder>>,
    children: Query<&Children>,
    has_mesh: Query<(), With<Mesh3d>>,
    mut visibility: Query<&mut Visibility>,
) {
    let Some(mut events) = events else { return };
    for ev in events.read() {
        if let AssetEvent::LoadedWithDependencies { id } = ev {
            for (e, root, parent) in scene_roots.iter() {
                if root.0.id() == *id {
                    if let Ok(mut vis) = visibility.get_mut(e) {
                        *vis = Visibility::Inherited;
                    }
                    // Dropping `Mesh3d` is what stops the placeholder drawing;
                    // dropping `PbrLook` retires its appearance intent (the binder
                    // owns the `MeshMaterial3d`, which is inert with no mesh).
                    commands
                        .entity(e)
                        .remove::<Mesh3d>()
                        .remove::<PbrLook>()
                        .remove::<GlbPlaceholder>()
                        .remove::<PlaceholderAssetUri>();

                    if let Some(parent) = parent {
                        if let Ok(siblings) = children.get(parent.0) {
                            for sib in siblings.iter() {
                                if sib != e && has_mesh.get(sib).is_ok() {
                                    commands.entity(sib).remove::<Mesh3d>().remove::<PbrLook>();
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Reveals a red, semi-transparent diagnostic box when a [`GlbPlaceholder`]'s
/// glTF scene fails to load or never loads within
/// [`DiagnosticLabelConfig::grace_secs`] (the web case, where a 404 may not
/// surface a clean `is_failed()`). The filename label is baked on separately by
/// [`bake_pending_labels`] once the font is ready, via [`PendingDiagnosticLabel`].
pub fn reveal_placeholder_on_failure(
    mut commands: Commands,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<CanonicalStages>,
    cfg: Res<DiagnosticLabelConfig>,
    scene_roots: Query<
        (
            Entity,
            &WorldAssetRoot,
            &GlobalTransform,
            &PlaceholderAssetUri,
            &UsdPrimPath,
        ),
        (With<GlbPlaceholder>, Without<DiagnosticStub>),
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    // Per-placeholder time spent waiting on its glTF scene. Used to trip the
    // grace timeout on web, where a broken load may never report `is_failed()`.
    mut waited: Local<std::collections::HashMap<Entity, f32>>,
) {
    for (e, root, _global_transform, uri, prim_path) in scene_roots.iter() {
        let state = asset_server.load_state(root.0.id());
        // The asset arrived — stop tracking; `hide_glb_placeholder_meshes`
        // drops the marker on the next `LoadedWithDependencies` event.
        if state.is_loaded() {
            waited.remove(&e);
            continue;
        }
        let elapsed = waited.entry(e).or_insert(0.0);
        *elapsed += time.delta_secs();
        let timed_out = *elapsed >= cfg.grace_secs;
        if state.is_failed() || timed_out {
            waited.remove(&e);
            info!(
                "[usd-bevy-diagnostics] asset {} for {:?} ({}), spawning diagnostic stub",
                if timed_out {
                    "did not load in time"
                } else {
                    "load FAILED"
                },
                root.0.id(),
                uri.0,
            );

            // Default scale
            let mut scale = Vec3::ONE;

            // Attempt to resolve dimensions from USD prim attributes
            if let Some(stage_asset) = stages.get(&prim_path.stage_handle) {
                let (reader, _generation) =
                    canonical.reader_for(prim_path.stage_handle.id(), stage_asset);

                // Navigate up from the current prim to its parent to find the sibling "Placeholder"
                let parent_path = prim_path.path.rsplit_once('/').map(|x| x.0).unwrap_or("");
                let sibling_placeholder_path = format!("{}/Placeholder", parent_path);

                // Helper to check attributes
                let check_path = |path: &str| -> Option<Vec3> {
                    if let Ok(sdf_path) = SdfPath::new(path) {
                        get_attribute_as_vec3(&reader, &sdf_path, "xformOp:scale").or_else(|| {
                            UsdRead::real(&reader, &sdf_path, "size")
                                .map(|size| Vec3::splat(size as f32))
                        })
                    } else {
                        None
                    }
                };

                // Check sibling first, then parent prim path itself
                if let Some(s) =
                    check_path(&sibling_placeholder_path).or_else(|| check_path(&prim_path.path))
                {
                    debug!("[usd-bevy-diagnostics] Found scale: {:?}", s);
                    scale = s;
                } else {
                    debug!(
                        "[usd-bevy-diagnostics] No scale or size found on paths: {:?} or {:?}",
                        sibling_placeholder_path, prim_path.path
                    );
                }
            }

            debug!("[usd-bevy-diagnostics] Computed stub scale: {:?}", scale);

            // Just the filename — strip the `lunco://…/` path prefix and
            // the `#Scene0` glTF sub-label.
            let file_name = uri
                .0
                .rsplit('/')
                .next()
                .unwrap_or(&uri.0)
                .split('#')
                .next()
                .unwrap_or(&uri.0);

            commands.entity(e).try_insert((
                Mesh3d(meshes.add(Cuboid::from_size(scale))),
                PbrLook {
                    base_color: cfg.box_color.to_linear(),
                    emissive: LinearRgba::from(cfg.box_color),
                    alpha: SurfaceAlpha::Blend, // Support transparency
                    unlit: true,                // readable even with no scene lighting
                    ..default()
                },
                // No `Transform` / `Visibility` insert here. `Mesh3d` pulls both in as
                // required components, and re-inserting a `Transform` built from
                // `GlobalTransform::compute_transform()` would overwrite the prim's LOCAL
                // transform with a world-space one — wrong for any entity with a parent.
                DiagnosticStub,
                // The label is baked on once the font is ready (frame 1 on
                // native, whenever the fetch lands on web).
                PendingDiagnosticLabel {
                    text: format!("{}{file_name}", cfg.prefix),
                    box_size: scale,
                },
            ));
        }
    }
}

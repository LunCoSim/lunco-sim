//! `UsdLuxDomeLight` with a texture → image-based lighting + skybox.
//!
//! # Why `DomeLight` and not a `lunco:` prim
//!
//! HDRI environment lighting already has a USD schema, and this is it. Every
//! DCC (Houdini, Maya, Blender, omniverse) writes a `DomeLight` when you set an
//! environment image, so a scene authored anywhere lights correctly here with
//! no import step. The attributes below are stock `UsdLuxDomeLight` /
//! `UsdLuxLightAPI` — we invent nothing:
//!
//! ```usda
//! def DomeLight "Sky" (prepend apiSchemas = ["ShapingAPI"]) {
//!     asset  inputs:texture:file   = @../hdri/lunar_horizon_2k.hdr@
//!     token  inputs:texture:format = "latlong"
//!     float  inputs:intensity      = 1.0
//!     float  inputs:exposure       = 0.0
//!     color3f inputs:color         = (1, 1, 1)
//!     float3 xformOp:rotateXYZ     = (0, 90, 0)   # spin the environment
//!     uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
//! }
//! ```
//!
//! Two knobs have no UsdLux equivalent and are namespaced `lunco:`:
//! `lunco:dome:skybox` (draw it, or light-only — a lunar scene wants a black
//! sky but may still want bounce light) and `lunco:dome:faceSize` (cubemap
//! resolution).
//!
//! # The one hard part: latlong is not a cubemap
//!
//! Bevy's [`Skybox`] and [`GeneratedEnvironmentMapLight`] both demand a **cubemap**
//! (6 array layers, `TextureViewDimension::Cube`, power-of-two). Every HDRI you
//! can actually download — and everything USD calls `latlong` — is a 2:1
//! *equirectangular* 2D image. So [`equirect_to_cubemap`] projects one into the
//! other on the CPU, on the async pool, once per texture.
//!
//! What we deliberately do NOT do is bake diffuse-irradiance and
//! specular-radiance maps offline (the classic IBL pipeline, and what a bare
//! [`EnvironmentMapLight`] would require). [`GeneratedEnvironmentMapLight`] takes
//! a single source cubemap and does that prefiltering on the GPU at runtime, so
//! an author drops a `.hdr` next to their `.usda` and it lights — no tooling
//! step, no `.ktx2` conversion, nothing to check in.
//!
//! # Ambient interplay
//!
//! A textureless `DomeLight` keeps its historical meaning: a scalar
//! `GlobalAmbientLight` contribution (`UsdDomeAmbient`, see `light.rs`). A
//! *textured* dome contributes **no** flat ambient — the IBL it provides is a
//! strictly better version of the same thing, and summing both would
//! double-count the sky.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::light::{GeneratedEnvironmentMapLight, Skybox};
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use wgpu_types::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};

/// Default cubemap face resolution. Native gets 1024 (a visibly sharp skybox);
/// wasm gets 512 because `AsyncComputeTaskPool` runs on the main thread there,
/// so the projection cost is a frame hitch rather than background work.
#[cfg(not(target_arch = "wasm32"))]
pub const DEFAULT_FACE_SIZE: u32 = 1024;
#[cfg(target_arch = "wasm32")]
pub const DEFAULT_FACE_SIZE: u32 = 512;

/// Intensity for a dome that authors none, in **cd/m²**.
///
/// UsdLux's spec default is `1.0`, and we deliberately ignore it — for exactly
/// the reason `DistantLight` above ignores the same spec default of 1 lx. This
/// app runs its cameras at a physically-calibrated EV100 ≈ 15 (a 128 klx sun),
/// where 1 cd/m² is *indistinguishable from black*. A dome authored with a
/// texture and no intensity plainly means "light my scene with this sky", and
/// honouring the spec there yields a black screen and a bug report.
///
/// 1000 cd/m² lands a typical HDRI sky in the middle of that exposure. Authors
/// tune from there: a few hundred for a dim/overcast sky, a few thousand for a
/// bright one. Above ~10 000 the sky clips to white at this exposure.
const DEFAULT_DOME_INTENSITY: f32 = 1000.0;

/// `inputs:texture:format` — how to interpret the dome's image.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DomeFormat {
    /// Equirectangular (2:1 lat/long). USD's `latlong`, and what `automatic`
    /// resolves to for any non-cube image. The only format HDRI libraries ship.
    #[default]
    LatLong,
    /// The source asset is already a cubemap (a `.ktx2` with 6 array layers).
    /// USD has no token for this — it is what `automatic` means when the image
    /// turns out to have 6 layers. Used as-is, no projection.
    Cube,
}

/// A `DomeLight` that authored `inputs:texture:file`: the render-free *intent*,
/// stamped on the dome prim's entity by `light.rs`. The texture is still
/// loading at this point; [`project_dome_textures`] turns it into a
/// [`DomeCubemap`] once it lands.
#[derive(Component, Clone)]
pub struct UsdDomeEnvironment {
    /// The authored HDRI, loading. `MAIN_WORLD`-only — it is CPU input to the
    /// cubemap projection and is never itself rendered.
    pub texture: Handle<Image>,
    pub format: DomeFormat,
    /// `inputs:intensity` × 2^`inputs:exposure`.
    pub intensity: f32,
    /// `inputs:color`, multiplied into the image.
    pub tint: LinearRgba,
    /// `lunco:dome:faceSize`.
    pub face_size: u32,
    /// `lunco:dome:skybox` — false = light the scene but leave the sky black.
    pub skybox: bool,
}

/// The projected cubemap, ready to hand to a camera.
#[derive(Component)]
pub struct DomeCubemap(pub Handle<Image>);

/// In-flight `equirect → cubemap` projection. Held on the dome entity; the
/// `Without<DomeProjection>` guard on [`project_dome_textures`] is what makes
/// the work fire exactly once.
#[derive(Component)]
pub struct DomeProjection(Task<Image>);

/// The dome's authored image/tint/size changed and its [`DomeCubemap`] is now
/// stale. Set by [`refresh_dome_entity`] on a live edit.
///
/// The old cubemap is deliberately left in place until the new one is ready —
/// dropping it immediately would leave the scene with no dome for the frames the
/// projection takes, and the sky would flash black on every edit.
#[derive(Component)]
pub struct DomeDirty;

/// Stamped on a camera that carries a dome's [`Skybox`] /
/// [`GeneratedEnvironmentMapLight`], so [`bind_dome_to_cameras`] can tell a
/// camera it must strip (dome gone) from one it never touched.
#[derive(Component)]
pub struct DomeBoundCamera;

pub struct DomePlugin;

impl Plugin for DomePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (project_dome_textures, bind_dome_to_cameras).chain());
    }
}

/// Load a dome's HDRI.
///
/// Deliberately a **plain** `load` with no `with_settings`. A `.hdr` is decoded
/// by `HdrTextureLoader`, whose settings type is `()`, while `.png`/`.ktx2` go
/// through `ImageLoader` with `ImageLoaderSettings` — so pinning either settings
/// type here makes the asset server reject it for the other formats
/// ("Configured settings type … does not match AssetLoader settings type") and
/// silently fall back to defaults. The defaults are what we want anyway:
/// `RenderAssetUsages::default()` keeps the pixels in the main world, which is
/// exactly the CPU access [`Equirect::flatten`] needs.
fn load_dome_texture(asset_server: &AssetServer, path: &str) -> Handle<Image> {
    asset_server.load::<Image>(path.to_string())
}

/// Read a `DomeLight` prim's HDRI intent. `None` when the prim authors no
/// `inputs:texture:file` — that dome is a plain scalar ambient (`light.rs`).
///
/// The single definition of how a dome's attributes are read, shared by the
/// load path (`instantiate_light_prim`) and the live-edit path
/// (`lunco_usd::live_consume`). Two copies would drift, and the symptom would
/// be a dome that loads one way from disk and another way after an edit.
pub fn read_dome_environment<R: crate::read::UsdRead>(
    reader: &R,
    sdf_path: &openusd::sdf::Path,
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<crate::UsdStageAsset>,
) -> Option<UsdDomeEnvironment> {
    let texture_path = reader
        .asset(sdf_path, "inputs:texture:file")
        .filter(|p| !p.is_empty())
        .and_then(|p| crate::resolve_texture_path(asset_server, stage_id, &p))?;

    // `automatic` (USD's default) means "infer from the file". We infer from the
    // *decoded* image in `project_dome_textures` rather than the extension, so
    // this starts as LatLong and is corrected there if the image turns out to
    // carry 6 layers.
    match reader.text(sdf_path, "inputs:texture:format").as_deref() {
        Some("cubeMapVerticalCross") | Some("angular") | Some("mirroredBall") => {
            warn!(
                "[usd-bevy] {} DomeLight inputs:texture:format is unsupported (only \
                 `latlong`/`automatic`, plus a .ktx2 cubemap) — reading the image as \
                 equirectangular, which will look wrong",
                sdf_path.as_str(),
            );
        }
        _ => {}
    }

    Some(UsdDomeEnvironment {
        texture: load_dome_texture(asset_server, &texture_path),
        format: DomeFormat::LatLong,
        intensity: crate::light::read_intensity_with_exposure(
            reader,
            sdf_path,
            DEFAULT_DOME_INTENSITY,
        ),
        tint: crate::get_attribute_as_vec3(reader, sdf_path, "inputs:color")
            .map(|c| LinearRgba::rgb(c.x, c.y, c.z))
            .unwrap_or(LinearRgba::WHITE),
        face_size: crate::light::get_attribute_as_f32(reader, sdf_path, "lunco:dome:faceSize")
            .map(|f| f as u32)
            .unwrap_or(DEFAULT_FACE_SIZE),
        skybox: crate::light::get_attribute_as_bool(reader, sdf_path, "lunco:dome:skybox")
            .unwrap_or(true),
    })
}

impl UsdDomeEnvironment {
    /// Whether swapping to `next` invalidates the projected cubemap.
    ///
    /// The tint and face size are **baked into** the cubemap, so changing either
    /// means re-projecting. Intensity and the skybox flag are applied per-frame
    /// at the camera, so they are free — and that distinction is what keeps a
    /// brightness slider from re-running a 300 ms projection on every drag.
    pub fn needs_reprojection(&self, next: &Self) -> bool {
        self.texture != next.texture
            || self.face_size != next.face_size
            || self.tint != next.tint
    }
}

/// Kick off (and reap) the projection for every dome whose texture has landed.
fn project_dome_textures(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    pending: Query<
        (Entity, &UsdDomeEnvironment),
        (
            Without<DomeProjection>,
            Or<(Without<DomeCubemap>, With<DomeDirty>)>,
        ),
    >,
    mut running: Query<(Entity, &UsdDomeEnvironment, &mut DomeProjection)>,
) {
    for (entity, dome) in &pending {
        // `Assets::get` is `Some` only once the loader has run. A failed load
        // never becomes `Some`, so this simply stays pending — the asset
        // server has already logged the error, and a scene whose HDRI is
        // missing renders exactly as it would with no dome at all.
        let Some(source) = images.get(&dome.texture) else {
            continue;
        };

        // An already-cube source (a `.ktx2` env map) needs no projection: hand
        // it straight to the camera. Note this consults the *decoded* image
        // rather than trusting `inputs:texture:format`, which is how USD's
        // `automatic` is supposed to behave.
        if source.texture_descriptor.size.depth_or_array_layers == 6 {
            commands
                .entity(entity)
                .try_insert(DomeCubemap(dome.texture.clone()))
                .remove::<DomeDirty>();
            continue;
        }
        if dome.format == DomeFormat::Cube {
            warn!(
                "[usd-bevy] dome texture declares a cubemap format but decoded to \
                 {} layer(s) — projecting it as equirectangular instead",
                source.texture_descriptor.size.depth_or_array_layers,
            );
        }

        // Flatten to linear RGBA once. `get_color_at` dispatches on the texture
        // format every call, and the projection below samples each source texel
        // many times over — paying that dispatch 25M times instead of 2M is the
        // difference between a snappy load and a stall.
        let Some(equirect) = Equirect::flatten(source) else {
            warn!(
                "[usd-bevy] dome texture has an unreadable format ({:?}) — skipping",
                source.texture_descriptor.format,
            );
            commands
                .entity(entity)
                .try_insert(DomeCubemap(Handle::default()))
                .remove::<DomeDirty>();
            continue;
        };

        let face_size = dome.face_size;
        let tint = dome.tint;
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { equirect_to_cubemap(&equirect, face_size, tint) });
        commands
            .entity(entity)
            .try_insert(DomeProjection(task))
            .remove::<DomeDirty>();
    }

    for (entity, dome, mut proj) in &mut running {
        let Some(cube) = block_on(future::poll_once(&mut proj.0)) else {
            continue;
        };
        info!(
            "[usd-bevy] dome cubemap ready ({}² × 6, skybox={})",
            dome.face_size, dome.skybox,
        );
        commands
            .entity(entity)
            .remove::<DomeProjection>()
            .try_insert(DomeCubemap(images.add(cube)));
    }
}

/// Re-apply a `DomeLight` prim's authored state to its live entity after an
/// attribute-only edit (a `SetDomeLight`, or a hand edit to the `.usda`).
///
/// Called by `lunco_usd::live_consume` — the USD change sink is the only thing
/// that knows an attribute moved. `next` is the re-read intent (`None` = the
/// author removed `inputs:texture:file`, so the dome reverts to the scalar
/// ambient it means without one), and `ambient` is that fallback's
/// `inputs:intensity`.
pub fn refresh_dome_entity(
    world: &mut World,
    entity: Entity,
    next: Option<UsdDomeEnvironment>,
    ambient: f32,
) {
    use crate::light::{UsdAuthoredLight, UsdDomeAmbient};

    let Ok(mut e) = world.get_entity_mut(entity) else {
        return;
    };
    match next {
        Some(next) => {
            // Only re-project when the *baked* inputs moved. An intensity or
            // skybox toggle is applied at the camera each frame, so it must not
            // pay for a re-projection.
            let stale = e
                .get::<UsdDomeEnvironment>()
                .is_none_or(|cur| cur.needs_reprojection(&next));
            e.insert(next);
            e.remove::<UsdDomeAmbient>();
            if stale {
                e.insert(DomeDirty);
            }
        }
        None => {
            e.remove::<UsdDomeEnvironment>();
            e.remove::<DomeCubemap>();
            e.remove::<DomeProjection>();
            e.remove::<DomeDirty>();
            e.insert(UsdDomeAmbient(ambient));
        }
    }
    // `on_usd_light_added` recomputes `GlobalAmbientLight` from the authored
    // domes, and it observes `Add`. Re-stamping the marker is what re-runs that
    // sum — without it, a dome that just gained (or lost) its texture would
    // leave the old flat ambient standing and double-light the scene.
    e.remove::<UsdAuthoredLight>();
    e.insert(UsdAuthoredLight);
}

/// Push the scene's dome onto every 3D camera, and strip it from cameras when
/// the dome goes away.
///
/// `Skybox` and `EnvironmentMapLight` are *view* components in Bevy — they live
/// on the camera, not on the light entity — so a dome prim can't simply carry
/// them. Cameras also spawn late and often here (viewport switches, rover
/// mounts, the avatar's provisional camera), which is why this reconciles every
/// frame from current world state instead of firing once on `Add`.
fn bind_dome_to_cameras(
    mut commands: Commands,
    domes: Query<(&UsdDomeEnvironment, &DomeCubemap, Option<&GlobalTransform>)>,
    cameras: Query<Entity, With<Camera3d>>,
    bound: Query<Entity, (With<DomeBoundCamera>, With<Camera3d>)>,
) {
    // One sky. A second textured dome is a scene-authoring error, not a feature
    // to blend — say so once rather than silently letting iteration order pick.
    let mut iter = domes.iter();
    let Some((dome, cube, xform)) = iter.next() else {
        for camera in &bound {
            commands
                .entity(camera)
                .remove::<(Skybox, GeneratedEnvironmentMapLight, DomeBoundCamera)>();
        }
        return;
    };
    if iter.next().is_some() {
        warn_once!("[usd-bevy] scene authors more than one textured DomeLight — using the first");
    }
    if cube.0 == Handle::default() {
        return;
    }

    // The dome's `xformOp:rotate*` lands in its `Transform` via the shared
    // instantiation path, which is exactly how a USD author spins an
    // environment. Feed it to both components so the lighting and the visible
    // sky rotate together.
    let rotation = xform.map(|t| t.rotation()).unwrap_or(Quat::IDENTITY);

    for camera in &cameras {
        commands.entity(camera).try_insert((
            GeneratedEnvironmentMapLight {
                environment_map: cube.0.clone(),
                intensity: dome.intensity,
                rotation,
                ..default()
            },
            DomeBoundCamera,
        ));
        if dome.skybox {
            commands.entity(camera).try_insert(Skybox {
                image: Some(cube.0.clone()),
                brightness: dome.intensity,
                rotation,
            });
        } else {
            commands.entity(camera).remove::<Skybox>();
        }
    }
}

/// A source equirect flattened to linear RGBA — the projection's input.
pub struct Equirect {
    width: u32,
    height: u32,
    texels: Vec<[f32; 4]>,
}

impl Equirect {
    /// Decode every texel to linear RGBA once. `None` if the format has no
    /// CPU-side reader (e.g. a compressed BCn texture).
    fn flatten(image: &Image) -> Option<Self> {
        let width = image.texture_descriptor.size.width;
        let height = image.texture_descriptor.size.height;
        let mut texels = Vec::with_capacity((width as usize) * (height as usize));
        for y in 0..height {
            for x in 0..width {
                let c: LinearRgba = image.get_color_at(x, y).ok()?.into();
                texels.push(c.to_f32_array());
            }
        }
        Some(Self { width, height, texels })
    }

    /// Construct directly from linear texels (tests, and any future
    /// procedurally-generated sky).
    pub fn from_texels(width: u32, height: u32, texels: Vec<[f32; 4]>) -> Self {
        assert_eq!(texels.len(), (width as usize) * (height as usize));
        Self { width, height, texels }
    }

    /// Bilinear sample. Wraps in longitude (the seam is continuous — a sky is a
    /// cylinder in `u`) and clamps in latitude (the poles are not).
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let fx = u * self.width as f32 - 0.5;
        let fy = v * self.height as f32 - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;

        let wrap_x = |x: i64| x.rem_euclid(self.width as i64) as u32;
        let clamp_y = |y: i64| y.clamp(0, self.height as i64 - 1) as u32;

        let x0i = wrap_x(x0 as i64);
        let x1i = wrap_x(x0 as i64 + 1);
        let y0i = clamp_y(y0 as i64);
        let y1i = clamp_y(y0 as i64 + 1);

        let at = |x: u32, y: u32| self.texels[(y as usize) * (self.width as usize) + x as usize];
        let (a, b, c, d) = (at(x0i, y0i), at(x1i, y0i), at(x0i, y1i), at(x1i, y1i));

        let mut out = [0.0f32; 4];
        for i in 0..4 {
            let top = a[i] * (1.0 - tx) + b[i] * tx;
            let bot = c[i] * (1.0 - tx) + d[i] * tx;
            out[i] = top * (1.0 - ty) + bot * ty;
        }
        out
    }

    /// Sample along a world-space direction.
    ///
    /// USD orients a `DomeLight`'s latlong texture with its **centre at -Z** and
    /// **+Y up**, which is what this mapping reproduces: `u = 0.5` looks down
    /// -Z, `v = 0` is the +Y pole. An author who wants a different heading
    /// spins the prim (`xformOp:rotateY`) rather than editing the image.
    fn sample_dir(&self, d: Vec3) -> [f32; 4] {
        let u = 0.5 + d.x.atan2(-d.z) / core::f32::consts::TAU;
        let v = d.y.clamp(-1.0, 1.0).acos() / core::f32::consts::PI;
        self.sample(u, v)
    }
}

/// Direction through texel (`px`+`dx`, `py`+`dy`) on cubemap face `face`, using
/// wgpu's layer order: +X, -X, +Y, -Y, +Z, -Z. The sub-texel offsets are what
/// the supersampler perturbs.
fn face_dir_at(face: usize, px: u32, py: u32, size: u32, dx: f32, dy: f32) -> Vec3 {
    let a = 2.0 * (px as f32 + 0.5 + dx) / size as f32 - 1.0;
    let b = 2.0 * (py as f32 + 0.5 + dy) / size as f32 - 1.0;
    match face {
        0 => Vec3::new(1.0, -b, -a),
        1 => Vec3::new(-1.0, -b, a),
        2 => Vec3::new(a, 1.0, b),
        3 => Vec3::new(a, -1.0, -b),
        4 => Vec3::new(a, -b, 1.0),
        _ => Vec3::new(-a, -b, -1.0),
    }
    .normalize()
}

/// Direction through the centre of texel (`px`,`py`) on face `face`.
#[cfg(test)]
fn face_dir(face: usize, px: u32, py: u32, size: u32) -> Vec3 {
    face_dir_at(face, px, py, size, 0.0, 0.0)
}

/// Project an equirectangular image onto a cubemap.
///
/// 2×2 supersampled: a cube face magnifies the equator and *minifies* the poles
/// hard, and point-sampling that produces exactly the shimmering pole artefacts
/// HDRI skies are notorious for.
///
/// Output is `Rgba16Float` — the widest float format wgpu guarantees is
/// filterable, and IBL needs values well above 1.0 (a sun disc in an HDRI is
/// hundreds of nits; clamping it to LDR is what makes image-based lighting look
/// flat and grey).
pub fn equirect_to_cubemap(src: &Equirect, face_size: u32, tint: LinearRgba) -> Image {
    let size = face_size.max(1).next_power_of_two();
    let tint = tint.to_f32_array();
    let mut data: Vec<u8> = Vec::with_capacity((size as usize).pow(2) * 6 * 4 * 2);

    const SS: [f32; 2] = [-0.25, 0.25];
    for face in 0..6 {
        for py in 0..size {
            for px in 0..size {
                let mut acc = [0.0f32; 4];
                for dy in SS {
                    for dx in SS {
                        // Nudge in texel space, then re-derive the direction —
                        // supersampling the *sphere*, not the flat image.
                        let s = src.sample_dir(face_dir_at(face, px, py, size, dx, dy));
                        for i in 0..4 {
                            acc[i] += s[i];
                        }
                    }
                }
                for i in 0..4 {
                    let v = acc[i] / 4.0 * tint[i];
                    data.extend_from_slice(&half::f16::from_f32(v).to_le_bytes());
                }
            }
        }
    }

    Image {
        texture_view_descriptor: Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        }),
        ..Image::new(
            Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 6,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba16Float,
            RenderAssetUsages::RENDER_WORLD,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An equirect that encodes its own direction: red = +X-ness, green =
    /// +Y-ness, blue = +Z-ness, each remapped to 0..1. Projecting it and
    /// reading a face back tells us whether the two direction conventions
    /// (`face_dir` and `sample_dir`) actually agree — the bug that silently
    /// mirrors or rolls a sky.
    fn direction_probe(width: u32, height: u32) -> Equirect {
        let mut texels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;
                // Inverse of `sample_dir`.
                let phi = (u - 0.5) * core::f32::consts::TAU;
                let theta = v * core::f32::consts::PI;
                let d = Vec3::new(
                    theta.sin() * phi.sin(),
                    theta.cos(),
                    -theta.sin() * phi.cos(),
                );
                texels.push([d.x * 0.5 + 0.5, d.y * 0.5 + 0.5, d.z * 0.5 + 0.5, 1.0]);
            }
        }
        Equirect::from_texels(width, height, texels)
    }

    /// Round-trip: the colour a face texel receives must decode back to that
    /// texel's own direction. This is the whole correctness argument for the
    /// projection — orientation, handedness, and pole placement in one assert.
    #[test]
    fn projection_preserves_direction() {
        let src = direction_probe(256, 128);
        let size = 16u32;
        let img = equirect_to_cubemap(&src, size, LinearRgba::WHITE);

        assert_eq!(img.texture_descriptor.size.depth_or_array_layers, 6);
        assert_eq!(img.texture_descriptor.size.width, size);

        let data = img.data.as_ref().expect("cubemap has CPU data");
        let read = |face: usize, px: u32, py: u32, ch: usize| -> f32 {
            let texel = (face * (size * size) as usize)
                + (py as usize) * (size as usize)
                + px as usize;
            let off = (texel * 4 + ch) * 2;
            half::f16::from_le_bytes([data[off], data[off + 1]]).to_f32()
        };

        for face in 0..6 {
            for py in [0u32, size / 2, size - 1] {
                for px in [0u32, size / 2, size - 1] {
                    let expect = face_dir(face, px, py, size);
                    let got = Vec3::new(
                        read(face, px, py, 0) * 2.0 - 1.0,
                        read(face, px, py, 1) * 2.0 - 1.0,
                        read(face, px, py, 2) * 2.0 - 1.0,
                    );
                    // Supersampling averages 4 nearby directions, so the
                    // recovered vector is slightly shortened, not rotated —
                    // compare angle, and keep the tolerance loose enough to
                    // survive f16 quantisation at a 16² face.
                    let cos = got.normalize().dot(expect);
                    assert!(
                        cos > 0.99,
                        "face {face} texel ({px},{py}): expected {expect:?}, got {got:?} (cos {cos})",
                    );
                }
            }
        }
    }

    /// HDRI values above 1.0 are the entire point — they must survive to the
    /// cubemap, not get clamped to LDR on the way through.
    #[test]
    fn preserves_hdr_range() {
        let src = Equirect::from_texels(4, 2, vec![[50.0, 25.0, 10.0, 1.0]; 8]);
        let img = equirect_to_cubemap(&src, 4, LinearRgba::WHITE);
        let data = img.data.as_ref().unwrap();
        let r = half::f16::from_le_bytes([data[0], data[1]]).to_f32();
        assert!((r - 50.0).abs() < 0.1, "expected ~50.0, got {r}");
    }

    /// `inputs:color` is a multiplier on the image.
    #[test]
    fn tint_multiplies_image() {
        let src = Equirect::from_texels(4, 2, vec![[1.0, 1.0, 1.0, 1.0]; 8]);
        let img = equirect_to_cubemap(&src, 4, LinearRgba::rgb(0.5, 0.25, 0.0));
        let data = img.data.as_ref().unwrap();
        let px = |ch: usize| half::f16::from_le_bytes([data[ch * 2], data[ch * 2 + 1]]).to_f32();
        assert!((px(0) - 0.5).abs() < 0.01);
        assert!((px(1) - 0.25).abs() < 0.01);
        assert!(px(2).abs() < 0.01);
    }

    /// A non-power-of-two `lunco:dome:faceSize` is rounded up rather than
    /// rejected: `GeneratedEnvironmentMapLight` requires power-of-two and
    /// panics deep in the render graph otherwise.
    #[test]
    fn face_size_is_forced_power_of_two() {
        let src = Equirect::from_texels(4, 2, vec![[1.0; 4]; 8]);
        let img = equirect_to_cubemap(&src, 100, LinearRgba::WHITE);
        assert_eq!(img.texture_descriptor.size.width, 128);
    }
}

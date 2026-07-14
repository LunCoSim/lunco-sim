//! Binds [`SceneCamera`] intent to a real render pipeline.
//!
//! Domain crates spawn `Camera` + `SceneCamera` (both render-free) and filter on
//! `With<SceneCamera>`. This module attaches the `bevy_core_pipeline` half —
//! `Camera3d`, tonemapping, MSAA, bloom — which is what actually costs wgpu.
//!
//! See `lunco_render::camera` for why, and for the two `R4` bugs this closes.

use bevy::camera::Hdr;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use lunco_render::camera::{MsaaLevel, SceneCamera, ToneMap};

pub(crate) fn build(app: &mut App) {
    app.add_observer(bind_scene_camera)
        .add_systems(Update, rebind_changed_scene_camera);
}

fn tonemapping_of(t: ToneMap) -> Tonemapping {
    match t {
        ToneMap::None => Tonemapping::None,
        ToneMap::TonyMcMapface => Tonemapping::TonyMcMapface,
        ToneMap::AgX => Tonemapping::AgX,
        ToneMap::AcesFitted => Tonemapping::AcesFitted,
        ToneMap::Reinhard => Tonemapping::Reinhard,
    }
}

fn msaa_of(m: MsaaLevel) -> Msaa {
    match m {
        MsaaLevel::Off => Msaa::Off,
        MsaaLevel::X2 => Msaa::Sample2,
        MsaaLevel::X4 => Msaa::Sample4,
    }
}

/// Attach the pipeline components a `SceneCamera` describes.
///
/// Note `Camera.hdr` is set from the intent BEFORE bloom is considered — bloom on a
/// non-HDR target is a no-op that still pays for a downsample/upsample chain, which
/// is precisely the bug (`R4`) four crates in this repo shipped. Here it is refused,
/// loudly, instead of silently wasting the passes.
fn apply(commands: &mut Commands, e: Entity, cam: &SceneCamera) {
    let mut ec = commands.entity(e);
    ec.insert((
        Camera3d::default(),
        tonemapping_of(cam.tone_map),
        msaa_of(cam.msaa),
    ));

    // `Hdr` is a marker component in `bevy_camera` — render-FREE. So "this camera is
    // HDR" is expressible headless too; only the pipeline that acts on it is not.
    if cam.hdr {
        ec.insert(Hdr);
    } else {
        ec.remove::<Hdr>();
    }

    match (cam.bloom, cam.hdr) {
        (Some(b), true) => {
            ec.insert(Bloom {
                intensity: b.intensity,
                low_frequency_boost: b.low_frequency_boost,
                ..Bloom::default()
            });
        }
        (Some(_), false) => {
            warn!(
                "SceneCamera on {e:?} asks for bloom without hdr — refusing. Bloom on a \
                 non-HDR target renders nothing and still pays for the downsample chain. \
                 Use `SceneCamera::with_bloom`, which turns hdr on for you."
            );
            ec.remove::<Bloom>();
        }
        (None, _) => {
            ec.remove::<Bloom>();
        }
    }
}

fn bind_scene_camera(
    add: On<Add, SceneCamera>,
    cams: Query<&SceneCamera>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(cam) = cams.get(e) else { return };
    apply(&mut commands, e, cam);
}

/// Re-apply when the look is retuned live (the render-settings panel).
fn rebind_changed_scene_camera(
    changed: Query<(Entity, &SceneCamera), Changed<SceneCamera>>,
    mut commands: Commands,
) {
    for (e, cam) in &changed {
        apply(&mut commands, e, cam);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_render::camera::BloomLook;

    fn app() -> App {
        let mut a = App::new();
        a.add_plugins(MinimalPlugins);
        build(&mut a);
        a
    }

    /// The camera identity is render-free: a domain crate filters on `SceneCamera`,
    /// and the pipeline half is attached here.
    #[test]
    fn scene_camera_gains_a_pipeline() {
        let mut a = app();
        let e = a.world_mut().spawn((Camera::default(), SceneCamera::agx())).id();
        a.update();
        assert!(a.world().entity(e).contains::<Camera3d>());
        assert_eq!(a.world().entity(e).get::<Tonemapping>(), Some(&Tonemapping::AgX));
    }

    /// **R4, half one.** MSAA was never configured anywhere, so WebGL2 ran Bevy's
    /// default 4× on a full-screen terrain. Native gets 2×; wasm gets Off.
    #[test]
    fn msaa_is_actually_configured() {
        let mut a = app();
        let e = a.world_mut().spawn((Camera::default(), SceneCamera::default())).id();
        a.update();
        let expected = if cfg!(target_arch = "wasm32") { Msaa::Off } else { Msaa::Sample2 };
        assert_eq!(a.world().entity(e).get::<Msaa>(), Some(&expected));
    }

    /// **R4, half two.** Four crates configured Bloom on a camera with no HDR target,
    /// where it renders nothing and still costs a downsample/upsample chain. Asking
    /// for it without hdr must be refused, not silently honoured.
    #[test]
    fn bloom_without_hdr_is_refused() {
        let mut a = app();
        let e = a
            .world_mut()
            .spawn((
                Camera::default(),
                SceneCamera { bloom: Some(BloomLook::default()), hdr: false, ..Default::default() },
            ))
            .id();
        a.update();
        assert!(!a.world().entity(e).contains::<Bloom>(), "bloom on an LDR camera must not attach");
    }

    /// ...and `with_bloom` makes the correct thing the easy thing.
    #[test]
    fn with_bloom_turns_on_hdr() {
        let mut a = app();
        let e = a
            .world_mut()
            .spawn((Camera::default(), SceneCamera::default().with_bloom(BloomLook::default())))
            .id();
        a.update();
        assert!(a.world().entity(e).contains::<Bloom>());
        assert!(a.world().entity(e).contains::<Hdr>(), "bloom implies hdr");
    }
}

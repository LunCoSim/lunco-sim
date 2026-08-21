//! The render-bound arm of `lunco_environment::SetEnvironmentLight`: **bloom**.
//!
//! Everything else that command touches — the sun's `DirectionalLight`, its
//! `CascadeShadowConfig`, `GlobalAmbientLight`, camera `Exposure` — is render-free
//! (`bevy_light` / `bevy_camera`) and is applied by the observer that stayed in
//! `lunco-environment`. Bloom is `bevy_post_process` → `bevy_render` → wgpu, so it
//! must be applied from this side of the boundary. A command may carry as many
//! observers as it has effects; the second one lives here.
//!
//! # Why this writes `SceneCamera` and not `Bloom`
//!
//! [`SceneCamera`] is the render intent and the binder is the sole writer of the
//! concrete post-process component. This observer therefore records the authored
//! environment override on the intent, where it can be applied consistently to
//! cameras that exist now and cameras spawned later.

use bevy::prelude::*;
use lunco_environment::SetEnvironmentLight;
use lunco_render::camera::{BloomLook, SceneCamera};

pub(crate) fn build(app: &mut App) {
    app.add_observer(on_set_environment_light_bloom);
}

/// Apply `SetEnvironmentLight::bloom_intensity` to every scene camera's look.
///
/// Only the intensity is authored — `low_frequency_boost` keeps the current
/// Graphics camera setting (or the camera's existing value), matching the USD
/// environment contract.
fn on_set_environment_light_bloom(
    trigger: On<SetEnvironmentLight>,
    mut cams: Query<&mut SceneCamera>,
    settings: Res<lunco_render::RenderingQualitySettings>,
    mut bloom_override: ResMut<lunco_render::SceneBloomOverride>,
) {
    let cmd = trigger.event();
    let Some(intensity) = cmd.bloom_intensity else {
        return;
    };
    if !intensity.is_finite() || intensity < 0.0 {
        warn!("SetEnvironmentLight rejected non-finite or negative bloom intensity");
        return;
    }
    bloom_override.intensity = Some(intensity);
    for mut cam in &mut cams {
        let low_frequency_boost = cam.bloom.map_or_else(
            || settings.profile().camera_bloom_low_frequency_boost,
            |bloom| bloom.low_frequency_boost,
        );
        let next = (intensity > 0.0).then(|| BloomLook::new(intensity, low_frequency_boost));
        // Change-guarded: `SceneCamera` is `Changed`-driven on the binder side, so a
        // blind write would re-run the pipeline binding on every identical command.
        if cam.bloom != next {
            cam.bloom = next;
        }
        cam.hdr = cam.bloom.is_some();
    }
}

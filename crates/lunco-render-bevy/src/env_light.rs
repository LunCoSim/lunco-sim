//! The render-bound arm of `lunco_environment::SetEnvironmentLight`: **bloom**.
//!
//! Everything else that command touches — the sun's `DirectionalLight`, its
//! `CascadeShadowConfig`, `GlobalAmbientLight`, camera `Exposure` — is render-free
//! (`bevy_light` / `bevy_camera`) and is applied by the observer that stayed in
//! `lunco-environment`. Bloom is `bevy_post_process` → `bevy_render` → wgpu, so it
//! must be applied from this side of the boundary. A command may carry as many
//! observers as it has effects; the second one lives here.
//!
//! # Why this writes `SceneCamera` and not `Bloom` (review `R4`)
//!
//! `hdr` is set true NOWHERE in this repo. Bloom on a non-HDR view renders
//! **nothing** while still paying for a downsample/upsample chain — the command has
//! been quietly buying that cost for as long as it has existed.
//!
//! So this observer writes [`SceneCamera::bloom`] rather than inserting a raw
//! `Bloom`, and it deliberately **does not** turn `hdr` on. `scene_camera.rs`'s
//! binder then refuses the bloom and warns. That warning is the point: today's
//! visual output is preserved byte-for-byte (bloom rendered nothing before; it
//! renders nothing now), and the previously-silent bug becomes audible.
//!
//! Turning bloom on for real is a separate, deliberate visual decision — call
//! [`SceneCamera::with_bloom`], which enables `hdr` for you so the broken
//! combination stays unrepresentable. It is not something a decoupling refactor
//! whose whole premise is "zero behaviour change" gets to do by accident.

use bevy::prelude::*;
use lunco_environment::SetEnvironmentLight;
use lunco_render::camera::{BloomLook, SceneCamera};

pub(crate) fn build(app: &mut App) {
    app.add_observer(on_set_environment_light_bloom);
}

/// Apply `SetEnvironmentLight::bloom_intensity` to every scene camera's look.
///
/// Only the intensity is authored — `low_frequency_boost` keeps whatever the
/// camera already had (or the [`BloomLook`] default on a camera that had no bloom
/// at all), mirroring the old `for mut bloom in &mut q_bloom { bloom.intensity = i }`.
fn on_set_environment_light_bloom(
    trigger: On<SetEnvironmentLight>,
    mut cams: Query<&mut SceneCamera>,
) {
    let cmd = trigger.event();
    let Some(intensity) = cmd.bloom_intensity else { return };
    for mut cam in &mut cams {
        let low_frequency_boost =
            cam.bloom.map(|b| b.low_frequency_boost).unwrap_or(BloomLook::default().low_frequency_boost);
        let next = BloomLook { intensity, low_frequency_boost };
        // Change-guarded: `SceneCamera` is `Changed`-driven on the binder side, so a
        // blind write would re-run `apply` (and re-log the no-hdr warning) on every
        // slider frame even when the value is identical.
        if cam.bloom != Some(next) {
            cam.bloom = Some(next);
        }
        // NOTE: `hdr` is left alone ON PURPOSE — see the module docs. The binder
        // refuses bloom without it, which is exactly today's rendered result.
    }
}

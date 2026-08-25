//! Data-driven **camera cuts** — the editorial "camera track" (doc 35, slice 1).
//!
//! A prim (canonically `def Scope "CameraTrack"`) authoring
//! `token lunco:activeCamera.timeSamples` becomes a timeline track whose keys
//! select which scene camera is live over time. This turns cutscene camera cuts
//! from imperative `set_camera("…")` rhai calls into inspectable USD **data**
//! that scrubs with the animation transport:
//!
//! ```usda
//! def Scope "CameraTrack" {
//!     token lunco:activeCamera.timeSamples = {
//!         0:  "WideTrack",
//!         6:  "TrackCam",
//!         12: "DescentCam",
//!         28: "PadCam",
//!     }
//! }
//! ```
//!
//! `activeCamera` is **held**-interpolated — a cut is instantaneous, never a
//! blend: at any time the live camera is the value of the greatest key ≤ now
//! (clamped to the first key before the track starts). Whenever that held value
//! changes — including when the playhead is scrubbed backward — the track fires
//! the internal [`ActivateCamera`](crate::camera_switch::ActivateCamera)
//! trigger, reusing the single-authority viewport path
//! ([`reconcile_scene_viewport`](crate::camera_switch::reconcile_scene_viewport))
//! — no new camera plumbing.
//!
//! Mirrors the animation substrate exactly: a spawn-time marker ([`CameraTrack`]),
//! a once-derived plan ([`CameraTrackPlan`], a tier-1 RAM memo of the key list),
//! and a per-frame sampler ([`sample_camera_tracks`]) that only reads the held
//! value at `t`. The track is bound to the [`AnimationPreview`] domain
//! ([`bind_camera_tracks_to_preview`]) so play / pause / scrub / rate reach it,
//! and its keys grow the preview [`Playback`] range like any animated clip.

use bevy::prelude::*;
use lunco_render::SceneCamera;
use lunco_time::{AnimationPreview, Playback, ResolvedDomains, TimeBinding, WorldTime};

use crate::camera_switch::{
    resolve_named_camera, ActivateCamera, CameraSelectionOwner, CameraSelectionStatus,
    ViewportCameraSelection,
};
use crate::{
    attr_has_time_samples, read_token_timesamples, stage_time_codes_per_second, CanonicalStages,
    SdfPath, UsdPrimPath, UsdStageAsset,
};

/// The token channel a camera track keys: which camera is live over time.
pub const ACTIVE_CAMERA_ATTR: &str = "lunco:activeCamera";

/// True iff `path` authors `lunco:activeCamera` `timeSamples` — i.e. it is a
/// camera track and its entity should get the [`CameraTrack`] marker at spawn.
pub fn prim_is_camera_track(reader: &crate::StageView<'_>, path: &SdfPath) -> bool {
    attr_has_time_samples(reader, path, ACTIVE_CAMERA_ATTR)
}

/// Spawn-time marker: this prim carries an `lunco:activeCamera` timeline.
/// [`plan_camera_tracks`] derives its [`CameraTrackPlan`] once the stage loads.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct CameraTrack;

/// Tier-1 RAM memo of a camera track's keys, derived once from the stage.
///
/// The key list is a *structural* property of the composed stage (only the
/// sample time `t` changes frame to frame), so the sampler skips the reader walk
/// and does a cheap held lookup. `last` is the cursor: the camera name most
/// recently activated, so the sampler fires [`ActivateCamera`] only on an actual
/// cut. Cleared on stage hot-reload so it re-derives against new content.
#[derive(Component, Debug, Clone, Default)]
pub struct CameraTrackPlan {
    /// `(time_code, camera_name)` keys, ascending. Time codes are stage-native;
    /// the sampler converts resolved seconds → code via `time_codes_per_second`.
    pub keys: Vec<(f64, String)>,
    /// Stage `timeCodesPerSecond` (constant per stage) — seconds × this = code.
    pub time_codes_per_second: f64,
    /// The camera name last activated by this track (cut de-dup cursor).
    pub last: Option<String>,
    /// The director revision for which `last` was emitted. An explicit Resume
    /// command increments the shared revision so the held cut is re-applied
    /// after an operator override, even if its name is unchanged.
    pub last_director_revision: u64,
    /// Stable diagnostic for an unresolved track target. This suppresses
    /// per-frame log spam while keeping the owning camera status visible.
    pub last_error: Option<String>,
}

/// The held camera name at time code `t`: the value of the greatest key ≤ `t`,
/// clamped to the first key's value before the track starts. `None` for an
/// empty key list.
fn held_camera(keys: &[(f64, String)], t: f64) -> Option<&str> {
    let mut cur = keys.first().map(|(_, n)| n.as_str())?;
    for (kt, name) in keys {
        if *kt <= t {
            cur = name.as_str();
        } else {
            break;
        }
    }
    Some(cur)
}

/// Derive each [`CameraTrack`]'s [`CameraTrackPlan`] once, as soon as its stage
/// asset is loaded. Gated on `Without<CameraTrackPlan>`, so it retries per frame
/// only for tracks not yet planned and is empty in steady state.
pub fn plan_camera_tracks(
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
    mut status: ResMut<CameraSelectionStatus>,
    q: Query<(Entity, &UsdPrimPath), (With<CameraTrack>, Without<CameraTrackPlan>)>,
) {
    for (entity, prim) in &q {
        let Some(cs) = canonical.get(prim.stage_handle.id()) else {
            continue;
        };
        let view = cs.view();
        let reader = &view;
        let Ok(sdf_path) = SdfPath::new(prim.path.as_str()) else {
            continue;
        };
        let keys = read_token_timesamples(reader, &sdf_path, ACTIVE_CAMERA_ATTR);
        if keys.is_empty() {
            let message = format!(
                "camera track '{}' has no readable activeCamera time samples",
                prim.path
            );
            status.last_error = Some(message.clone());
            warn!("[camera] {message}");
            commands
                .entity(entity)
                .try_insert(CameraTrackPlan::default());
            continue;
        }
        commands.entity(entity).try_insert(CameraTrackPlan {
            time_codes_per_second: stage_time_codes_per_second(reader),
            keys,
            last: None,
            last_director_revision: 0,
            last_error: None,
        });
    }
}

/// Bind freshly-tagged [`CameraTrack`]s to the [`AnimationPreview`] domain so the
/// animation transport (play / pause / scrub / rate) drives which camera is live,
/// and grow the preview [`Playback`] range to cover the track's key span. Mirror
/// of `bind_animated_to_preview` for the editorial track. `Without<TimeBinding>`
/// leaves an explicit binding intact; absent time spine → stays on the world clock.
pub fn bind_camera_tracks_to_preview(
    preview: Option<Res<AnimationPreview>>,
    canonical: NonSend<CanonicalStages>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), (Added<CameraTrack>, Without<TimeBinding>)>,
    mut playback: Query<&mut Playback>,
) {
    let Some(preview) = preview else {
        return;
    };
    let mut span: Option<(f64, f64)> = None;
    for (entity, prim) in &q {
        commands.entity(entity).try_insert(TimeBinding {
            domain: preview.domain,
        });
        // Union the track's key span (seconds) into the range to grow the domain.
        if let Some(cs) = canonical.get(prim.stage_handle.id()) {
            let view = cs.view();
            if let Ok(sp) = SdfPath::new(prim.path.as_str()) {
                let tcps = stage_time_codes_per_second(&view);
                let keys = read_token_timesamples(&view, &sp, ACTIVE_CAMERA_ATTR);
                if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
                    let (a, b) = (first.0 / tcps, last.0 / tcps);
                    span = Some(match span {
                        Some((lo, hi)) => (lo.min(a), hi.max(b)),
                        None => (a, b),
                    });
                }
            }
        }
    }
    if let Some((a, b)) = span {
        if let Ok(mut pb) = playback.get_mut(preview.domain) {
            pb.start = pb.start.min(a);
            pb.end = pb.end.max(b);
        }
    }
}

/// Per-frame camera-track sampler: for each [`CameraTrackPlan`], resolve its
/// clock (bound domain or world), take the held camera name at that time, and —
/// when it differs from the last activated one — resolve the name to a camera
/// entity and fire [`ActivateCamera`]. Only fires on a cut (change), so it never
/// fights the viewport reconciler. Scrubbing backward re-evaluates the held key,
/// so the correct camera is shown at any playhead position.
///
/// If the named camera is absent or ambiguous, the track remains uncommitted
/// and publishes one stable diagnostic. It never selects another camera.
pub fn sample_camera_tracks(
    world: Res<WorldTime>,
    resolved: Res<ResolvedDomains>,
    selection: Res<ViewportCameraSelection>,
    mut q: Query<(&mut CameraTrackPlan, Option<&TimeBinding>, &UsdPrimPath)>,
    q_cams: Query<(Entity, &Name), With<SceneCamera>>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    for (mut plan, binding, prim) in &mut q {
        if selection.owner() == CameraSelectionOwner::User {
            continue;
        }
        if plan.keys.is_empty() {
            continue;
        }
        let secs = lunco_time::domain_time(&resolved, binding, &world);
        let t = secs * plan.time_codes_per_second;
        let Some(want) = held_camera(&plan.keys, t) else {
            continue;
        };
        let needs_resume = plan.last_director_revision < selection.director_revision();
        if plan.last.as_deref() == Some(want) && !needs_resume {
            continue;
        }
        match resolve_named_camera(want, &q_cams) {
            Ok(e) => {
                commands.trigger(ActivateCamera::director(e));
                plan.last = Some(want.to_string());
                plan.last_director_revision = selection.director_revision();
                if let Some(message) = plan.last_error.take() {
                    if status.last_error.as_deref() == Some(message.as_str()) {
                        status.last_error = None;
                    }
                }
            }
            Err(reason) => {
                let message = format!(
                    "camera track '{}' cannot activate '{want}': {reason}",
                    prim.path
                );
                if plan.last_error.as_deref() != Some(message.as_str()) {
                    warn!("[camera] {message}");
                    status.last_error = Some(message.clone());
                    plan.last_error = Some(message);
                }
            }
        }
    }
}

/// Drop cached [`CameraTrackPlan`]s for tracks whose stage was hot-reloaded, so
/// [`plan_camera_tracks`] re-derives them. Runs only on frames carrying a
/// `UsdStageAsset` reload event. Mirrors `clear_animation_plans_on_stage_reload`.
pub fn clear_camera_track_plans_on_stage_reload(
    mut ev: MessageReader<AssetEvent<UsdStageAsset>>,
    mut commands: Commands,
    q: Query<(Entity, &UsdPrimPath), With<CameraTrackPlan>>,
) {
    let reloaded: Vec<bevy::asset::AssetId<UsdStageAsset>> = ev
        .read()
        .filter_map(|e| match e {
            AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            _ => None,
        })
        .collect();
    if reloaded.is_empty() {
        return;
    }
    for (entity, prim) in &q {
        if reloaded.contains(&prim.stage_handle.id()) {
            commands.entity(entity).remove::<CameraTrackPlan>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_camera_clamps_before_first_and_holds() {
        let keys = vec![
            (0.0, "A".to_string()),
            (6.0, "B".to_string()),
            (12.0, "C".to_string()),
        ];
        // Before the first key → clamp to first.
        assert_eq!(held_camera(&keys, -3.0), Some("A"));
        // At and after a key, hold until the next.
        assert_eq!(held_camera(&keys, 0.0), Some("A"));
        assert_eq!(held_camera(&keys, 5.9), Some("A"));
        assert_eq!(held_camera(&keys, 6.0), Some("B"));
        assert_eq!(held_camera(&keys, 11.9), Some("B"));
        assert_eq!(held_camera(&keys, 100.0), Some("C"));
    }

    #[test]
    fn held_camera_empty_is_none() {
        assert_eq!(held_camera(&[], 1.0), None);
    }
}

//! Resolve the shared render-quality catalog from authored application policy.
//!
//! Mesh generation and scene projection consume these settings in graphical
//! and headless hosts. GPU recovery is a separate presentation concern.

use bevy::prelude::*;
use lunco_settings::AppSettingsExt;

use crate::{RenderingQuality, RenderingQualityProfiles, RenderingQualitySettings};

/// Update set for the Rhai-backed profile catalog and selected settings.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderQualityPolicySet;

/// Install the typed bridge between authored quality profiles and shared
/// runtime settings. The authored policy owns profile values; this plugin
/// validates the complete catalog before publishing it to consumers.
pub struct RenderQualityPolicyPlugin;

impl Plugin for RenderQualityPolicyPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<RenderingQualitySettings>()
            .init_resource::<RenderingQualityProfiles>()
            .add_systems(Startup, load_authored_render_quality_profiles)
            .add_systems(
                Update,
                load_authored_render_quality_profiles
                    .in_set(RenderQualityPolicySet)
                    .run_if(render_quality_profiles_stale),
            );
    }
}

/// Load and validate every profile from the typed Rhai policy before consumers
/// project quality-dependent scene visuals. Fresh settings use the authored
/// default; persisted user edits stay authoritative.
fn load_authored_render_quality_profiles(
    mut profiles: ResMut<RenderingQualityProfiles>,
    mut settings: ResMut<RenderingQualitySettings>,
) {
    let previous_preset = settings.preset(&profiles);
    let default_quality = match lunco_hooks::invoke(crate::RENDER_DEFAULT_QUALITY_PROFILE_HOOK, &[])
    {
        Some(Ok(lunco_hooks::HookValue::Str(id))) => match RenderingQuality::parse_id(&id) {
            Some(quality) => quality,
            None => {
                let reason = format!(
                    "authored default rendering-quality policy returned unknown profile id '{id}'"
                );
                profiles.mark_unavailable(&reason, lunco_hooks::generation());
                warn!("[render] {reason}");
                return;
            }
        },
        Some(Err(error)) => {
            let reason = format!("authored default rendering-quality policy failed: {error}");
            profiles.mark_unavailable(&reason, lunco_hooks::generation());
            warn!("[render] {reason}");
            return;
        }
        None => {
            let reason = "authored default rendering-quality policy is unavailable".to_string();
            profiles.mark_unavailable(&reason, lunco_hooks::generation());
            warn!("[render] {reason}");
            return;
        }
        Some(Ok(value)) => {
            let reason = format!(
                "authored default rendering-quality policy returned {}, expected string",
                value.type_name()
            );
            profiles.mark_unavailable(&reason, lunco_hooks::generation());
            warn!("[render] {reason}");
            return;
        }
    };

    let mut loaded = Vec::with_capacity(RenderingQuality::all().len());
    for quality in RenderingQuality::all() {
        let value = match lunco_hooks::invoke(
            crate::RENDER_QUALITY_PROFILE_HOOK,
            &[lunco_hooks::HookValue::str(quality.id())],
        ) {
            Some(Ok(value)) => value,
            Some(Err(error)) => {
                let reason = format!(
                    "authored rendering-quality policy '{}' failed: {error}",
                    quality.id()
                );
                profiles.mark_unavailable(&reason, lunco_hooks::generation());
                warn!("[render] {reason}");
                return;
            }
            None => {
                let reason = format!(
                    "authored rendering-quality policy '{}' is unavailable",
                    quality.id()
                );
                profiles.mark_unavailable(&reason, lunco_hooks::generation());
                warn!("[render] {reason}");
                return;
            }
        };
        let profile = match crate::RenderQualityProfile::from_policy_value(&value) {
            Ok(profile) => profile,
            Err(error) => {
                let reason = format!(
                    "authored rendering-quality profile '{}' is invalid: {error}",
                    quality.id()
                );
                profiles.mark_unavailable(&reason, lunco_hooks::generation());
                warn!("[render] {reason}");
                return;
            }
        };
        let mut validation = RenderingQualitySettings::default();
        validation.apply_profile(profile);
        if let Err(error) = validation.validate() {
            let reason = format!(
                "authored rendering-quality profile '{}' violates the render contract: {error}",
                quality.id()
            );
            profiles.mark_unavailable(&reason, lunco_hooks::generation());
            warn!("[render] {reason}");
            return;
        }
        loaded.push((quality, profile));
    }

    let generation = lunco_hooks::generation();
    if let Err(error) = profiles.install(loaded, default_quality, generation) {
        profiles.mark_unavailable(&error, generation);
        warn!("[render] {error}");
        return;
    }
    if !settings.is_profile_initialized() || settings.has_requested_profile() {
        if let Err(error) = settings.initialize_profile(&profiles) {
            warn!("[render] could not initialize selected quality profile: {error}");
        }
    } else if let Some(quality) = previous_preset {
        if let Some(profile) = profiles.get(quality) {
            if settings.profile() != profile {
                settings.apply_profile(profile);
            }
        }
    }
}

fn render_quality_profiles_stale(profiles: Res<RenderingQualityProfiles>) -> bool {
    profiles.is_stale()
}

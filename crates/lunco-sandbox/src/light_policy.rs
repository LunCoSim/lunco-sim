//! Engine-level light-handling policy — a *simulation setting*, not a per-rover
//! behaviour.
//!
//! ## The split
//!
//! *How a light is rendered* (does it cast a shadow, and when) is an **engine**
//! decision, so it lives here as a setting ([`ShadowCastingSettings`]). A rover's
//! *intrinsic* behaviour (it HAS headlights; it drives) stays authored on the
//! **vessel** in USD. This code never encodes anything rover-specific — it reads
//! possession generically and applies the engine policy to whichever vessel is
//! under local control.
//!
//! ## The default policy (`PossessedOnly`)
//!
//! Every shadow-casting spot/point light re-renders the whole scene into its own
//! shadow map each frame, so a field of parked rovers (two headlights each) stacks
//! up a dozen wasted shadow passes — profiled as the dominant render cost on the
//! moonbase twin. Local lights therefore spawn with shadows **off** (see
//! `lunco-usd-bevy/light.rs`), and the engine turns the projection back **on** only
//! for the vessel you are actually driving. Perf win (idle rovers stay cheap) + UX
//! win (your rover looks right).
//!
//! ## Reactive, and orchestration-only
//!
//! The policy is applied **on possession events** (observers on [`PossessVessel`] /
//! [`ReleaseVessel`]) and when the setting itself is toggled (`resource_changed`
//! run-condition) — never polled per frame. It runs in `Update`, never the fixed
//! sim tick, and only mutates a render flag (`shadow_maps_enabled`): firewalled from
//! the deterministic core.

use bevy::prelude::*;
use lunco_avatar::{PossessVessel, ReleaseVessel};
use lunco_controller::ControllerLink;
use lunco_core::SyncApplyGuard;
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

/// How the engine handles shadow casting for **local** lights — rover headlights
/// and fill lamps (a UsdLux `SphereLight` → `SpotLight`/`PointLight`), as opposed
/// to the scene-dominant sun (`DistantLight` → `DirectionalLight`, owned by the
/// environment).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum LocalLightShadows {
    /// No local light casts a shadow. Cheapest; lights still illuminate.
    Off,
    /// Only the locally-possessed vessel's lights cast shadows — the rover you
    /// drive projects, the parked ones stay cheap. The default.
    #[default]
    PossessedOnly,
    /// Every local light casts a shadow. Most expensive.
    All,
}

/// Simulation setting: local-light shadow handling. Persisted through
/// [`lunco_settings`] — mutate `ResMut<ShadowCastingSettings>` and it saves; the
/// change re-projects reactively via [`reapply_on_settings_change`].
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub(crate) struct ShadowCastingSettings {
    /// Policy for rover headlights / local fill lights. See [`LocalLightShadows`].
    pub local_lights: LocalLightShadows,
}

impl Default for ShadowCastingSettings {
    fn default() -> Self {
        Self {
            local_lights: LocalLightShadows::PossessedOnly,
        }
    }
}

impl SettingsSection for ShadowCastingSettings {
    const KEY: &'static str = "shadow_casting";
}

/// Set every local light's `shadow_maps_enabled` for `mode` and the `possessed`
/// vessel set. Called only on a possession/settings *event*, never per frame, so
/// iterating the (handful of) local lights here is free.
fn apply_projection(
    mode: LocalLightShadows,
    possessed: &[Entity],
    parents: &Query<&ChildOf>,
    q_spot: &mut Query<(Entity, &mut SpotLight)>,
    q_point: &mut Query<(Entity, &mut PointLight)>,
) {
    for (entity, mut light) in q_spot.iter_mut() {
        let want = shadow_wanted(entity, mode, possessed, parents);
        if light.shadow_maps_enabled != want {
            light.shadow_maps_enabled = want;
        }
    }
    for (entity, mut light) in q_point.iter_mut() {
        let want = shadow_wanted(entity, mode, possessed, parents);
        if light.shadow_maps_enabled != want {
            light.shadow_maps_enabled = want;
        }
    }
}

fn shadow_wanted(
    light: Entity,
    mode: LocalLightShadows,
    possessed: &[Entity],
    parents: &Query<&ChildOf>,
) -> bool {
    match mode {
        LocalLightShadows::Off => false,
        LocalLightShadows::All => true,
        // A light "belongs to" the vessel it hangs under; walk the ChildOf chain
        // up from the light to a possessed vessel root.
        LocalLightShadows::PossessedOnly => is_descendant_of_any(light, possessed, parents),
    }
}

fn is_descendant_of_any(mut entity: Entity, roots: &[Entity], parents: &Query<&ChildOf>) -> bool {
    loop {
        if roots.contains(&entity) {
            return true;
        }
        match parents.get(entity) {
            Ok(child_of) => entity = child_of.parent(),
            Err(_) => return false,
        }
    }
}

/// Reactive: a LOCAL possession makes the newly-controlled vessel's headlights
/// cast shadows (under `PossessedOnly`) and every other local light stop. A
/// possession *swap* A→B is handled by this alone — the recompute keys off the new
/// possessed set `{target}`, so A's lights fall out and turn off.
fn project_on_possess(
    trigger: On<PossessVessel>,
    guard: Res<SyncApplyGuard>,
    settings: Res<ShadowCastingSettings>,
    parents: Query<&ChildOf>,
    mut q_spot: Query<(Entity, &mut SpotLight)>,
    mut q_point: Query<(Entity, &mut PointLight)>,
) {
    // A possession applied from the wire (host attributing a remote client's
    // claim) is not *our* camera/render — only local possessions drive local
    // shadow projection. Mirrors `on_possess_command`'s guard.
    if guard.is_from_sync() {
        return;
    }
    apply_projection(
        settings.local_lights,
        &[trigger.event().target],
        &parents,
        &mut q_spot,
        &mut q_point,
    );
}

/// Reactive: releasing local control drops the projection (nothing is locally
/// possessed → all local lights off under `PossessedOnly`).
fn project_on_release(
    _trigger: On<ReleaseVessel>,
    guard: Res<SyncApplyGuard>,
    settings: Res<ShadowCastingSettings>,
    parents: Query<&ChildOf>,
    mut q_spot: Query<(Entity, &mut SpotLight)>,
    mut q_point: Query<(Entity, &mut PointLight)>,
) {
    if guard.is_from_sync() {
        return;
    }
    apply_projection(
        settings.local_lights,
        &[],
        &parents,
        &mut q_spot,
        &mut q_point,
    );
}

/// Reactive (runs only when the setting changes — `resource_changed` — including
/// the initial insert): re-project for the currently-possessed set when the policy
/// is toggled at runtime, or to establish the baseline at startup.
fn reapply_on_settings_change(
    settings: Res<ShadowCastingSettings>,
    links: Query<&ControllerLink>,
    parents: Query<&ChildOf>,
    mut q_spot: Query<(Entity, &mut SpotLight)>,
    mut q_point: Query<(Entity, &mut PointLight)>,
) {
    let possessed: Vec<Entity> = links.iter().map(|l| l.vessel_entity).collect();
    apply_projection(
        settings.local_lights,
        &possessed,
        &parents,
        &mut q_spot,
        &mut q_point,
    );
}

fn project_on_light_added(
    _trigger: On<Add, SpotLight>,
    settings: Res<ShadowCastingSettings>,
    links: Query<&ControllerLink>,
    parents: Query<&ChildOf>,
    mut q_spot: Query<(Entity, &mut SpotLight)>,
    mut q_point: Query<(Entity, &mut PointLight)>,
) {
    let possessed: Vec<Entity> = links.iter().map(|l| l.vessel_entity).collect();
    apply_projection(
        settings.local_lights,
        &possessed,
        &parents,
        &mut q_spot,
        &mut q_point,
    );
}

/// Registers the [`ShadowCastingSettings`] simulation setting and the reactive
/// observers/system that project it onto local lights.
pub(crate) struct LightPolicyPlugin;

impl Plugin for LightPolicyPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<ShadowCastingSettings>();
        app.add_observer(project_on_possess);
        app.add_observer(project_on_release);
        app.add_observer(project_on_light_added);
        app.add_systems(
            Update,
            reapply_on_settings_change.run_if(resource_changed::<ShadowCastingSettings>),
        );
    }
}

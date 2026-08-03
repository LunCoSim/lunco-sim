//! LunCoSim visualization framework.
//!
//! See `README.md` for architecture — the three layers (signal / viz /
//! view), the dependency direction, and the roadmap for 3D / Panel3D /
//! additional viz kinds.
//!
//! This crate deliberately stays domain-agnostic: it knows how to route
//! typed samples into renderers, and nothing about Modelica, Avian, or
//! any specific producer. Domain crates depend on `lunco-viz`, not the
//! reverse.
//!
//! # Feature `ui`
//!
//! Everything that renders — viz kinds, `VizPanel`, the registry
//! plumbing, [`LuncoVizPlugin`] — sits behind the `ui` feature (off by
//! default), which is what links bevy_egui/egui_plot/workbench. A plain
//! dependency gets only the [`signal`] re-export of `lunco-signal`, so
//! it stays render-free.

#[cfg(feature = "ui")]
pub mod kinds;
#[cfg(feature = "ui")]
pub mod panel;
#[cfg(feature = "ui")]
pub mod plot_fmt;
#[cfg(feature = "ui")]
pub mod registry;
pub mod signal;
#[cfg(feature = "ui")]
pub mod telemetry_browser;
#[cfg(feature = "ui")]
pub mod view;
#[cfg(feature = "ui")]
pub mod viz;

#[cfg(feature = "ui")]
pub use kinds::line_plot::{LinePlot, LINE_PLOT_KIND};
#[cfg(feature = "ui")]
pub use panel::{VizPanel, VIZ_PANEL_KIND};
#[cfg(feature = "ui")]
pub use registry::{AppVizExt, VisualizationRegistry, VizFitRequests, VizKindCatalog};
pub use signal::{
    PersistedSignalRef, ScalarHistory, ScalarSample, SignalMeta, SignalRef, SignalRegistry,
    SignalType, TelemetryFocus,
};
#[cfg(feature = "ui")]
pub use telemetry_browser::{
    bind_dropped_channel, drain_plot_drops, plot_node_at, ChannelDragPayload, PlotDropRequest,
    TelemetryBrowserPanel, TELEMETRY_BROWSER_PANEL_ID,
};
#[cfg(feature = "ui")]
pub use view::{Panel2DCtx, ViewKind, ViewTarget};
#[cfg(feature = "ui")]
pub use viz::{RoleSpec, SignalBinding, Visualization, VisualizationConfig, VizId, VizKindId};

#[cfg(feature = "ui")]
use bevy::prelude::*;
#[cfg(feature = "ui")]
use lunco_workbench::WorkbenchAppExt;
#[cfg(feature = "ui")]
use std::collections::HashMap;

/// Default sample capacity per scalar signal. ~20k samples covers
/// roughly 5–6 minutes of 60 Hz stepping or ~3 hours at 2 Hz — long
/// enough that scrolling back through a long-running simulation is
/// useful, short enough that the ring buffer stays under ~2 MB per
/// signal worst-case (16 B × 20 000).
pub const DEFAULT_SIGNAL_HISTORY: usize = 20_000;

/// Install the visualization framework.
///
/// Registers:
///
/// * `SignalRegistry` resource (default sample horizon
///   `DEFAULT_SIGNAL_HISTORY`).
/// * `VisualizationRegistry` + `VizKindCatalog` resources.
/// * Built-in `LinePlot` viz kind.
/// * `VizPanel` as a multi-instance workbench panel.
/// * [`TelemetryBrowserPanel`] as a singleton side-browser panel.
///
/// Domain plugins (`ModelicaPlugin`, future Avian bridge, …) are
/// expected to be added *after* this plugin so they see the registry
/// resources on app build.
#[cfg(feature = "ui")]
pub struct LuncoVizPlugin;

#[cfg(feature = "ui")]
impl Plugin for LuncoVizPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SignalRegistry::with_default_capacity(
            DEFAULT_SIGNAL_HISTORY,
        ))
        .init_resource::<VisualizationRegistry>()
        .init_resource::<VizKindCatalog>()
        .init_resource::<VizFitRequests>()
        .register_visualization::<LinePlot>()
        .register_instance_panel(VizPanel::default())
        .register_panel(TelemetryBrowserPanel::default())
        .add_observer(kinds::line_plot::on_line_plot_edit_requested)
        .add_observer(kinds::line_plot::on_line_plot_fit_requested)
        .add_observer(panel::on_bind_channel_requested)
        .add_observer(telemetry_browser::on_open_visualization_requested)
        // A plot config survives scene replacement; its Bevy entity does not.
        // Reconcile after the scene has had a chance to publish replacement
        // content IDs, before the next UI frame reads the config.
        .add_systems(Update, reconcile_persisted_plot_bindings);
    }
}

/// Refresh every live plot source from the stable content-derived identity
/// captured while it was first bound. A source that is not in the new scene is
/// deliberately left unresolved: the line plot renders an explicit waiting
/// state and retries every frame, rather than silently plotting a different
/// entity with a coincidentally similar mnemonic.
#[cfg(feature = "ui")]
fn reconcile_persisted_plot_bindings(
    mut plots: ResMut<VisualizationRegistry>,
    ids: Query<(Entity, &lunco_core::GlobalEntityId)>,
    signals: Res<SignalRegistry>,
) {
    let mut gid_of = HashMap::new();
    let mut entity_of = HashMap::new();
    for (entity, gid) in &ids {
        gid_of.insert(entity, *gid);
        entity_of.insert(*gid, entity);
    }
    // Pre-identity layouts carried only a session-local entity plus mnemonic.
    // There is one safe migration: exactly one currently published scalar with
    // that mnemonic. Zero is unavailable and more than one is ambiguous, so
    // neither case is silently rebound to the wrong vehicle.
    let mut unique_scalar_by_path: HashMap<String, Option<SignalRef>> = HashMap::new();
    for (source, signal_type) in signals.iter_signals() {
        if signal_type != SignalType::Scalar {
            continue;
        }
        match unique_scalar_by_path.entry(source.path.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(source.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }

    for config in plots.values_mut() {
        for binding in &mut config.inputs {
            reconcile_persisted_binding(binding, &gid_of, &entity_of, &unique_scalar_by_path);
        }
        if config.kind == LINE_PLOT_KIND {
            kinds::line_plot::reconcile_persisted_x_source(config, &gid_of, &entity_of);
        }
    }
}

#[cfg(feature = "ui")]
fn reconcile_persisted_binding(
    binding: &mut SignalBinding,
    gid_of: &HashMap<Entity, lunco_core::GlobalEntityId>,
    entity_of: &HashMap<lunco_core::GlobalEntityId, Entity>,
    unique_scalar_by_path: &HashMap<String, Option<SignalRef>>,
) -> bool {
    let mut changed = false;
    if binding.persisted_source.is_none() {
        // A saved pre-identity binding can no longer name its owner. Adopt a
        // live source only when its mnemonic is unambiguous, then immediately
        // capture the stable identity so this fallback is never needed again.
        if !gid_of.contains_key(&binding.source.entity) {
            if let Some(Some(source)) = unique_scalar_by_path.get(&binding.source.path) {
                if binding.source != *source {
                    binding.source = source.clone();
                    changed = true;
                }
            }
        }
        let persisted = binding.source.to_persisted(|e| gid_of.get(&e).copied());
        if persisted.is_some() {
            binding.persisted_source = persisted;
            changed = true;
        }
    }
    if let Some(persisted) = &binding.persisted_source {
        if let Some(source) = persisted.resolve(|gid| entity_of.get(&gid).copied()) {
            if binding.source != source {
                binding.source = source;
                changed = true;
            }
        }
    }
    changed
}

#[cfg(all(test, feature = "ui"))]
mod tests {
    use super::*;

    #[test]
    fn persisted_binding_retargets_a_reloaded_entity() {
        let old = Entity::from_raw_u32(7).unwrap();
        let replacement = Entity::from_raw_u32(9).unwrap();
        let gid = lunco_core::GlobalEntityId::from_raw(42);
        let mut binding = SignalBinding::live(SignalRef::new(old, "power.soc"), "y");
        let mut old_ids = HashMap::new();
        old_ids.insert(old, gid);
        let mut entities = HashMap::new();
        entities.insert(gid, replacement);
        let mut unique = HashMap::new();
        unique.insert(
            "power.soc".to_string(),
            Some(SignalRef::new(replacement, "power.soc")),
        );
        assert!(reconcile_persisted_binding(
            &mut binding,
            &old_ids,
            &entities,
            &unique,
        ));
        assert_eq!(binding.source, SignalRef::new(replacement, "power.soc"));
    }

    #[test]
    fn legacy_binding_only_adopts_an_unambiguous_live_mnemonic() {
        let stale = Entity::from_raw_u32(7).unwrap();
        let replacement = Entity::from_raw_u32(9).unwrap();
        let gid = lunco_core::GlobalEntityId::from_raw(42);
        let mut binding = SignalBinding::live(SignalRef::new(stale, "power.soc"), "y");
        let mut ids = HashMap::new();
        ids.insert(replacement, gid);
        let mut entities = HashMap::new();
        entities.insert(gid, replacement);
        let mut unique = HashMap::new();
        unique.insert(
            "power.soc".to_string(),
            Some(SignalRef::new(replacement, "power.soc")),
        );

        assert!(reconcile_persisted_binding(
            &mut binding,
            &ids,
            &entities,
            &unique,
        ));
        assert_eq!(binding.source, SignalRef::new(replacement, "power.soc"));
        assert!(binding.persisted_source.is_some());

        let mut ambiguous = SignalBinding::live(SignalRef::new(stale, "power.soc"), "y");
        unique.insert("power.soc".to_string(), None);
        assert!(!reconcile_persisted_binding(
            &mut ambiguous,
            &ids,
            &entities,
            &unique,
        ));
        assert_eq!(ambiguous.source, SignalRef::new(stale, "power.soc"));
        assert!(ambiguous.persisted_source.is_none());
    }
}

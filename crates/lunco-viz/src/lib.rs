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
    compact_channel_label, display_channel_label, humanize_identifier, operator_channel_label,
    PersistedSignalRef, ScalarHistory, ScalarSample, SignalExposure, SignalMeta, SignalRef,
    SignalRegistry, SignalType, TelemetryFocus,
};
#[cfg(feature = "ui")]
pub use telemetry_browser::{
    bind_dropped_channel, drain_plot_drops, plot_node_at, ChannelDragPayload, PlotDropRequest,
    SetTelemetryBrowserView, TelemetryBrowserPanel, TelemetryBrowserView, TelemetryDisplaySettings,
    TELEMETRY_BROWSER_PANEL_ID,
};
#[cfg(feature = "ui")]
pub use view::{Panel2DCtx, ViewKind, ViewTarget};
#[cfg(feature = "ui")]
pub use viz::{RoleSpec, SignalBinding, Visualization, VisualizationConfig, VizId, VizKindId};

#[cfg(feature = "ui")]
use bevy::prelude::*;
#[cfg(feature = "ui")]
use lunco_settings::AppSettingsExt;
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
        .register_settings_section::<telemetry_browser::TelemetryDisplaySettings>()
        .init_resource::<VisualizationRegistry>()
        .init_resource::<telemetry_browser::TelemetryBrowserView>()
        .init_resource::<VizKindCatalog>()
        .init_resource::<VizFitRequests>()
        .register_visualization::<LinePlot>()
        .register_instance_panel(VizPanel)
        .register_panel(TelemetryBrowserPanel::default())
        .add_observer(kinds::line_plot::on_line_plot_edit_requested)
        .add_observer(kinds::line_plot::on_line_plot_fit_requested)
        .add_observer(panel::on_bind_channel_requested)
        .add_observer(telemetry_browser::on_open_visualization_requested)
        // A plot config survives scene replacement; its Bevy entity does not.
        // Reconcile after the scene has had a chance to publish replacement
        // content IDs, before the next UI frame reads the config.
        .add_systems(Update, reconcile_persisted_plot_bindings);
        telemetry_browser::register_all_commands(app);
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
) {
    let mut gid_of = HashMap::new();
    let mut entity_of = HashMap::new();
    for (entity, gid) in &ids {
        gid_of.insert(entity, *gid);
        entity_of.insert(*gid, entity);
    }

    for config in plots.values_mut() {
        for binding in &mut config.inputs {
            reconcile_persisted_binding(binding, &gid_of, &entity_of);
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
) -> bool {
    let mut changed = false;
    if binding.persisted_source.is_none() {
        // A live binding captures its stable owner identity while the entity is
        // present. A stale binding without that identity stays unresolved; its
        // mnemonic is not sufficient to choose another entity.
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
    fn stable_binding_retargets_a_reloaded_entity() {
        let old = Entity::from_raw_u32(7).unwrap();
        let replacement = Entity::from_raw_u32(9).unwrap();
        let gid = lunco_core::GlobalEntityId::from_raw(42);
        let mut binding = SignalBinding::live(SignalRef::new(old, "power.soc"), "y");
        let mut old_ids = HashMap::new();
        old_ids.insert(old, gid);
        let mut entities = HashMap::new();
        entities.insert(gid, replacement);
        assert!(reconcile_persisted_binding(
            &mut binding,
            &old_ids,
            &entities,
        ));
        assert_eq!(binding.source, SignalRef::new(replacement, "power.soc"));
    }

    #[test]
    fn unpersisted_binding_stays_unresolved_after_reload() {
        let stale = Entity::from_raw_u32(7).unwrap();
        let replacement = Entity::from_raw_u32(9).unwrap();
        let gid = lunco_core::GlobalEntityId::from_raw(42);
        let mut binding = SignalBinding::live(SignalRef::new(stale, "power.soc"), "y");
        let mut ids = HashMap::new();
        ids.insert(replacement, gid);
        let mut entities = HashMap::new();
        entities.insert(gid, replacement);
        assert!(!reconcile_persisted_binding(&mut binding, &ids, &entities));
        assert_eq!(binding.source, SignalRef::new(stale, "power.soc"));
        assert!(binding.persisted_source.is_none());
    }
}

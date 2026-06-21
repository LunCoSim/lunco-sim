//! UI-reactive observers of CORE state.
//!
//! These systems are the *reactive UI layer built on top of the core*: they
//! read core domain state (e.g. [`crate::msl_remote::MslLoadState`]) and
//! project it into UI surfaces (the workbench status bus, console, plots).
//! The core never references these surfaces — it just owns the observable
//! state. All of this is `ui`-feature only; a headless build has no observers
//! and therefore no egui/workbench dependency.

use bevy::prelude::*;
use lunco_workbench::status_bus::{StatusBus, StatusLevel};
use lunco_viz::{SignalMeta, SignalRef, SignalRegistry, VisualizationRegistry};

use lunco_assets::msl::{MslLoadPhase, MslLoadState};

const MSL_SOURCE: &str = "MSL";

/// Watch [`MslLoadState`] and translate transitions / progress ticks into
/// [`StatusBus`] events. Phase changes become discrete `Info` entries
/// (preserved in history); byte/file counts within a phase become `Progress`
/// ticks (updated in place).
///
/// This is a pure state mirror, not a task owner — `MslLoadState` itself is the
/// lifetime authority, so it uses the legacy `push_progress`/`clear_progress`
/// API (implicitly `BusyScope::Global`, matching MSL-preload-affects-everything
/// semantics) rather than `begin` + `BusyHandle`.
pub fn mirror_msl_state_to_status_bus(
    state: Res<MslLoadState>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<Option<MirrorMemo>>,
) {
    let Some(mut bus) = bus else {
        return;
    };
    let now_summary = MirrorMemo::from(&*state);
    let prior_phase_label = last.as_ref().and_then(|m| m.phase_label);

    match &*state {
        MslLoadState::NotStarted => {}
        MslLoadState::Loading { phase, bytes_done, bytes_total } => {
            let label = msl_phase_label(*phase);
            // Phase transition → discrete history entry.
            if prior_phase_label != Some(label) {
                bus.push(MSL_SOURCE, StatusLevel::Info, label);
            }
            // Progress tick (in-place; doesn't accumulate in history).
            let detail = format_progress_detail(*phase, *bytes_done, *bytes_total);
            bus.push_progress(MSL_SOURCE, detail, *bytes_done, *bytes_total);
        }
        MslLoadState::Ready { file_count, .. } => {
            // Only fire once per Ready transition (re-renders shouldn't spam).
            if !matches!(last.as_ref(), Some(MirrorMemo { ready: true, .. })) {
                bus.push(MSL_SOURCE, StatusLevel::Info, format!("ready — {file_count} files"));
                bus.clear_progress(MSL_SOURCE);
            }
        }
        MslLoadState::Failed(msg) => {
            if !matches!(last.as_ref(), Some(MirrorMemo { failed: true, .. })) {
                bus.push(MSL_SOURCE, StatusLevel::Error, msg.clone());
                bus.clear_progress(MSL_SOURCE);
            }
        }
    }

    *last = Some(now_summary);
}

fn msl_phase_label(p: MslLoadPhase) -> &'static str {
    match p {
        MslLoadPhase::FetchingManifest => "fetching manifest",
        MslLoadPhase::FetchingBundle => "downloading",
        MslLoadPhase::LoadingCache => "loading from cache",
        MslLoadPhase::Decompressing => "decompressing",
        MslLoadPhase::Parsing => "loading",
    }
}

fn format_progress_detail(phase: MslLoadPhase, done: u64, total: u64) -> String {
    let label = msl_phase_label(phase);
    match phase {
        MslLoadPhase::Parsing if total > 0 => format!("{label} {done} / {total}"),
        _ if total > 0 => format!(
            "{label} — {:.1} / {:.1} MB",
            done as f64 / 1_048_576.0,
            total as f64 / 1_048_576.0,
        ),
        _ => label.to_string(),
    }
}

/// One-frame memo so the mirror only emits discrete history entries on actual
/// transitions (not on every re-render of the same state).
#[derive(Default)]
pub struct MirrorMemo {
    phase_label: Option<&'static str>,
    ready: bool,
    failed: bool,
}

impl From<&MslLoadState> for MirrorMemo {
    fn from(s: &MslLoadState) -> Self {
        match s {
            MslLoadState::NotStarted => Self::default(),
            MslLoadState::Loading { phase, .. } => Self {
                phase_label: Some(msl_phase_label(*phase)),
                ..Self::default()
            },
            MslLoadState::Ready { .. } => Self { ready: true, ..Self::default() },
            MslLoadState::Failed(_) => Self { failed: true, ..Self::default() },
        }
    }
}

/// Drain core live-sim sample batches ([`crate::SimSampleStream`]) into the viz
/// `SignalRegistry` — the reactive UI projection of the running simulation.
///
/// This is the plot-aware half of the old `worker::handle_modelica_responses`
/// viz block; the core handler now only appends UI-agnostic samples. Per batch:
/// clear history on a fresh compile, push every scalar, attach doc-index
/// descriptions on compile/param-update, and reset the default graph bindings.
pub fn drain_sim_samples_to_viz(
    mut stream: ResMut<crate::SimSampleStream>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut viz_registry: Option<ResMut<VisualizationRegistry>>,
    doc_registry: Option<Res<crate::state::ModelicaDocumentRegistry>>,
) {
    if stream.batches.is_empty() {
        return;
    }
    // Always take (so the queue can't grow); drop if there's no SignalRegistry.
    let batches = std::mem::take(&mut stream.batches);
    let Some(sigs) = signals.as_deref_mut() else {
        return;
    };
    for batch in &batches {
        if batch.is_new_model {
            for (name, _) in &batch.samples {
                sigs.clear_history(&SignalRef::new(batch.entity, name.clone()));
            }
        }
        for (name, val) in &batch.samples {
            sigs.push_scalar(SignalRef::new(batch.entity, name.clone()), batch.time, *val);
        }
        // Descriptions from the document index (canonical AST projection),
        // looked up by leaf name — refreshed on compile-type results.
        if batch.is_new_model || batch.is_parameter_update {
            let index_ref = doc_registry
                .as_deref()
                .and_then(|r| r.host(batch.document))
                .map(|h| h.document().index());
            if let Some(index) = index_ref {
                for (name, _) in &batch.samples {
                    let Some(entry) = index.find_component_by_leaf(name) else {
                        continue;
                    };
                    if entry.description.is_empty() {
                        continue;
                    }
                    sigs.update_meta(
                        SignalRef::new(batch.entity, name.clone()),
                        SignalMeta {
                            description: Some(entry.description.clone()),
                            unit: None,
                            provenance: Some("modelica".to_string()),
                        },
                    );
                }
            }
        }
        // A fresh compile starts the default plot empty (users add signals via
        // the Telemetry panel) — clear any stale bindings from a prior model.
        if batch.is_new_model {
            if let Some(reg) = viz_registry.as_deref_mut() {
                if let Some(cfg) = reg.get_mut(crate::ui::viz::DEFAULT_MODELICA_GRAPH) {
                    cfg.inputs.clear();
                }
            }
        }
    }
}

/// Reactive UI: project core [`crate::ModelicaNotice`] events into the Console
/// panel. The core worker emits notices; this observer renders them.
pub fn drain_notices_to_console(
    mut notices: MessageReader<crate::ModelicaNotice>,
    console: Option<ResMut<crate::ui::panels::console::ConsoleLog>>,
) {
    let Some(mut console) = console else {
        notices.clear();
        return;
    };
    for n in notices.read() {
        match n.level {
            crate::NoticeLevel::Info => console.info(n.text.clone()),
            crate::NoticeLevel::Warn => console.warn(n.text.clone()),
            crate::NoticeLevel::Error => console.error(n.text.clone()),
        }
    }
}

/// Reactive UI: project core `SourceRootRegistry` load-state transitions into
/// the status bar — progress while `Loading`, a completion entry on
/// `Ready`/`Failed`. Core sets the registry state; it no longer touches the bus.
pub fn mirror_source_roots_to_status_bus(
    registry: Option<Res<crate::source_roots::SourceRootRegistry>>,
    bus: Option<ResMut<StatusBus>>,
    mut last: Local<std::collections::HashMap<String, u8>>,
) {
    use crate::source_roots::{LoadState, STATUS_BUS_SOURCE};
    let (Some(registry), Some(mut bus)) = (registry, bus) else {
        return;
    };
    for (id, root) in &registry.roots {
        let disc = match &root.state {
            LoadState::NotLoaded => 0u8,
            LoadState::Loading { .. } => 1,
            LoadState::Ready => 2,
            LoadState::Failed(_) => 3,
        };
        if last.get(id) == Some(&disc) {
            continue;
        }
        match &root.state {
            LoadState::NotLoaded => {}
            LoadState::Loading { .. } => {
                bus.push_progress(STATUS_BUS_SOURCE, format!("Loading library `{id}`…"), 0, 0);
                bus.push(STATUS_BUS_SOURCE, StatusLevel::Info, format!("Loading library `{id}`"));
            }
            LoadState::Ready => {
                bus.clear_progress(STATUS_BUS_SOURCE);
                bus.push(STATUS_BUS_SOURCE, StatusLevel::Info, format!("Library `{id}` ready"));
            }
            LoadState::Failed(msg) => {
                bus.clear_progress(STATUS_BUS_SOURCE);
                bus.push(
                    STATUS_BUS_SOURCE,
                    StatusLevel::Warn,
                    format!("Library `{id}` load failed: {msg}"),
                );
            }
        }
        last.insert(id.clone(), disc);
    }
}

/// Reactive UI: translate core [`crate::CompileRequested`] events into the UI
/// `CompileModel` command. The core stepper asks for a compile without ever
/// naming the UI command type.
pub fn relay_compile_requests(
    mut requests: MessageReader<crate::CompileRequested>,
    mut commands: Commands,
) {
    for r in requests.read() {
        commands.trigger(crate::ui::commands::CompileModel {
            doc: r.doc,
            class: r.class.clone(),
            force: r.force,
            resume_after_compile: r.resume_after_compile,
        });
    }
}

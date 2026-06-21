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

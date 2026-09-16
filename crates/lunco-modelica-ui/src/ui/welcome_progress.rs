//! Welcome-panel progress tracking.
//!
//! A tiny persisted ledger of "how many times has this example been
//! opened" keyed by source library qualified name. Drives the progress dots
//! (⚪/✅) and "X of N" counters on the Welcome learning paths.
//!
//! Scope is deliberately small:
//!
//!  * **One counter** (`opens`) per qualified class name. Bumped when
//!    `OpenClass` fires. That's enough to feel like progress without
//!    inventing a telemetry pipeline — compile/run/step tracking can
//!    hook later observers in the same way.
//!
//!  * **Persistence** is the `welcome_progress` section of the central
//!    `settings.json` document. The settings owner handles the portable
//!    storage backend and atomic flush, so this UI state does not create a
//!    second per-feature file.
//!
//!  * The section is loaded and flushed by `lunco-settings`, which also
//!    isolates test settings from the user's real configuration.
//!
//! Kept in `ui/` rather than `state.rs` so the Welcome panel owns
//! its own concern; `ExampleProgress` is a normal Bevy `Resource`
//! and any panel can read it.

use std::collections::HashMap;

use bevy::prelude::*;
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};

/// Persistent open-count ledger keyed by the fully-qualified class
/// name (e.g. `"Modelica.Blocks.Examples.PID_Controller"`). Missing
/// entries are treated as zero — don't insert on read.
#[derive(Resource, Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExampleProgress {
    #[serde(default)]
    pub opens: HashMap<String, u32>,
}

impl ExampleProgress {
    /// Total opens for `qualified`. Zero when never opened.
    pub fn opens_of(&self, qualified: &str) -> u32 {
        self.opens.get(qualified).copied().unwrap_or(0)
    }

    /// `true` when the user has opened `qualified` at least once.
    pub fn is_opened(&self, qualified: &str) -> bool {
        self.opens_of(qualified) > 0
    }

    /// Count of the entries in `qualifieds` the user has opened.
    /// Used by the path-header "X of N" summary.
    pub fn opened_count<'a, I>(&self, qualifieds: I) -> usize
    where
        I: IntoIterator<Item = &'a str>,
    {
        qualifieds.into_iter().filter(|q| self.is_opened(q)).count()
    }
}

impl SettingsSection for ExampleProgress {
    const KEY: &'static str = "welcome_progress";
}

/// Observer registered in the Modelica commands plugin that bumps
/// the open-counter for the target qualified name every time the
/// user opens a class via `OpenClass` (drill-in, source library palette click,
/// Welcome card click all route through this event).
///
/// The central settings persister flushes the changed section at the end of
/// the frame through the configured storage backend.
pub fn on_open_class_for_progress(
    trigger: On<lunco_modelica_ui_core::OpenClass>,
    mut progress: ResMut<ExampleProgress>,
) {
    let qualified = trigger.event().qualified.clone();
    if qualified.is_empty() {
        return;
    }
    *progress.opens.entry(qualified).or_insert(0) += 1;
}

/// Plugin stub: inserts the resource and registers the observer.
/// Wired from `ModelicaUiPlugin` or the commands plugin — one call
/// and the ledger is live.
pub struct WelcomeProgressPlugin;

impl Plugin for WelcomeProgressPlugin {
    fn build(&self, app: &mut App) {
        app.register_settings_section::<ExampleProgress>()
            .add_observer(on_open_class_for_progress);
    }
}

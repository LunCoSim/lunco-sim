//! Welcome-panel progress tracking.
//!
//! A tiny persisted ledger of "how many times has this example been
//! opened" keyed by MSL qualified name. Drives the progress dots
//! (⚪/✅) and "X of N" counters on the Welcome learning paths.
//!
//! Scope is deliberately small:
//!
//!  * **One counter** (`opens`) per qualified class name. Bumped when
//!    `OpenClass` fires. That's enough to feel like progress without
//!    inventing a telemetry pipeline — compile/run/step tracking can
//!    hook later observers in the same way.
//!
//!  * **Persistence** is a single JSON file under the assets cache
//!    (`<cache>/welcome_progress.json`). Saved inline on every bump;
//!    cheap at 15-entry scale and means a crash can't eat the
//!    progress.
//!
//!  * **Storage location** matches `msl_index.json` — both live in
//!    the workspace cache so power-users can reset by deleting the
//!    cache dir, and so CI/test runs don't pollute a user's real
//!    home directory.
//!
//! Kept in `ui/` rather than `state.rs` so the Welcome panel owns
//! its own concern; `ExampleProgress` is a normal Bevy `Resource`
//! and any panel can read it.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Persistent open-count ledger keyed by the fully-qualified class
/// name (e.g. `"Modelica.Blocks.Examples.PID_Controller"`). Missing
/// entries are treated as zero — don't insert on read.
#[derive(Resource, Default, Serialize, Deserialize, Clone, Debug)]
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
        qualifieds
            .into_iter()
            .filter(|q| self.is_opened(q))
            .count()
    }
}

fn progress_file_path() -> PathBuf {
    lunco_assets::cache_dir().join("welcome_progress.json")
}

/// Load the ledger from disk at startup. Missing file / parse error
/// → fresh empty ledger. Logged at debug so test runs don't spam.
pub fn load_progress() -> ExampleProgress {
    let path = progress_file_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            bevy::log::debug!(
                "welcome_progress: couldn't parse {:?} ({e}) — starting fresh",
                path
            );
            ExampleProgress::default()
        }),
        Err(_) => ExampleProgress::default(),
    }
}

/// Write the ledger back to disk. Best-effort — failure is logged
/// and swallowed, since this is UX polish, not data-of-record.
pub fn save_progress(progress: &ExampleProgress) {
    let path = progress_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(progress) {
        Ok(s) => {
            if let Err(e) = std::fs::write(&path, s) {
                bevy::log::warn!(
                    "welcome_progress: couldn't write {:?}: {e}",
                    path
                );
            }
        }
        Err(e) => bevy::log::warn!("welcome_progress: serialize failed: {e}"),
    }
}

/// Observer registered in the Modelica commands plugin that bumps
/// the open-counter for the target qualified name every time the
/// user opens a class via `OpenClass` (drill-in, MSL palette click,
/// Welcome card click all route through this event).
///
/// Saves to disk inline — there's no in-memory ledger worth
/// batching at this volume (one write per click).
pub fn on_open_class_for_progress(
    trigger: On<crate::ui::commands::OpenClass>,
    mut progress: ResMut<ExampleProgress>,
) {
    let qualified = trigger.event().qualified.clone();
    if qualified.is_empty() {
        return;
    }
    *progress.opens.entry(qualified).or_insert(0) += 1;
    save_progress(&progress);
}

/// Plugin stub: inserts the resource and registers the observer.
/// Wired from `ModelicaUiPlugin` or the commands plugin — one call
/// and the ledger is live.
pub struct WelcomeProgressPlugin;

impl Plugin for WelcomeProgressPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_progress())
            .add_observer(on_open_class_for_progress);
    }
}

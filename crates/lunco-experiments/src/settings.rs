//! Persisted settings shared by experiment execution and artifact persistence.
use bevy::prelude::Resource;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// Platform default for the number of runs allowed to execute
/// concurrently (the "auto" setting). Both branches leave one logical core
/// for the UI/main thread and clamp low; the user can override via
/// `experiments.max_parallel`.
///
/// Native: `available_parallelism() - 1`. Wasm: `hardwareConcurrency - 1`,
/// clamped tighter because each pooled worker is a full second wasm instance
/// carrying its own copy of the (large) source library bundle — so concurrency there
/// trades real memory, not just CPU. `hardwareConcurrency` is logical cores
/// (or 0/absent when the browser hides it → fall back to 1).
fn default_max_parallel() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(1).clamp(1, 4))
            .unwrap_or(2)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let cores = web_sys::window()
            .map(|w| w.navigator().hardware_concurrency())
            .filter(|n| n.is_finite() && *n >= 1.0)
            .map(|n| n as usize)
            .unwrap_or(1);
        cores.saturating_sub(1).clamp(1, 4)
    }
}

/// Persisted experiment-execution settings (`settings.json` key
/// `experiments`). Owned here, the feature that consumes it.
#[derive(Resource, Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct ExperimentSettings {
    /// Max Fast Runs allowed to execute concurrently. `None` (or `0`) means
    /// "auto" — the platform default ([`default_max_parallel`]). A user
    /// value is clamped to at least 1. Kept conservative by default because
    /// each concurrent run holds a full DAE + result buffer and (cache-cold)
    /// a rumoca compile; raise it to use more cores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    /// Trajectory and serialized-artifact budgets shared by execution and persistence.
    #[serde(default)]
    pub result_limits: crate::RunResultLimits,
}

impl SettingsSection for ExperimentSettings {
    const KEY: &'static str = "experiments";
}

impl ExperimentSettings {
    /// Resolve to a concrete cap: the user value (clamped ≥1) when set and
    /// non-zero, else the platform default.
    pub fn resolved_max_parallel(&self) -> usize {
        match self.max_parallel {
            Some(n) if n >= 1 => n,
            _ => default_max_parallel(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn experiment_parallel_setting_preserves_auto_and_explicit_caps() {
        assert!(default_max_parallel() >= 1);
        for value in [None, Some(0), Some(1), Some(3)] {
            let settings = ExperimentSettings {
                max_parallel: value,
                ..Default::default()
            };
            assert_eq!(
                settings.resolved_max_parallel(),
                value
                    .filter(|value| *value > 0)
                    .unwrap_or_else(default_max_parallel)
            );
            assert_eq!(settings.result_limits, crate::RunResultLimits::default());
        }
    }
}

//! USD runtime policy values that are shared by headless and UI adapters.

/// Twin-manifest key controlling persistence of generated USD runtime edits.
pub const RUNTIME_PERSISTENCE_SETTING: &str = "usd.runtime_persistence";

/// Read the runtime-persistence policy from one Twin manifest.
///
/// The setting is disabled by omission and during an isolated run. A
/// malformed value is returned as an authoring error so callers cannot turn a
/// typo into an unexpected filesystem write.
pub fn runtime_persistence_for_twin(twin: &lunco_twin::Twin) -> Result<bool, String> {
    if lunco_twin::isolated_run_requested() {
        return Ok(false);
    }
    let Some(manifest) = twin.manifest.as_ref() else {
        return Ok(false);
    };
    match manifest.setting(RUNTIME_PERSISTENCE_SETTING) {
        None => Ok(false),
        Some(lunco_twin::TwinSettingValue::Bool(enabled)) => Ok(*enabled),
        Some(value) => Err(format!(
            "`{RUNTIME_PERSISTENCE_SETTING}` must be a boolean, got {value:?}"
        )),
    }
}

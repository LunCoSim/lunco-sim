//! Runtime subsystem toggles — the "progressive fidelity" substrate.
//!
//! An authored scenario can ramp simulation fidelity one concept at a time:
//! start with kinematic driving, then switch on thermal, then comms degradation,
//! etc. Rather than each subsystem inventing its own runtime flag, they share
//! ONE resource ([`SubsystemToggles`]) flipped by ONE generic command that a
//! Rhai step can call:
//!
//! The command accepts a name registered by the owning subsystem plugin.
//!
//! **Opt-in gating.** A subsystem registers its own toggle name when its plugin
//! is added, then honours [`SubsystemToggles::enabled`] in its systems. An
//! unset registered toggle defaults to `true`, so adding the substrate changes
//! nothing until a subsystem opts in. Both the resource and its command live in
//! this owner so the command is available to every host without a tutorial
//! dependency.

use bevy::prelude::*;
use std::collections::{BTreeSet, HashMap};

/// Runtime enable/disable state per subsystem. Missing key ⇒ enabled (`true`),
/// so the toggle only ever *removes* fidelity a tutorial hasn't introduced yet.
#[derive(Resource, Default, Debug, Clone)]
pub struct SubsystemToggles {
    enabled: HashMap<String, bool>,
    registered: BTreeSet<String>,
}

/// Enable or disable a runtime subsystem registered by its owning plugin.
///
/// The command is intentionally generic: authored scenarios may use it for a
/// progressive-fidelity flow, while the core only validates the registered
/// subsystem name and publishes the resulting state.
#[crate::Command(default)]
pub struct SetSubsystemEnabled {
    /// Registered subsystem key.
    pub name: String,
    /// `true` enables, `false` disables.
    pub on: bool,
}

#[crate::on_command(SetSubsystemEnabled)]
fn on_set_subsystem_enabled(
    trigger: On<SetSubsystemEnabled>,
    mut toggles: ResMut<SubsystemToggles>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    if !toggles.is_registered(&ev.name) {
        warn!(
            "[subsystem] unknown subsystem '{}' (registered: {:?}) — ignored",
            ev.name,
            toggles.registered_names()
        );
        return;
    }
    if !toggles.set(ev.name.clone(), ev.on) {
        warn!(
            "[subsystem] '{}' was unregistered before its state could be set",
            ev.name
        );
        return;
    }
    info!("[subsystem] {} = {}", ev.name, ev.on);
    commands.trigger(crate::TelemetryEvent {
        name: format!("subsystem:{}", ev.name),
        source: 0,
        severity: crate::Severity::Info,
        data: crate::TelemetryValue::Bool(ev.on),
        timestamp: 0.0,
    });
}

crate::register_commands!(on_set_subsystem_enabled);

impl SubsystemToggles {
    /// Register the toggle owned by a subsystem plugin.
    pub fn register(&mut self, name: impl Into<String>) -> bool {
        let name = name.into();
        if name.is_empty()
            || name
                .bytes()
                .any(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'))
        {
            return false;
        }
        self.registered.insert(name)
    }

    /// Is `name` currently enabled? An unset registered toggle is enabled.
    pub fn enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(true)
    }

    /// Set a registered subsystem's state. Unknown names are rejected and do
    /// not create a dead toggle with no consuming subsystem.
    pub fn set(&mut self, name: impl Into<String>, on: bool) -> bool {
        let name = name.into();
        if !self.registered.contains(&name) {
            return false;
        }
        self.enabled.insert(name, on);
        true
    }

    /// True if a subsystem plugin has registered `name`.
    pub fn is_registered(&self, name: &str) -> bool {
        self.registered.contains(name)
    }

    /// Registered names in deterministic order, for diagnostics and APIs.
    pub fn registered_names(&self) -> Vec<String> {
        self.registered.iter().cloned().collect()
    }
}

/// Init [`SubsystemToggles`]. Called from [`LunCoCorePlugin`](crate::LunCoCorePlugin)
/// so every build has the substrate and its generic toggle command.
pub(crate) fn build_subsystems(app: &mut App) {
    app.init_resource::<SubsystemToggles>();
    register_all_commands(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsystem_plugins_register_their_own_names() {
        let mut toggles = SubsystemToggles::default();
        assert!(!toggles.set("obstacle-field", false));
        assert!(toggles.register("obstacle-field"));
        assert!(!toggles.register("obstacle-field"));
        assert!(toggles.set("obstacle-field", false));
        assert!(!toggles.enabled("obstacle-field"));
        assert_eq!(toggles.registered_names(), vec!["obstacle-field"]);
    }

    #[test]
    fn invalid_names_are_not_registered() {
        let mut toggles = SubsystemToggles::default();
        assert!(!toggles.register("Thermal"));
        assert!(!toggles.register(""));
        assert!(!toggles.is_registered("Thermal"));
    }
}

//! Runtime subsystem toggles — the "progressive fidelity" substrate.
//!
//! A tutorial (spec 011, Story 2) ramps simulation fidelity one concept at a
//! time: start with kinematic driving, then switch on thermal, then comms
//! degradation, etc. Rather than each subsystem inventing its own runtime flag,
//! they share ONE resource ([`SubsystemToggles`]) flipped by ONE command
//! (`SetSubsystemEnabled`, defined in `lunco-tutorial` — the `#[Command]` derive
//! can't expand inside `lunco-core` itself) that a rhai step can call:
//!
//! ```rhai
//! cmd("SetSubsystemEnabled", #{ name: "thermal", on: true });
//! ```
//!
//! **Opt-in gating.** A subsystem honours a toggle by reading
//! [`SubsystemToggles::enabled`] in its own systems; absence defaults to `true`,
//! so adding the substrate changes nothing until a subsystem opts in. The set of
//! valid names is an allow-list ([`SUBSYSTEMS`]) so a typo is rejected loudly
//! instead of silently creating a dead flag. The resource lives here (every crate
//! depends on `lunco-core`); the command that flips it lives in `lunco-tutorial`.

use bevy::prelude::*;
use std::collections::HashMap;

/// The subsystems a tutorial may toggle. Extend as subsystems opt into gating.
/// Keep names short, stable, and lower-kebab.
pub const SUBSYSTEMS: &[&str] = &["thermal", "comms-degradation", "obstacle-field"];

/// Runtime enable/disable state per subsystem. Missing key ⇒ enabled (`true`),
/// so the toggle only ever *removes* fidelity a tutorial hasn't introduced yet.
#[derive(Resource, Default, Debug, Clone)]
pub struct SubsystemToggles {
    enabled: HashMap<String, bool>,
}

impl SubsystemToggles {
    /// Is `name` currently enabled? Unknown/unset ⇒ `true` (opt-in gating).
    pub fn enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(true)
    }

    /// Set `name`'s state. No allow-list check here — the command handler
    /// validates before calling; direct callers are trusted.
    pub fn set(&mut self, name: impl Into<String>, on: bool) {
        self.enabled.insert(name.into(), on);
    }

    /// True if `name` is a recognised subsystem ([`SUBSYSTEMS`]).
    pub fn is_known(name: &str) -> bool {
        SUBSYSTEMS.contains(&name)
    }
}

/// Init [`SubsystemToggles`]. Called from [`LunCoCorePlugin`](crate::LunCoCorePlugin)
/// so every build has the substrate; the `SetSubsystemEnabled` command that
/// mutates it is registered by `lunco-tutorial`.
pub(crate) fn build_subsystems(app: &mut App) {
    app.init_resource::<SubsystemToggles>();
}

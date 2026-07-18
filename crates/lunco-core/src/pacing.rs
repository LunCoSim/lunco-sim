//! Frame-pacing intent, shared across crates.
//!
//! Winit's `unfocused_mode` is a single global knob that several subsystems have
//! an opinion about, and the last writer each frame wins. `lunco-modelica`'s
//! `sim_focus_pace` re-pegs it every frame (Continuous while a Modelica sim runs,
//! the binary's idle policy otherwise), so any other crate that merely *sets*
//! `WinitSettings` has its choice silently reverted on the next frame.
//!
//! [`KeepAwake`] is how a subsystem states the intent instead of fighting over the
//! knob: whoever paces winit ORs these requests in. It is a counter, not a bool, so
//! overlapping requesters cannot clobber one another — each takes a token and drops
//! it when done.
//!
//! It lives in `lunco-core` because both the requester (`lunco-workbench`'s offline
//! recorder) and the pacer (`lunco-modelica`) depend on core, and neither depends on
//! the other.

use bevy::prelude::*;

/// Outstanding requests to keep the app updating continuously, ignoring the
/// unfocused power-saving throttle.
///
/// The canonical requester is offline frame recording: an unattended capture run
/// has no focused window, and under `reactive_low_power` the app sleeps between
/// redraws, stretching a frame from ~50 ms to whole seconds.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct KeepAwake(pub u32);

impl KeepAwake {
    /// Take a token — the app should update continuously until it is released.
    pub fn acquire(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    /// Release a previously taken token. Saturating, so an unbalanced release
    /// cannot wrap into "everyone wants to stay awake forever".
    pub fn release(&mut self) {
        self.0 = self.0.saturating_sub(1);
    }

    /// Whether anything currently wants continuous updates.
    pub fn wanted(&self) -> bool {
        self.0 > 0
    }
}

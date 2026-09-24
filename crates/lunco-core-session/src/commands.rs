//! Typed commands owned by the session and identity subsystem.

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};

use crate::authority::{claim_control, release_control_target};
use crate::{LocalSession, NetworkRole, SessionRbac, SessionRegistry, SyncApplyGuard};

/// Claim a stable control endpoint for the originating session.
///
/// The command only changes session authority. Embodiment camera binding and
/// controller-specific composition remain with the higher-level command that
/// needs them.
#[Command]
pub struct ClaimControl {
    /// Endpoint whose global identity becomes controlled.
    #[authz_target]
    pub target: Entity,
}

/// Release one stable control endpoint owned by the originating session.
#[Command]
pub struct ReleaseControlClaim {
    /// Endpoint whose global identity is released.
    #[authz_target]
    pub target: Entity,
}

/// Update the display name associated with the active user session.
#[Command(default)]
pub struct UpdateProfile {
    /// New session display name.
    pub name: String,
}

/// Apply a generic authority claim from a local or host-authorized origin.
#[on_command(ClaimControl)]
fn on_claim_control(
    trigger: On<ClaimControl>,
    role: Res<NetworkRole>,
    guard: Res<SyncApplyGuard>,
    local: Res<LocalSession>,
    rbac: Res<SessionRbac>,
    mut registry: ResMut<SessionRegistry>,
    q_identity: Query<&lunco_core::GlobalEntityId, With<lunco_port_core::InputPorts>>,
    mut commands: Commands,
) {
    // A client sends the command to the host and learns the authoritative
    // result through the ownership snapshot. The host applies the reflected
    // command under SyncApplyGuard; standalone applies it locally.
    if matches!(*role, NetworkRole::Client) && !guard.is_from_sync() {
        return;
    }
    let origin = guard.0.unwrap_or(local.0);
    let target = trigger.event().target;
    let Ok(gid) = q_identity.get(target) else {
        warn!(target = ?target, "[control] claim refused: target has no writable input endpoint");
        return;
    };
    match claim_control(&mut registry, &rbac, origin, gid.get()) {
        Ok(change) => commands.trigger(change),
        Err(error) => warn!(target = ?target, origin = ?origin, "[control] claim refused: {error}"),
    }
}

/// Apply a generic authority release from a local or host-authorized origin.
#[on_command(ReleaseControlClaim)]
fn on_release_control_claim(
    trigger: On<ReleaseControlClaim>,
    role: Res<NetworkRole>,
    guard: Res<SyncApplyGuard>,
    local: Res<LocalSession>,
    mut registry: ResMut<SessionRegistry>,
    q_identity: Query<&lunco_core::GlobalEntityId>,
    mut commands: Commands,
) {
    if matches!(*role, NetworkRole::Client) && !guard.is_from_sync() {
        return;
    }
    let origin = guard.0.unwrap_or(local.0);
    let target = trigger.event().target;
    let Ok(gid) = q_identity.get(target) else {
        warn!(target = ?target, "[control] release refused: target has no global identity");
        return;
    };
    match release_control_target(&mut registry, origin, gid.get()) {
        Ok(change) => commands.trigger(change),
        Err(error) => {
            warn!(target = ?target, origin = ?origin, "[control] release refused: {error}")
        }
    }
}

register_commands!(on_claim_control, on_release_control_claim);

//! Typed commands owned by the session and identity subsystem.

use bevy::prelude::*;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{Command, on_command, register_commands};
use lunco_hooks::HookValue;

use crate::authority::{claim_control, release_control_target};
use crate::{
    LocalSession, NetworkRole, SessionInputStream, SessionInputStreamSettings, SessionRbac,
    SessionRegistry, SyncApplyGuard,
};

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

/// Begin a bounded in-memory capture of admitted simulation inputs.
#[Command(default)]
pub struct StartSessionInputCapture {}

/// Stop the active session-input capture while retaining its records.
#[Command(default)]
pub struct StopSessionInputCapture {}

/// Discard a stopped or failed in-memory session-input capture.
#[Command(default)]
pub struct ClearSessionInputCapture {}

fn session_input_capture_ack(stream: &SessionInputStream) -> Result<Ack, String> {
    let record_count = u64::try_from(stream.records().len())
        .map_err(|_| "session input record count exceeds the API integer range".to_owned())?;
    let record_limit = u64::try_from(stream.record_limit())
        .map_err(|_| "session input record limit exceeds the API integer range".to_owned())?;
    let state = match stream.state() {
        crate::SessionInputStreamState::Idle => "idle",
        crate::SessionInputStreamState::Recording => "recording",
        crate::SessionInputStreamState::Complete => "complete",
        crate::SessionInputStreamState::Failed => "failed",
    };
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([
            ("state", HookValue::str(state)),
            ("record_count", HookValue::UInt(record_count)),
            ("record_limit", HookValue::UInt(record_limit)),
            (
                "failure",
                stream.failure().map_or(HookValue::Unit, HookValue::str),
            ),
        ]),
    ))
}

#[on_command(StartSessionInputCapture)]
fn on_start_session_input_capture(
    _trigger: On<StartSessionInputCapture>,
    settings: Res<SessionInputStreamSettings>,
    mut stream: ResMut<SessionInputStream>,
) -> Result<Ack, String> {
    stream.begin(settings.max_records_per_capture)?;
    session_input_capture_ack(&stream)
}

#[on_command(StopSessionInputCapture)]
fn on_stop_session_input_capture(
    _trigger: On<StopSessionInputCapture>,
    mut stream: ResMut<SessionInputStream>,
) -> Result<Ack, String> {
    if !stream.is_recording() {
        return Err("session input capture is not recording".to_owned());
    }
    stream.finish();
    session_input_capture_ack(&stream)
}

#[on_command(ClearSessionInputCapture)]
fn on_clear_session_input_capture(
    _trigger: On<ClearSessionInputCapture>,
    mut stream: ResMut<SessionInputStream>,
) -> Result<Ack, String> {
    stream.clear()?;
    session_input_capture_ack(&stream)
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

register_commands!(
    on_claim_control,
    on_release_control_claim,
    on_start_session_input_capture,
    on_stop_session_input_capture,
    on_clear_session_input_capture,
);

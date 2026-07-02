//! Autopilot = a user with a specialty (spec 034).
//!
//! An autopilot is **not** a bespoke control layer and has nothing to do with the
//! human avatar. It is an ordinary *actor*: a session in the shared
//! [`lunco_core::SessionRegistry`] that carries the [`AuthorityRole::AiAgent`] role
//! and drives its vessel from a behaviour instead of a keyboard. It is fully
//! **headless** — this crate depends only on `lunco-core` (session + ports) and
//! `lunco-cosim` (the `SetPorts` command), never on the avatar/UI.
//!
//! Because control authority is just vessel *ownership*, the model is inherently
//! **multi-actor**: any number of vessels can each be owned by a different session
//! — some humans, some autopilots — with no central arbiter. Each [`Autopilot`]
//! owns one vessel and drives only that vessel while it owns it:
//!
//! - **Engage** → the autopilot registers its `AiAgent` session and `claim`s the
//!   vessel (exactly as a player does on possess).
//! - **Drive** → each fixed tick, every engaged autopilot that still `owns` its
//!   vessel emits one `SetPorts`. That is the single writer for that vessel.
//! - **Yield** → if ownership transfers (another actor possessed/claimed the
//!   vessel), `owns` goes false and the autopilot simply stops writing. No
//!   disengage polling, no per-frame arbiter, no coupling to *who* took over or by
//!   what command. Losing ownership IS the yield signal.
//!
//! The human keyboard path yields symmetrically: `lunco_controller::drive_from_bindings`
//! drives only vessels the local session owns. So a human and an autopilot never
//! write the same vessel on the same tick — the rover-jitter root cause is gone.
//!
//! This prototype drives with constant `throttle`/`steer` setpoints to exercise the
//! *authority* mechanism. A production autopilot swaps the constant setpoints for a
//! bound rhai behaviour (`nav_to`/`steer_to`, already in
//! `assets/scripting/prelude/nav.rhai`) — the ownership plumbing here is unchanged
//! by what produces the setpoint.

use bevy::prelude::*;
use lunco_core::session::{AuthorityRole, SessionRbac, UserSession};
use lunco_core::{GlobalEntityId, NetworkRole, SessionId, SessionRegistry};
use lunco_cosim::SetPorts;

/// Base of the reserved `SessionId` band for local autopilots. Kept clear of
/// [`SessionId::LOCAL`] (`0`, the human/host) and of host-minted client ids
/// (allocated low, per connection). A fleet offsets by index within this band, so
/// each autopilot is a distinct actor with its own ownership.
pub const AUTOPILOT_SESSION_BASE: u64 = 0xA0_7000;

/// Session id for the autopilot at `index` within the reserved band.
pub fn autopilot_session(index: u64) -> SessionId {
    SessionId(AUTOPILOT_SESSION_BASE + index)
}

/// An autonomous driver for `vessel`, acting under its own [`AiAgent`]
/// [`session`](Self::session). Add it (with `engaged = true`) to any entity — its
/// session possesses `vessel` and drives it. Many can coexist, each owning a
/// different vessel.
///
/// [`AiAgent`]: AuthorityRole::AiAgent
#[derive(Component, Debug, Clone)]
pub struct Autopilot {
    /// The vessel this autopilot drives.
    pub vessel: Entity,
    /// The autopilot's own session identity (role `AiAgent`). Distinct per actor —
    /// see [`autopilot_session`].
    pub session: SessionId,
    /// Whether the autopilot currently holds + drives the vessel. Set true to
    /// engage. It self-clears if the claim is refused (vessel already owned).
    pub engaged: bool,
    /// Constant forward setpoint `[-1, 1]` (prototype behaviour).
    pub throttle: f64,
    /// Constant steer setpoint `[-1, 1]` (prototype behaviour).
    pub steer: f64,
}

impl Autopilot {
    /// An engaged autopilot for `vessel`, actor `index`, driving straight forward
    /// at `throttle`.
    pub fn forward(vessel: Entity, index: u64, throttle: f64) -> Self {
        Self { vessel, session: autopilot_session(index), engaged: true, throttle, steer: 0.0 }
    }
}

/// Host/standalone: when an [`Autopilot`] appears, register its `AiAgent` session
/// (so the role lattice + authorization treat it as a first-class user) and
/// `claim` its vessel. A refused claim (vessel already owned by another actor)
/// leaves it disengaged rather than fighting for control.
pub fn setup_autopilot_session(
    role: Res<NetworkRole>,
    mut q: Query<&mut Autopilot, Added<Autopilot>>,
    q_gid: Query<&GlobalEntityId>,
    mut rbac: ResMut<SessionRbac>,
    mut registry: ResMut<SessionRegistry>,
) {
    // Run on the authoritative peer (Host or single-player Standalone), never a
    // Client — ownership is the host's to assign. `is_host()` alone would skip the
    // idle single-player sandbox, which is `Standalone`.
    if matches!(*role, NetworkRole::Client) {
        return;
    }
    for mut ap in &mut q {
        // Register the autopilot exactly like a connecting user: authenticated, with
        // a host-issued token so `SessionRbac::is_authorized` treats it as trusted.
        rbac.sessions.entry(ap.session.0).or_insert_with(|| UserSession {
            session_id: ap.session,
            username: format!("autopilot-{}", ap.session.0),
            role: AuthorityRole::AiAgent,
            authenticated: true,
            token: Some("autopilot-local".to_string()),
        });
        let Ok(gid) = q_gid.get(ap.vessel) else {
            warn!("[autopilot] vessel {:?} has no GlobalEntityId; cannot engage", ap.vessel);
            ap.engaged = false;
            continue;
        };
        match registry.claim(ap.session, gid.get()) {
            Ok(()) => {
                ap.engaged = true;
                info!("[autopilot] session {} engaged, owns entity {}", ap.session, gid.get());
            }
            Err(cur) => {
                ap.engaged = false;
                warn!(
                    "[autopilot] session {} could not claim entity {} (owned by {cur}); staying disengaged",
                    ap.session,
                    gid.get()
                );
            }
        }
    }
}

/// Fixed-tick driver: every engaged autopilot that still **owns** its vessel emits
/// one `SetPorts` — the same command the keyboard and scripts use. Losing ownership
/// (another actor took the vessel) makes `owns` false, so the autopilot stops
/// writing with no disengage polling. This is the autopilot's self-gate; the human
/// keyboard has the symmetric one in `drive_from_bindings`.
pub fn drive_autopilots(
    registry: Res<SessionRegistry>,
    q: Query<&Autopilot>,
    q_gid: Query<&GlobalEntityId>,
    mut commands: Commands,
) {
    for ap in &q {
        if !ap.engaged {
            continue;
        }
        let Ok(gid) = q_gid.get(ap.vessel) else { continue };
        if !registry.owns(ap.session, gid.get()) {
            continue; // lost the vessel → stop driving (one writer per tick)
        }
        commands.trigger(SetPorts {
            target: ap.vessel,
            writes: vec![
                ("throttle".to_string(), ap.throttle),
                ("steer".to_string(), ap.steer),
                ("brake".to_string(), 0.0),
            ],
            seq: 0,
            tick: 0,
        });
    }
}

/// Headless-safe plugin: engage autopilots on spawn and drive them each fixed tick.
/// Adds no rendering/UI/avatar dependency, so a `--no-ui` server runs autopilots
/// identically to the GUI.
pub struct AutopilotPlugin;

impl Plugin for AutopilotPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, setup_autopilot_session);
        app.add_systems(FixedUpdate, drive_autopilots);
    }
}

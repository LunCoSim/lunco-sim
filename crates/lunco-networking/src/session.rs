//! Wire-fed session state — the netcode-only half of what used to live in
//! `lunco-core/src/session.rs` (Phase 6, review C7).
//!
//! ## Why this lives here and not in `lunco-core`
//!
//! The boundary rule for `lunco-core/src/session.rs` is: a type stays in core
//! only if a crate that does NOT depend on `lunco-networking` consumes it
//! (the always-on D7 seam — `NetworkRole`, `NetStatus`, `SessionRegistry`,
//! the input logs the controller writes, the replication markers usd-sim
//! stamps). Every type in THIS module is produced and consumed exclusively by
//! this crate — the wire snapshot sample, the deep-link confirm gate, the
//! prediction contact gate, the desync gauge, the reconcile residual — so
//! keeping them in core was accretion, not substrate. Resources here are
//! initialized by the plugins whose systems read them ([`crate::sync::SyncPlugin`],
//! [`crate::prediction::NetcodePredictionPlugin`]), not by `LunCoCorePlugin`.

use bevy::prelude::*;
use std::collections::HashMap;

/// A connect request that arrived from an **untrusted deep link** (a clicked
/// `luncosim://connect?…` link, or the web `?connect=…#digest`) and is awaiting
/// the user's confirmation. Unlike the menu's
/// [`NetConnectRequest`](lunco_core::NetConnectRequest) (an explicit
/// in-app click), a link could be planted by a third party to silently redirect
/// the session, so the UI shows a "Connect to X? [Join] [Cancel]" prompt while
/// this is `Some`; only on *Join* does it become a `JoinServer`. The networking
/// adapter seeds it (native arg parse / wasm URL); the UI clears it on either
/// choice. Both the seeder and the confirm modal live in this crate
/// (`client` / `single_instance` / `ui`), so unlike [`lunco_core::NetStatus`]
/// this is not an always-on seam.
#[derive(Resource, Clone, Debug, Default)]
pub struct PendingConnect {
    /// The pending link, or `None` when nothing awaits confirmation.
    pub request: Option<PendingConnectRequest>,
}

/// The address + optional cert digest a [`PendingConnect`] is asking to dial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConnectRequest {
    /// `host:port` — hostname or `ip:port`.
    pub address: String,
    /// Self-signed cert digest to pin, or empty for CA validation.
    pub digest: String,
}

/// **Contact-prediction eligibility:** this non-owned replicated body *may* be
/// promoted to a locally-`Dynamic` [`lunco_core::PredictedDynamic`] body, but
/// **only while an [`lunco_core::OwnedLocally`] body is actually touching it**
/// (`promote_contacting_proxies`).
/// The rest of the time it stays a kinematic snapshot proxy — perfectly synced to
/// authority, no drift.
///
/// Why this exists (the fix for the "predict-all → drift then chaos" bug): the
/// earlier design flipped *every* non-owned rover/prop to `PredictedDynamic` the
/// moment it was seen, so N mutually-colliding Dynamic bodies free-ran local
/// physics reconciled only against a ~0.18 s-stale curve — the solver pushed the
/// pile apart faster than the bounded correction could pull it back → chaos.
/// Kinematic proxies (pose forced by snapshots) provably cannot drift; the ONLY
/// reason to make a body Dynamic is so it *yields* when your owned rover shoves
/// it. So we gate that Dynamic window to the exact interval a shove is happening,
/// against exactly one pusher — the stable regime. On promotion the body gains
/// `PredictedDynamic` (every proxy-driving seam already excludes that marker); on
/// contact-end it loses it and `drive_kinematic_proxies` re-seats it on the
/// authoritative curve.
///
/// Stamped by `maintain_predicted_dynamic` (free props) and
/// `maintain_predicted_vehicles` (remote raycast rovers) on the same eligible set
/// they used to promote outright — cosim/opaque ([`lunco_core::NotPredictable`]),
/// articulated ([`lunco_core::ArticulatedVehicle`], which flips if made Dynamic),
/// owned, and static bodies are all excluded there. Removed when this peer
/// possesses the body (the input-replay [`lunco_core::OwnedLocally`] path takes
/// over). Client-only; stamped and read only by this crate's prediction systems.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ContactPredictable;

/// One replicated transform sample to apply on a client, keyed by
/// [`lunco_core::GlobalEntityId`] raw `u64`. Pushed by the wire layer; applied by
/// this crate's snapshot ingest/interpolation (which sets avian `Position` so the
/// physics sync doesn't overwrite it).
#[derive(Clone, Copy, Debug)]
pub struct SnapshotSample {
    pub gid: u64,
    /// Host `SimTick` this batch was generated at (60 Hz). The client interpolates
    /// in this tick-derived timebase rather than local receipt time, so bursty /
    /// render-throttled delivery (several 20 Hz snapshots arriving in one frame,
    /// e.g. when the sending peer's window is unfocused) still reconstructs smooth
    /// motion instead of collapsing to one effective sample per burst.
    pub tick: u64,
    pub t: [f32; 3],
    pub r: [f32; 4],
    /// Authoritative linear velocity (avian `LinearVelocity`, f64→f32). Used by
    /// the owned-rover prediction to seat the body for replay; remote bodies
    /// ignore it.
    pub lv: [f32; 3],
    /// Authoritative angular velocity (avian `AngularVelocity`, f64→f32).
    pub av: [f32; 3],
    /// The highest input `seq` the host has applied for this gid (0 = none). The
    /// owning client uses it to drop acked inputs and replay the rest.
    pub last_input_seq: u32,
    /// Authoritative **absolute** position from avian f64 `Position` (gap A). `t`
    /// above is the f32 render-space offset; `pos` is the precise physics truth
    /// the proxy apply seats `Position` from, so lunar/orbital-scale bodies don't
    /// lose precision to f32. Falls back to `t` when the host had no `Position`.
    pub pos: [f64; 3],
    /// big_space `CellCoord` (i64/axis). `[0,0,0]` in the current single-cell
    /// config; carried so replication stays correct once recentering is enabled.
    pub cell: [i64; 3],
}

/// Inbound transform samples awaiting application on a client.
#[derive(Resource, Default)]
pub struct IncomingSnapshots(pub Vec<SnapshotSample>);

/// Which prediction path a divergence sample came from (see [`DivergenceStats`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PredictionKind {
    /// The body this peer owns and drives (`OwnedLocally`) — error measured at the
    /// **acked input seq**, so the client's legitimate latency lead cancels.
    #[default]
    Owned,
    /// A free locally-simulated body (`PredictedDynamic`: props, bumped rocks,
    /// remote rovers under the contact gate) — error measured against the delayed
    /// authoritative curve it is being reconciled onto.
    Free,
}

/// Per-body divergence gauge for one gid.
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyDivergence {
    pub kind: PredictionKind,
    /// Most recent |authority − prediction| (m).
    pub last_m: f32,
    /// Worst value seen since the body started being predicted.
    pub max_m: f32,
    /// Consecutive samples above [`DivergenceStats::warn_m`].
    pub over_streak: u32,
    /// How many times this body was force-rebaselined (hard snap to authority).
    pub rebaselines: u32,
}

/// CLIENT-side desync detection (review N3).
///
/// Before this there was **no way to observe a desync in the field**: the only
/// backstop was a *silent* per-body snap, and the owned-body half of it could be
/// permanently disabled by the stale-ack bug (see
/// [`lunco_core::AppliedInputSeq`]). A client
/// could drift indefinitely and nothing said so — not a log line, not a counter.
///
/// Every client-side reconcile feeds a sample here, so each locally-simulated body
/// carries a live error, a running max, and a rebaseline count. Sustained error
/// past [`Self::warn_m`] is logged once per body per streak — the "I diverged"
/// signal — and the existing snap/teleport paths count themselves as rebaselines.
///
/// **Deliberately not a wire state-hash.** A rolling digest of the host's pose set
/// would tell the client only what each snapshot already tells it, body by body,
/// with authority attached — and a client cannot recompute a host digest for
/// *interpolated* bodies at all (it holds no tick-aligned local state for them; it
/// holds the host's). Comparing local simulation against received authority is the
/// same test, cheaper, and it names the body that diverged. Empty on host/standalone.
#[derive(Resource, Debug)]
pub struct DivergenceStats {
    /// gid → gauge.
    pub bodies: HashMap<u64, BodyDivergence>,
    /// Error (m) above which a sample counts as divergence.
    pub warn_m: f32,
    /// Consecutive over-threshold samples before the client says so out loud.
    pub warn_streak: u32,
}

impl Default for DivergenceStats {
    fn default() -> Self {
        // 1.0 m is comfortably above the reconcile dead-zones (0.40 m) and the
        // measured free-driving prediction error (~13–27 cm per 20 Hz ack), and well
        // below the 6 m gross-desync snap — so a sustained metre of error is real
        // divergence, not noise, and it is reported BEFORE the snap papers over it.
        Self {
            bodies: HashMap::new(),
            warn_m: 1.0,
            warn_streak: 5,
        }
    }
}

impl DivergenceStats {
    /// Record one |authority − prediction| sample for `gid`. Returns `true` exactly
    /// on the sample where the body crosses into a sustained divergence (so the
    /// caller logs once per streak, not once per tick).
    pub fn observe(&mut self, gid: u64, kind: PredictionKind, err_m: f32) -> bool {
        let warn_m = self.warn_m;
        let warn_streak = self.warn_streak;
        let b = self.bodies.entry(gid).or_default();
        b.kind = kind;
        b.last_m = err_m;
        b.max_m = b.max_m.max(err_m);
        if err_m > warn_m {
            b.over_streak += 1;
            b.over_streak == warn_streak
        } else {
            b.over_streak = 0;
            false
        }
    }

    /// Note that `gid` was force-rebaselined (hard snap / teleport to authority).
    pub fn note_rebaseline(&mut self, gid: u64) {
        let b = self.bodies.entry(gid).or_default();
        b.rebaselines += 1;
        b.over_streak = 0;
    }

    /// The worst live divergence `(gid, metres)` — the gauge the diagnostics export.
    pub fn worst(&self) -> Option<(u64, f32)> {
        self.bodies
            .iter()
            .max_by(|a, b| a.1.last_m.total_cmp(&b.1.last_m))
            .map(|(&g, b)| (g, b.last_m))
    }

    /// Forget a body (despawn / no longer predicted).
    pub fn forget(&mut self, gid: u64) {
        self.bodies.remove(&gid);
    }
}

/// A reconciliation residual parked on a predicted body, drained a little per
/// fixed tick in **physics space** (`Position`/`Rotation`), never on `Transform`.
///
/// **Why not `Transform`.** The sandbox runs
/// `PhysicsInterpolationPlugin::interpolate_all()`, so `bevy_transform_interpolation`
/// owns every body's `Transform` at render rate and treats ANY external `Transform`
/// write as a teleport — resetting its easing. An offset written there therefore
/// *disabled* interpolation for the corrected body (≈ continuously while driving)
/// and the rover rendered at raw fixed-tick steps: the "jitters while just holding
/// the key" the host never showed. Parking the residual here and nudging
/// `Position`/`Rotation` lets avian writeback + interpolation render it smoothly
/// with no second writer anywhere.
///
/// It once lived in `lunco-core` (review A6) because `lunco-luncosim-edit` owned
/// the producer/drain systems; those now live in this crate's `prediction`, so
/// the type followed them here (review C7) — this crate is its only consumer.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PendingCorrection {
    /// Remaining position delta to apply (world metres).
    pub pos: Vec3,
    /// Remaining orientation delta (applied as `rot * current`).
    pub rot: Quat,
}

impl PendingCorrection {
    /// Residual small enough to drop the component (the drain is done).
    pub fn is_negligible(&self) -> bool {
        self.pos.length_squared() < 1e-8 && self.rot.angle_between(Quat::IDENTITY) < 1e-4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R1: u64 = 0xA1;

    // ── Desync gauge (review N3) ──────────────────────────────────────────────

    /// A body that keeps diverging past the threshold raises the "I diverged" signal
    /// exactly ONCE per streak (so the caller logs, rather than spamming per tick),
    /// and a body that returns to tolerance resets — with the max preserved.
    #[test]
    fn divergence_gauge_signals_once_per_sustained_streak() {
        let mut stats = DivergenceStats::default();
        let over = stats.warn_m + 0.5;
        let mut signals = 0;
        for _ in 0..(stats.warn_streak * 3) {
            if stats.observe(R1, PredictionKind::Owned, over) {
                signals += 1;
            }
        }
        assert_eq!(
            signals, 1,
            "one signal per sustained divergence, not one per tick"
        );
        assert_eq!(stats.worst(), Some((R1, over)));

        // Back in tolerance → streak resets, max is remembered.
        assert!(!stats.observe(R1, PredictionKind::Owned, 0.01));
        assert_eq!(stats.bodies[&R1].over_streak, 0);
        assert_eq!(stats.bodies[&R1].max_m, over);
        // A hard snap to authority is a rebaseline, and it is counted (it used to be
        // entirely silent — the netcode's loudest symptom, invisible in the field).
        stats.note_rebaseline(R1);
        assert_eq!(stats.bodies[&R1].rebaselines, 1);
    }
}

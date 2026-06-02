//! Pure predict-own reconciliation decision (D2, input-replay model).
//!
//! The owned, locally-predicted body records its post-step pose each tick keyed
//! by the input `seq` it applied. When a server snapshot acks a `seq`, we compare
//! *what we predicted at that seq* against the authoritative state **at the same
//! seq** — apples-to-apples, so the client's legitimate latency lead (it has
//! applied more inputs since the ack) cancels and never triggers a backward tug.
//! That is the whole reason this beats continuous snapshot-blending, which pulls
//! the present toward a stale past and rubber-bands.
//!
//! This module is the *pure geometry* of that comparison. The ECS system in the
//! spawn domain (`reconcile_owned_prediction`) feeds it predicted / current /
//! authoritative poses and applies the result (plus velocity seating). Kept here,
//! dependency-free, so the decision logic is unit-tested without the heavy
//! avian/render build.

use bevy::math::{Quat, Vec3};

/// Tunables for the reconciliation decision. [`Default`] is the single source of
/// truth for the thresholds the live system uses.
#[derive(Clone, Copy, Debug)]
pub struct ReconcileParams {
    /// Below this positional error (m) between prediction-at-the-acked-seq and
    /// authority, the prediction is "correct" and we do **nothing** — no pull,
    /// no rubber-band. Only genuine divergence past this is reconciled.
    pub eps_pos: f32,
    /// Angular tolerance (rad, ≈1.7°) for the same "prediction correct" test.
    pub eps_rot: f32,
    /// Beyond this positional divergence (m) the prediction has grossly desynced
    /// (teleport / respawn / long stall) — hard-snap fully to authority.
    pub snap_pos: f32,
    /// Fraction of a (non-snap) pose error applied to the present per ack; the
    /// rest eases in over subsequent acks so a rare correction slides rather than
    /// jumps. Velocity is seated fully by the caller regardless, so the residual
    /// converges in ~3–4 acks.
    pub blend: f32,
}

impl Default for ReconcileParams {
    fn default() -> Self {
        Self {
            eps_pos: 0.25,
            eps_rot: 0.03,
            snap_pos: 6.0,
            blend: 0.3,
        }
    }
}

/// The reconciliation decision for one acked input `seq`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reconciliation {
    /// Prediction matched authority within tolerance — leave the body untouched.
    InSync,
    /// Apply this corrected present pose (a blended nudge toward authority); the
    /// caller also seats velocity to authoritative so the body stops re-diverging.
    Correct { pos: Vec3, rot: Quat },
    /// Gross desync — hard-snap the present fully to authority.
    Snap { pos: Vec3, rot: Quat },
}

/// Decide how to reconcile, comparing the **prediction at the acked seq**
/// (`predicted_*`) against the **authoritative state at that same seq**
/// (`auth_*`), then expressing any correction relative to the body's **present**
/// pose (`current_*`).
///
/// Comparing at the same seq is load-bearing: a correct prediction yields a
/// near-zero error even though `current` has advanced well past `auth` (the
/// latency lead), so [`Reconciliation::InSync`] is returned and the body is never
/// tugged backward. The error measured at the acked seq is applied to the present
/// because, over the ~3–6 unacked ticks, the error at the ack ≈ the error now.
pub fn reconcile_decision(
    predicted_pos: Vec3,
    predicted_rot: Quat,
    current_pos: Vec3,
    current_rot: Quat,
    auth_pos: Vec3,
    auth_rot: Quat,
    p: ReconcileParams,
) -> Reconciliation {
    let err_pos = auth_pos - predicted_pos;
    let dist = err_pos.length();
    let mut err_rot = auth_rot * predicted_rot.inverse();
    if err_rot.w < 0.0 {
        err_rot = -err_rot; // shortest arc
    }
    let angle = err_rot.to_axis_angle().1.abs();

    // COMMON CASE: prediction matched authority → leave the body alone.
    if dist < p.eps_pos && angle < p.eps_rot {
        return Reconciliation::InSync;
    }
    // GROSS DESYNC: hard-snap the present fully to authority.
    if dist > p.snap_pos {
        return Reconciliation::Snap {
            pos: auth_pos,
            rot: auth_rot,
        };
    }
    // MISPREDICTION: ease the divergence into the present over a few acks.
    Reconciliation::Correct {
        pos: current_pos + err_pos * p.blend,
        rot: (Quat::IDENTITY.slerp(err_rot, p.blend) * current_rot).normalize(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> ReconcileParams {
        ReconcileParams::default()
    }

    /// THE no-rubber-band guarantee: when the prediction at the acked seq matches
    /// authority, the body is left alone **regardless** of how far the present
    /// pose has advanced since the ack. A correct prediction is never pulled back
    /// to a stale snapshot — this is exactly what continuous blending got wrong.
    #[test]
    fn in_sync_even_with_a_large_latency_lead() {
        let predicted = Vec3::new(10.0, 0.0, 0.0);
        let auth = Vec3::new(10.01, 0.0, 0.0); // within eps_pos of prediction
        let current = Vec3::new(25.0, 0.0, 0.0); // moved 15 m ahead since the ack
        let r = reconcile_decision(
            predicted,
            Quat::IDENTITY,
            current,
            Quat::IDENTITY,
            auth,
            Quat::IDENTITY,
            p(),
        );
        assert_eq!(r, Reconciliation::InSync);
    }

    /// A genuine small mispredict nudges the present by `blend * err` — it does
    /// NOT teleport to authority (that would be visible as a jump).
    #[test]
    fn corrects_small_mispredict_without_teleport() {
        let predicted = Vec3::ZERO;
        let auth = Vec3::new(1.0, 0.0, 0.0); // 1 m error: > eps_pos, < snap_pos
        let current = Vec3::new(5.0, 0.0, 0.0);
        match reconcile_decision(
            predicted,
            Quat::IDENTITY,
            current,
            Quat::IDENTITY,
            auth,
            Quat::IDENTITY,
            p(),
        ) {
            Reconciliation::Correct { pos, .. } => {
                // present (5.0) nudged by blend*err = 0.3 → 5.3, not snapped to 1.0
                assert!((pos.x - 5.3).abs() < 1e-5, "got {pos:?}");
            }
            other => panic!("expected Correct, got {other:?}"),
        }
    }

    /// Past `snap_pos` the prediction is hopeless (teleport/respawn/long stall) →
    /// hard-snap fully to authority.
    #[test]
    fn snaps_on_gross_desync() {
        let r = reconcile_decision(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(7.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::new(50.0, 0.0, 0.0), // >> snap_pos
            Quat::IDENTITY,
            p(),
        );
        assert_eq!(
            r,
            Reconciliation::Snap {
                pos: Vec3::new(50.0, 0.0, 0.0),
                rot: Quat::IDENTITY
            }
        );
    }

    /// As the prediction converges toward authority across acks, the correction
    /// shrinks and eventually returns [`Reconciliation::InSync`] — it does not
    /// oscillate.
    #[test]
    fn correction_converges_to_in_sync() {
        let auth = Vec3::new(2.0, 0.0, 0.0);
        let current = Vec3::new(10.0, 0.0, 0.0);
        let big = reconcile_decision(
            Vec3::ZERO,
            Quat::IDENTITY,
            current,
            Quat::IDENTITY,
            auth,
            Quat::IDENTITY,
            p(),
        );
        assert!(matches!(big, Reconciliation::Correct { .. }));
        // once the prediction lands within eps of auth → InSync
        let converged = reconcile_decision(
            Vec3::new(1.9, 0.0, 0.0),
            Quat::IDENTITY,
            current,
            Quat::IDENTITY,
            auth,
            Quat::IDENTITY,
            p(),
        );
        assert_eq!(converged, Reconciliation::InSync);
    }

    /// A rotational mispredict corrects along the shortest arc, partially (by
    /// `blend`), toward authority.
    #[test]
    fn rotation_correction_takes_shortest_arc() {
        let auth_rot = Quat::from_rotation_y(0.2); // > eps_rot
        match reconcile_decision(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::ZERO,
            auth_rot,
            p(),
        ) {
            Reconciliation::Correct { rot, .. } => {
                // blended toward auth by `blend` → angle ≈ 0.3 * 0.2, never past auth
                let ang = rot.to_axis_angle().1.abs();
                assert!(ang > 0.0 && ang < 0.2, "got angle {ang}");
            }
            other => panic!("expected Correct, got {other:?}"),
        }
    }
}

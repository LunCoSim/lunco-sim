//! Render-error smoothing (`PREDICT_AND_SMOOTH` Step 2): "smooth the pop, not the
//! truth".
//!
//! When client-side reconciliation corrects a predicted body, the *physics* pose
//! snaps to authority immediately (so contact/collision stays correct), but that
//! snap is visible as a jitter/pop. [`RenderErrorOffset`] records the visual
//! delta of each correction and **decays it to zero over a short time-const**, so
//! the body *renders* where it was and slides smoothly onto truth instead of
//! teleporting. Physics is always at the authoritative pose; only the render
//! `Transform` carries the (shrinking) offset.
//!
//! Always-on substrate (no networking feature gate). The apply/strip systems that
//! project the offset onto `Transform` live in the spawn/render crate; this module
//! is just the data + the pure math, unit-tested without avian/bevy-render.

use bevy::prelude::*;

/// Exponential ease time-constant (seconds): the visual offset shrinks by `1/e`
/// every `SMOOTH_TIME_CONST`. Originally 0.05 s, but measured owned-rover
/// corrections land every ~1–2 s (host-vs-client input-timing skew, D2
/// no-rollback) at ~0.15 m / ~2.5° each — a 50 ms decay replays each one as a
/// visible twitch. 0.18 s spreads the correction across ~11 frames (sub-
/// perceptible slide) while still converging well before the next one lands.
pub const SMOOTH_TIME_CONST: f32 = 0.18;

/// If a single correction would push the visual offset past this (metres), don't
/// smooth it — let the body render at truth. A pop this large means the prediction
/// was grossly wrong (teleport / long stall); hiding it would be a long, obviously
/// laggy slide, worse than just showing the corrected pose.
pub const MAX_VISUAL_OFFSET_POS: f32 = 3.0;

/// Rotational counterpart of [`MAX_VISUAL_OFFSET_POS`] (degrees).
pub const MAX_VISUAL_OFFSET_ROT_DEG: f32 = 30.0;

/// Per-body visual offset between where the body *renders* and where its *physics*
/// actually is. Identity (`pos = 0`, `rot = IDENTITY`) means render == physics.
/// Reconcilers add the just-applied correction here (so the render doesn't jump);
/// `decay` shrinks it toward identity each frame.
///
/// Composition convention: `render_pos = physics_pos + offset.pos` and
/// `render_rot = offset.rot * physics_rot` (offset is the *residual* error the
/// render still shows on top of the corrected physics pose).
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component)]
pub struct RenderErrorOffset {
    pub pos: Vec3,
    pub rot: Quat,
}

impl Default for RenderErrorOffset {
    fn default() -> Self {
        Self { pos: Vec3::ZERO, rot: Quat::IDENTITY }
    }
}

impl RenderErrorOffset {
    /// True when the offset is effectively identity (render == physics), so the
    /// apply/strip systems can skip it and it could be removed.
    pub fn is_negligible(&self) -> bool {
        self.pos.length_squared() < 1e-8 && self.rot.angle_between(Quat::IDENTITY) < 1e-4
    }

    /// Fold a freshly-applied correction into the offset so the render does NOT
    /// jump: the body's render pose stays at `old_render_*` while physics is now at
    /// `new_phys_*`. `pos` accumulates the positional gap; `rot` pre-composes the
    /// orientation gap. Accumulates (`+=` / pre-multiply) because several
    /// corrections can land before the previous offset has fully decayed.
    pub fn add_correction(
        &mut self,
        old_render_pos: Vec3,
        new_phys_pos: Vec3,
        old_render_rot: Quat,
        new_phys_rot: Quat,
    ) {
        self.pos += old_render_pos - new_phys_pos;
        // residual rotation that maps the new physics orientation back to where we
        // were rendering: old_render = rot_gap * new_phys  ⇒  rot_gap = old * new⁻¹
        let rot_gap = old_render_rot * new_phys_rot.inverse();
        self.rot = rot_gap * self.rot;
        self.clamp_to_max();
    }

    /// Snap the offset to identity if it exceeds the max visual offset (don't
    /// smooth a gross correction — show truth instead).
    pub fn clamp_to_max(&mut self) {
        if self.pos.length() > MAX_VISUAL_OFFSET_POS
            || self.rot.angle_between(Quat::IDENTITY).to_degrees() > MAX_VISUAL_OFFSET_ROT_DEG
        {
            self.pos = Vec3::ZERO;
            self.rot = Quat::IDENTITY;
        }
    }

    /// Ease the offset toward identity by `dt` seconds (exponential, time-const
    /// [`SMOOTH_TIME_CONST`]). Clean-snaps to identity once negligible so it stops
    /// dirtying change-detection / can be removed.
    pub fn decay(&mut self, dt: f32) {
        let remaining = (-dt / SMOOTH_TIME_CONST).exp().clamp(0.0, 1.0); // fraction left
        self.pos *= remaining;
        // `remaining` of the way from identity toward the current offset rotation.
        self.rot = Quat::IDENTITY.slerp(self.rot, remaining);
        if self.is_negligible() {
            self.pos = Vec3::ZERO;
            self.rot = Quat::IDENTITY;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_shrinks_toward_identity() {
        let mut o = RenderErrorOffset { pos: Vec3::new(1.0, 0.0, 0.0), rot: Quat::IDENTITY };
        let first = o.pos.x;
        o.decay(SMOOTH_TIME_CONST); // one time-constant ⇒ ~1/e left
        assert!(o.pos.x < first * 0.5, "should shrink by >half over a time-const; got {}", o.pos.x);
        assert!((o.pos.x - first / std::f32::consts::E).abs() < 1e-3, "≈1/e: {}", o.pos.x);
        // Many steps → negligible → clean zero.
        for _ in 0..100 {
            o.decay(SMOOTH_TIME_CONST);
        }
        assert_eq!(o.pos, Vec3::ZERO);
    }

    #[test]
    fn add_correction_hides_the_pop() {
        // Body rendered at x=5, physics corrected to x=4: offset should make render
        // stay at 5 (offset.pos.x = +1) so render_pos = phys + offset = 4 + 1 = 5.
        let mut o = RenderErrorOffset::default();
        o.add_correction(Vec3::new(5.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0), Quat::IDENTITY, Quat::IDENTITY);
        assert!((o.pos.x - 1.0).abs() < 1e-6, "offset should be +1; got {}", o.pos.x);
    }

    #[test]
    fn gross_correction_snaps_to_identity() {
        let mut o = RenderErrorOffset::default();
        // 10 m pop ≫ MAX_VISUAL_OFFSET_POS ⇒ don't smooth.
        o.add_correction(Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO, Quat::IDENTITY, Quat::IDENTITY);
        assert_eq!(o.pos, Vec3::ZERO, "gross pop must not be smoothed");
    }

    #[test]
    fn rotation_offset_composes_and_decays() {
        let mut o = RenderErrorOffset::default();
        let old = Quat::from_rotation_y(0.2);
        let new = Quat::IDENTITY;
        o.add_correction(Vec3::ZERO, Vec3::ZERO, old, new);
        // render_rot = offset.rot * new = offset.rot should ≈ old.
        assert!(o.rot.angle_between(old) < 1e-4, "rot offset should be ≈old");
        for _ in 0..200 {
            o.decay(SMOOTH_TIME_CONST);
        }
        assert!(o.rot.angle_between(Quat::IDENTITY) < 1e-3, "rot offset should decay to identity");
    }
}

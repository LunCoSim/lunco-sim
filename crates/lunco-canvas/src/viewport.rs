//! World↔screen transforms and camera state for the canvas.
//!
//! The [`Viewport`] owns a world-space anchor (`center`) and a scalar
//! `zoom`. Everything the user sees in the canvas is the result of a
//! [`Viewport::world_to_screen`] call; every mouse event becomes world
//! state via [`Viewport::screen_to_world`]. Two calls, same transform,
//! inverse of each other — a good round-trip test (see the bottom of
//! this file) is the simplest bug-catcher for pan/zoom regressions.
//!
//! # Smooth pan/zoom
//!
//! The viewport carries a `target_center` and `target_zoom` alongside
//! the live values. [`Viewport::tick`] eases current toward target
//! each frame. Immediate jumps use [`Viewport::snap_to`] (for one-off
//! resets); any UI-driven motion (`fit_to`, `zoom_to_selection`, etc.)
//! sets the target and lets the tick handle it. This is what lets
//! "press F to fit" animate smoothly without any extra machinery on
//! the caller side.
//!
//! Keeping the easing *inside* the viewport means every tool gets it
//! for free — if you pan by updating `center`, it's instant (usually
//! what you want during a drag); if you pan by `set_target`, it's
//! smooth (what you want for programmatic moves).
//!
//! # Zoom pivot
//!
//! Scrolling should keep the point under the cursor fixed, not the
//! world origin. [`Viewport::zoom_at`] adjusts `center` so that the
//! given screen point maps to the same world point before and after
//! the zoom change. Any zoom-via-input funnels through here.

use serde::{Deserialize, Serialize};

use crate::scene::{Pos, Rect};

/// The camera.
///
/// Coordinates: world units are whatever the scene picks; screen
/// units are egui points (pre-DPI, post-widget-scale). The
/// `screen_rect` passed into transforms is the *widget* rect the
/// canvas is painting into — NOT the window rect — so egui_dock's
/// varying panel sizes are naturally handled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Viewport {
    /// World-space point rendered at the centre of the widget rect.
    pub center: Pos,
    /// Screen pixels per world unit. `1.0` = world units == screen
    /// points.
    pub zoom: f32,

    /// Interpolation target for `center`. When `target_center !=
    /// center`, `tick` eases toward it. Set these to trigger smooth
    /// motion; set them together with `center` to skip the animation.
    pub target_center: Pos,
    /// Interpolation target for `zoom`.
    pub target_zoom: f32,

    pub config: ViewportConfig,
}

/// Tuning knobs for the viewport — clamp range, animation speed,
/// scroll gain. Exposed as a field on [`Viewport`] so apps can set
/// a reasonable default for their scene scale without patching code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportConfig {
    /// Minimum allowed zoom. A scene with big world coordinates
    /// (metres) needs a much smaller min than one in pixel units.
    pub zoom_min: f32,
    pub zoom_max: f32,
    /// Exponential smoothing factor per frame (0..=1). `1.0` = snap
    /// immediately (no animation), `0.15` = visibly smooth. Frame-
    /// rate-independent via `tick(dt)` below.
    pub ease: f32,
    /// Multiplier applied to raw scroll delta when converting to a
    /// zoom factor. Tuned to feel "right" with a typical mouse wheel.
    pub scroll_zoom_gain: f32,
}

impl Default for ViewportConfig {
    fn default() -> Self {
        Self {
            zoom_min: 0.1,
            zoom_max: 8.0,
            ease: 0.25,
            scroll_zoom_gain: 0.0015,
        }
    }
}

impl Default for Viewport {
    fn default() -> Self {
        let center = Pos::new(0.0, 0.0);
        Self {
            center,
            zoom: 1.0,
            target_center: center,
            target_zoom: 1.0,
            config: ViewportConfig::default(),
        }
    }
}

impl Viewport {
    /// Transform a world-space point into widget-local screen points.
    ///
    /// `screen_rect` is the rect the canvas is painting into; we map
    /// its centre to `self.center` and scale by `self.zoom`.
    pub fn world_to_screen(&self, p: Pos, screen_rect: Rect) -> Pos {
        let mid_x = (screen_rect.min.x + screen_rect.max.x) * 0.5;
        let mid_y = (screen_rect.min.y + screen_rect.max.y) * 0.5;
        Pos::new(
            mid_x + (p.x - self.center.x) * self.zoom,
            mid_y + (p.y - self.center.y) * self.zoom,
        )
    }

    /// Inverse of [`Self::world_to_screen`]. Used on every mouse
    /// event so tools can reason in world units.
    pub fn screen_to_world(&self, p: Pos, screen_rect: Rect) -> Pos {
        let mid_x = (screen_rect.min.x + screen_rect.max.x) * 0.5;
        let mid_y = (screen_rect.min.y + screen_rect.max.y) * 0.5;
        // Zoom can't be zero (clamped in setters), so no guard.
        Pos::new(
            self.center.x + (p.x - mid_x) / self.zoom,
            self.center.y + (p.y - mid_y) / self.zoom,
        )
    }

    /// Map a world-space rect to a screen-space rect.
    pub fn world_rect_to_screen(&self, r: Rect, screen_rect: Rect) -> Rect {
        Rect {
            min: self.world_to_screen(r.min, screen_rect),
            max: self.world_to_screen(r.max, screen_rect),
        }
    }

    /// Snap both current and target to the same value — no animation.
    /// Use on explicit resets (load scene, "home" button).
    pub fn snap_to(&mut self, center: Pos, zoom: f32) {
        let zoom = self.clamp_zoom(zoom);
        self.center = center;
        self.zoom = zoom;
        self.target_center = center;
        self.target_zoom = zoom;
    }

    /// Set the animation target — `tick` will ease current toward it.
    /// Use for programmatic camera moves (fit, zoom-to-selection).
    pub fn set_target(&mut self, center: Pos, zoom: f32) {
        self.target_center = center;
        self.target_zoom = self.clamp_zoom(zoom);
    }

    /// Zoom in/out around a fixed **screen-space** pivot, keeping the
    /// world point under the pivot stationary. This is the correct
    /// behaviour for scroll-wheel zoom: the point under the cursor
    /// stays put, the rest of the scene scales around it. Without
    /// this adjustment the cursor would drift as zoom changed.
    pub fn zoom_at(&mut self, pivot_screen: Pos, factor: f32, screen_rect: Rect) {
        let new_zoom = self.clamp_zoom(self.zoom * factor);
        if (new_zoom - self.zoom).abs() < f32::EPSILON {
            return; // already at a clamp boundary
        }
        // World point under pivot BEFORE the zoom change — this is
        // the invariant we preserve.
        let world_pivot = self.screen_to_world(pivot_screen, screen_rect);
        // Apply the zoom change (instantly — this is driven by an
        // input event, not a programmatic target), then adjust
        // centre so the same world point maps back to the same
        // screen pivot.
        self.zoom = new_zoom;
        let mid_x = (screen_rect.min.x + screen_rect.max.x) * 0.5;
        let mid_y = (screen_rect.min.y + screen_rect.max.y) * 0.5;
        self.center = Pos::new(
            world_pivot.x - (pivot_screen.x - mid_x) / self.zoom,
            world_pivot.y - (pivot_screen.y - mid_y) / self.zoom,
        );
        // Keep targets in sync so we don't immediately drift back.
        self.target_center = self.center;
        self.target_zoom = self.zoom;
    }

    /// Advance animation one frame. `dt` is seconds since last tick;
    /// easing is frame-rate-independent via `1 - exp(-k·dt)` style
    /// smoothing. Returns `true` while animation is still in flight,
    /// so callers can force a repaint as long as it does.
    pub fn tick(&mut self, dt: f32) -> bool {
        // Convert config.ease (per-60fps-frame) to a per-second rate.
        // At dt = 1/60s, alpha ≈ config.ease; at larger dt it scales
        // correctly. The `min(1.0)` guards absurdly long frames.
        let k = -self.config.ease.ln_1p().abs(); // unused — branch kept for future tuning
        let _ = k;
        let alpha = 1.0 - (1.0 - self.config.ease).powf(dt * 60.0);
        let alpha = alpha.min(1.0);
        let moved = {
            let dx = self.target_center.x - self.center.x;
            let dy = self.target_center.y - self.center.y;
            let dz = self.target_zoom - self.zoom;
            let moved = dx.abs() > 0.01 || dy.abs() > 0.01 || dz.abs() > 0.001;
            self.center.x += dx * alpha;
            self.center.y += dy * alpha;
            self.zoom += dz * alpha;
            moved
        };
        moved
    }

    /// Pan by a world-space delta. Instant (no animation) — this is
    /// what a pan-drag tool calls each mouse-move.
    pub fn pan_by_world(&mut self, dx: f32, dy: f32) {
        self.center.x += dx;
        self.center.y += dy;
        self.target_center = self.center;
    }

    /// Compute the centre + zoom that makes `world_rect` fit inside
    /// `screen_rect` with `padding` screen-pixels of margin on every
    /// side, clamped to the zoom limits. Does NOT apply — returns
    /// values so callers can choose `snap_to` vs `set_target`.
    pub fn fit_values(&self, world_rect: Rect, screen_rect: Rect, padding: f32) -> (Pos, f32) {
        let w_world = world_rect.width().max(f32::EPSILON);
        let h_world = world_rect.height().max(f32::EPSILON);
        let w_screen = (screen_rect.width() - 2.0 * padding).max(1.0);
        let h_screen = (screen_rect.height() - 2.0 * padding).max(1.0);
        let fit = (w_screen / w_world).min(h_screen / h_world);
        (world_rect.center(), self.clamp_zoom(fit))
    }

    fn clamp_zoom(&self, z: f32) -> f32 {
        z.clamp(self.config.zoom_min, self.config.zoom_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rect {
        Rect::from_min_size(Pos::new(0.0, 0.0), 400.0, 300.0)
    }

    #[test]
    fn round_trip_identity_at_default_zoom() {
        // With zoom = 1 and centre = (0,0), screen centre is the
        // origin — the classic sanity check.
        let v = Viewport::default();
        let p = Pos::new(0.0, 0.0);
        let sp = v.world_to_screen(p, screen());
        assert_eq!(sp, Pos::new(200.0, 150.0));
        let back = v.screen_to_world(sp, screen());
        assert_eq!(back, p);
    }

    #[test]
    fn round_trip_survives_offset_and_zoom() {
        let mut v = Viewport::default();
        v.snap_to(Pos::new(50.0, -25.0), 2.5);
        // Pick a few world points; round-trip must land within
        // float-rounding distance of the original.
        for p in [
            Pos::new(0.0, 0.0),
            Pos::new(100.0, 200.0),
            Pos::new(-30.0, 40.0),
        ] {
            let sp = v.world_to_screen(p, screen());
            let back = v.screen_to_world(sp, screen());
            assert!((back.x - p.x).abs() < 1e-3, "x drift: {:?} -> {:?}", p, back);
            assert!((back.y - p.y).abs() < 1e-3, "y drift");
        }
    }

    #[test]
    fn zoom_at_keeps_world_point_under_cursor_fixed() {
        // The invariant: the world point under the pivot before the
        // zoom change must still be under the pivot after.
        let mut v = Viewport::default();
        let rect = screen();
        let pivot = Pos::new(320.0, 80.0); // not centre
        let world_before = v.screen_to_world(pivot, rect);
        v.zoom_at(pivot, 2.0, rect);
        let world_after = v.screen_to_world(pivot, rect);
        assert!(
            (world_after.x - world_before.x).abs() < 1e-3
                && (world_after.y - world_before.y).abs() < 1e-3,
            "pivot drifted: {:?} -> {:?}",
            world_before,
            world_after
        );
        // Actual zoom changed (we weren't already at max).
        assert!((v.zoom - 2.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_clamps_to_config_limits() {
        let mut v = Viewport::default();
        v.config.zoom_max = 4.0;
        let rect = screen();
        // Zoom way in beyond the limit — should saturate at 4.0.
        v.zoom_at(rect.center(), 100.0, rect);
        assert_eq!(v.zoom, 4.0);
        // And out below the min.
        v.zoom_at(rect.center(), 0.001, rect);
        assert_eq!(v.zoom, v.config.zoom_min);
    }

    #[test]
    fn snap_to_bypasses_animation() {
        let mut v = Viewport::default();
        v.snap_to(Pos::new(10.0, 20.0), 3.0);
        // Both current and target match immediately.
        assert_eq!(v.center, Pos::new(10.0, 20.0));
        assert_eq!(v.target_center, Pos::new(10.0, 20.0));
        assert_eq!(v.zoom, 3.0);
        assert_eq!(v.target_zoom, 3.0);
        // A tick should be a no-op.
        let moved = v.tick(1.0 / 60.0);
        assert!(!moved);
    }

    #[test]
    fn tick_advances_toward_target_without_overshooting() {
        let mut v = Viewport::default();
        v.set_target(Pos::new(100.0, 100.0), 1.0);
        // Run until settled; should monotonically approach (never
        // overshoot) — if it did, we'd have negative residuals.
        let mut last_dist = f32::INFINITY;
        for _ in 0..200 {
            v.tick(1.0 / 60.0);
            let dx = v.target_center.x - v.center.x;
            let dy = v.target_center.y - v.center.y;
            let dist = (dx * dx + dy * dy).sqrt();
            assert!(dist <= last_dist + 1e-3, "overshoot: {} -> {}", last_dist, dist);
            last_dist = dist;
        }
        assert!(last_dist < 1.0, "did not converge: {}", last_dist);
    }

    #[test]
    fn fit_values_covers_scene_with_margin() {
        let v = Viewport::default();
        let world = Rect::from_min_size(Pos::new(0.0, 0.0), 200.0, 100.0);
        let (centre, zoom) = v.fit_values(world, screen(), 20.0);
        assert_eq!(centre, Pos::new(100.0, 50.0));
        // screen_w - 2*padding = 360; world_w = 200 → fit_x = 1.8.
        // screen_h - 2*padding = 260; world_h = 100 → fit_y = 2.6.
        // min → 1.8. Clamped against zoom_max = 8.0, so 1.8 survives.
        assert!((zoom - 1.8).abs() < 1e-3, "zoom = {}", zoom);
    }
}

//! Forced OS-window placement (`--window-pos`).
//!
//! Snaps the primary window to a screen region so a host and a client
//! instance (or any two app instances) can sit side by side without
//! manual dragging. Specs: `left` / `right` (half the screen) or
//! `top-left` / `top-right` / `bottom-left` / `bottom-right` (a quarter).
//!
//! ## Platform reality: X11 only
//!
//! This works on **X11** (and XWayland), where a client may set its own
//! toplevel position. On **native Wayland it does not and cannot** — the
//! Wayland protocol deliberately forbids a client from placing its own
//! toplevel (compositor owns placement; there's also an input-interception
//! security concern). winit documents `set_outer_position` as unsupported
//! on Wayland, and the initial-position attribute is X11-only. COSMIC's own
//! `cosmic-toplevel-management-unstable-v1` protocol can't do it either: it
//! exposes only maximize / minimize / fullscreen / activate / sticky /
//! move-to-workspace — no set-position / set-geometry / half-tile.
//!
//! So on Wayland this is a **clean no-op with a one-line message**. The
//! native-Wayland ways to get a half-screen window are all user-side:
//! COSMIC auto-tiling (`Super+Y`) or focus-tiling (`Super+←/→`).
//!
//! TODO(wayland-window-pos): if we ever need the app to position itself on
//! Wayland anyway, the only option that actually works is forcing the
//! XWayland backend when `--window-pos` is passed — clear `WAYLAND_DISPLAY`
//! (and `WAYLAND_SOCKET`) at the very top of `main()` *before* the event
//! loop is built, so winit falls back to X11 via `DISPLAY`. Then this
//! module's runtime system positions the window normally. Downsides:
//! XWayland HiDPI/fractional-scale differences. Decision deferred (user
//! preferred keeping it Wayland-honest for now). See
//! `memory/project_window_placement_wayland.md` for the full investigation.

#[cfg(not(target_arch = "wasm32"))]
use bevy::prelude::*;

/// Where on the screen the `--window-pos` flag parks the OS window.
/// Halves (`LeftHalf`/`RightHalf`) span the full monitor height; quarters
/// take one corner. Fractions are resolved against the primary monitor's
/// physical bounds in [`apply_window_placement`].
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowPlacement {
    /// Left half of the monitor, spanning its full height.
    LeftHalf,
    /// Right half of the monitor, spanning its full height.
    RightHalf,
    /// Top-left quarter.
    TopLeft,
    /// Top-right quarter.
    TopRight,
    /// Bottom-left quarter.
    BottomLeft,
    /// Bottom-right quarter.
    BottomRight,
}

#[cfg(not(target_arch = "wasm32"))]
impl WindowPlacement {
    /// Parse a `--window-pos` spec. Accepts the long form (`top-left`),
    /// underscores (`top_left`), and short aliases (`l`, `r`, `tl`, `tr`,
    /// `bl`, `br`). Returns `None` for anything unrecognized.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "left" | "l" => Some(Self::LeftHalf),
            "right" | "r" => Some(Self::RightHalf),
            "top-left" | "tl" => Some(Self::TopLeft),
            "top-right" | "tr" => Some(Self::TopRight),
            "bottom-left" | "bl" => Some(Self::BottomLeft),
            "bottom-right" | "br" => Some(Self::BottomRight),
            _ => None,
        }
    }

    /// Scan argv for `--window-pos <spec>`. Logs a warning (and returns
    /// `None`) on an unrecognized spec; returns `None` when the flag is
    /// absent.
    pub fn from_args(args: &[String]) -> Option<Self> {
        for i in 0..args.len() {
            if args[i] == "--window-pos" && i + 1 < args.len() {
                let p = Self::parse(&args[i + 1]);
                if p.is_none() {
                    eprintln!(
                        "[window] unrecognized --window-pos '{}' (use left|right|top-left|top-right|bottom-left|bottom-right)",
                        args[i + 1]
                    );
                }
                return p;
            }
        }
        None
    }

    /// `(x_frac, y_frac, w_frac, h_frac)` of the monitor occupied by this
    /// placement, with the origin at the monitor's top-left.
    pub fn rect(self) -> (f64, f64, f64, f64) {
        match self {
            Self::LeftHalf => (0.0, 0.0, 0.5, 1.0),
            Self::RightHalf => (0.5, 0.0, 0.5, 1.0),
            Self::TopLeft => (0.0, 0.0, 0.5, 0.5),
            Self::TopRight => (0.5, 0.0, 0.5, 0.5),
            Self::BottomLeft => (0.0, 0.5, 0.5, 0.5),
            Self::BottomRight => (0.5, 0.5, 0.5, 0.5),
        }
    }
}

/// Parse `--window-pos` and wire the placer **on X11 only**. On native
/// Wayland this logs a one-line explanation and does nothing (positioning
/// is impossible there — see the module docs). When it does wire up, it
/// also suppresses geometry persistence so this transient side-by-side
/// layout doesn't overwrite the user's saved bounds. No-op when the flag is
/// absent. Native-only; wasm has no OS window.
#[cfg(not(target_arch = "wasm32"))]
pub fn wire_window_placement(app: &mut App, args: &[String]) {
    let Some(p) = WindowPlacement::from_args(args) else {
        return;
    };
    // `WAYLAND_DISPLAY` set ⇒ native Wayland. Positioning is forbidden by
    // the protocol, so don't pretend — log once and bail. (XWayland leaves
    // `WAYLAND_DISPLAY` unset for the X11-backed client, so it takes the
    // X11 path below and works.)
    let on_wayland = std::env::var("WAYLAND_DISPLAY")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if on_wayland {
        eprintln!(
            "[window] --window-pos {p:?} ignored: native Wayland forbids apps from positioning their own window. \
             Tile manually on COSMIC with Super+←/→ (or enable auto-tiling, Super+Y). \
             --window-pos works on X11/XWayland."
        );
        return;
    }
    app.insert_resource(p);
    app.insert_resource(crate::window_persistence::SkipWindowGeometrySave(true));
    app.add_systems(Update, apply_window_placement);
}

/// wasm stub: there's no OS window to place.
#[cfg(target_arch = "wasm32")]
pub fn wire_window_placement(_app: &mut bevy::prelude::App, _args: &[String]) {}

/// `Update` system that resizes + repositions the primary window to its
/// [`WindowPlacement`] once winit has reported a monitor. **X11 only** (only
/// registered by [`wire_window_placement`] off Wayland).
///
/// Re-asserts for a few frames because a `resolution` change queues a winit
/// resize that completes on a *later* frame and can nudge the top-left; the
/// final assertion wins. Clears any restored maximized state first, since a
/// maximized window ignores explicit size/position.
#[cfg(not(target_arch = "wasm32"))]
fn apply_window_placement(
    placement: Res<WindowPlacement>,
    primary_mon: Query<&bevy::window::Monitor, With<bevy::window::PrimaryMonitor>>,
    any_mon: Query<&bevy::window::Monitor>,
    mut window: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
    mut frames: Local<u32>,
) {
    use bevy::window::WindowPosition;
    const REASSERT_FRAMES: u32 = 10;
    if *frames >= REASSERT_FRAMES {
        return;
    }
    // Prefer the flagged primary monitor; fall back to whatever winit
    // reported first (some platforms don't tag a primary).
    let Some(mon) = primary_mon.iter().next().or_else(|| any_mon.iter().next()) else {
        return; // monitors not enumerated yet — retry next frame (don't tick)
    };
    let Ok(mut win) = window.single_mut() else {
        return;
    };
    if *frames == 0 {
        win.set_maximized(false);
    }
    let sf = mon.scale_factor.max(0.1);
    let (fx, fy, fw, fh) = placement.rect();
    let (mw, mh) = (mon.physical_width as f64, mon.physical_height as f64);
    let (ox, oy) = (
        mon.physical_position.x as f64,
        mon.physical_position.y as f64,
    );
    let phys_x = (ox + fx * mw).round() as i32;
    let phys_y = (oy + fy * mh).round() as i32;
    // `WindowResolution` is logical points; convert from physical via the
    // monitor scale factor. Position stays physical pixels.
    let log_w = ((fw * mw) / sf).round() as f32;
    let log_h = ((fh * mh) / sf).round() as f32;
    win.resolution.set(log_w, log_h);
    win.position = WindowPosition::At(IVec2::new(phys_x, phys_y));
    if *frames == 0 {
        info!(
            "[window] placement {:?} -> pos ({phys_x},{phys_y}) size {log_w}x{log_h} logical (monitor {mw}x{mh}, sf {sf:.2})",
            *placement
        );
    }
    *frames += 1;
}

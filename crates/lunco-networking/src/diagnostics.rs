//! Client-prediction diagnostics — the tools that cracked the 2026-06-11 jitter.
//!
//! These are **read-only observers of public ECS state** (no instrumentation of the
//! hot netcode systems): they query rendered `GlobalTransform`, avian velocities,
//! and `PendingCorrection` residuals, and report anomalies via `tracing`. Because
//! the prediction systems live one crate down (`lunco-sandbox-edit`) and this crate
//! depends on it, the networking crate is the natural home — it sees everything the
//! prediction touches without re-coupling.
//!
//! **Compiled out of normal builds** — gated behind the `net-diag` cargo feature
//! (off by default), so it costs nothing unless you opt in. Build with it when
//! chasing jitter:
//!
//! ```text
//! cargo run -p lunco-sandbox --bin sandbox --features networking,net-diag \
//!     -- --connect 127.0.0.1:5888 --api 4002 --no-throttle
//! ```
//!
//! Active as soon as it's compiled in; silence a net-diag build for a given run
//! (without rebuilding) with `LUNCO_NET_DIAG=0`.
//!
//! Output (all prefixed `[net-diag ...]`, via `warn!`/`info!`):
//! - **jitter** — the keystone. A predicted body whose *rendered* motion steps
//!   backward against its travel direction = the visible stutter, reported with the
//!   gid and which prediction set it's in. Catches jitter from ANY layer (this is
//!   what localised the `interpolate_all()` Transform-reset bug — see
//!   `SYNC_ARCHITECTURE.md` §4.1 + README → *Client-Side Prediction*).
//! - **vel** — a replicated body moving implausibly fast = bad feed-forward (the
//!   deadbeat `(target−pos)/h` bug hit ~50 m/s) or a diverging authoritative body
//!   (a runaway cosim balloon hit ~200 m/s).
//! - **corr** — how many bodies are mid-correction and how large the residual =
//!   prediction-divergence pressure (constant non-trivial corrections ⇒ input-timing
//!   skew, the case Step 3 input-hardening would shrink).
//!
//! Method lesson encoded here: when the sim is right but it still *looks* wrong,
//! measure the **render layer** (`GlobalTransform`), not just the simulation.

use avian3d::prelude::{LinearVelocity, RigidBody};
use bevy::prelude::*;
use lunco_sandbox_edit::commands::PendingCorrection;
use std::collections::HashMap;

/// Speed (m/s) above which a replicated body is almost certainly mis-driven — no
/// rover legitimately exceeds it; the cap in `drive_kinematic_proxies` is 50.
const VEL_WARN: f64 = 30.0;

/// Backward render-step (m, against smoothed travel direction) that counts as a
/// visible stutter for [`report_render_jitter`].
const JITTER_BACK_STEP: f32 = 0.02;

/// Registers the diagnostic observers. Compiled only under the `net-diag` feature,
/// so its mere presence means you asked for diagnostics → **active by default**;
/// `LUNCO_NET_DIAG=0` silences this run without a rebuild.
pub(crate) struct NetDiagnosticsPlugin;

impl Plugin for NetDiagnosticsPlugin {
    fn build(&self, app: &mut App) {
        // Active unless explicitly silenced (the feature being compiled in is the
        // opt-in; the env var is just a per-run mute).
        let enabled = std::env::var("LUNCO_NET_DIAG").map(|v| v != "0").unwrap_or(true);
        if !enabled {
            info!("[net-diag] compiled in but muted (LUNCO_NET_DIAG=0)");
            return;
        }
        info!(
            "[net-diag] ON — render-jitter + velocity + correction census active. \
             Mute with LUNCO_NET_DIAG=0. See lunco-networking/src/diagnostics.rs."
        );
        // Jitter detection reads the FINAL GlobalTransform → run in `Last`, after
        // transform propagation has built it for this frame.
        app.add_systems(Last, report_render_jitter);
        app.add_systems(Update, (report_proxy_velocity, report_corrections));
    }
}

/// **Keystone.** Catch jitter at the layer the user actually sees — the
/// post-propagation `GlobalTransform` — independent of which system caused it. Per
/// frame, for each rover, compare rendered translation to last frame; an EMA of the
/// delta estimates travel direction; a frame that steps *backward* against it by
/// more than [`JITTER_BACK_STEP`] is the stutter signature. Reports the gid, which
/// prediction set the body is in (owned vs predicted-dynamic), and the backward
/// distance — so you know *which* body stutters and *where* in the pipeline to look.
fn report_render_jitter(
    q: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&lunco_core::GlobalEntityId>,
            Has<lunco_core::OwnedLocally>,
            Has<lunco_core::PredictedDynamic>,
        ),
        With<lunco_fsw::FlightSoftware>,
    >,
    mut prev: Local<HashMap<Entity, (Vec3, Vec3)>>, // entity → (last pos, smoothed delta)
    mut n: Local<u32>,
) {
    for (e, gt, gid, owned, pred) in q.iter() {
        let pos = gt.translation();
        let entry = prev.entry(e).or_insert((pos, Vec3::ZERO));
        let delta = pos - entry.0;
        entry.0 = pos;
        let dir = entry.1;
        entry.1 = dir * 0.85 + delta * 0.15; // per-frame EMA of travel direction
        let speed = dir.length();
        if speed < 0.005 {
            continue; // effectively at rest — nothing to stutter against
        }
        let back = delta.dot(dir / speed);
        if back < -JITTER_BACK_STEP {
            *n = n.wrapping_add(1);
            if *n % 5 == 1 {
                // throttle: 1-in-5, jitter fires in bursts
                warn!(
                    "[net-diag jitter] gid={:x} owned={owned} pred={pred} \
                     back_step={:.3}m frame_delta={:.3}m",
                    gid.map_or(0, |g| g.get()),
                    -back,
                    delta.length(),
                );
            }
        }
    }
}

/// Velocity-spike census over replicated bodies. Per-body `warn!` above
/// [`VEL_WARN`] (mis-driven proxy / diverging authority), plus a ~1 s max-speed
/// `info!` line so you can watch the distribution while driving.
fn report_proxy_velocity(
    q: Query<
        (Option<&lunco_core::GlobalEntityId>, &LinearVelocity),
        (With<RigidBody>, With<lunco_core::NetReplicate>),
    >,
    mut n: Local<u32>,
) {
    *n = n.wrapping_add(1);
    let mut max = 0.0f64;
    let mut max_gid = 0u64;
    for (gid, lv) in q.iter() {
        let s = lv.0.length();
        if s > VEL_WARN {
            warn!(
                "[net-diag vel] gid={:x} speed={s:.1} m/s (>{VEL_WARN}) — bad feed-forward \
                 or diverging authoritative body",
                gid.map_or(0, |g| g.get()),
            );
        }
        if s > max {
            max = s;
            max_gid = gid.map_or(0, |g| g.get());
        }
    }
    if *n % 60 == 0 && max > 0.1 {
        info!("[net-diag vel] max replicated speed={max:.1} m/s (gid={max_gid:x})");
    }
}

/// Correction-pressure census: how many bodies are mid-`PendingCorrection` and the
/// largest residual, every ~1 s. A healthy client shows few/small residuals;
/// sustained non-trivial corrections mean the prediction keeps diverging (input
/// phase skew — the case Step 3 input-hardening shrinks).
fn report_corrections(
    q: Query<(Option<&lunco_core::GlobalEntityId>, &PendingCorrection)>,
    mut n: Local<u32>,
) {
    *n = n.wrapping_add(1);
    if *n % 60 != 0 {
        return;
    }
    let mut count = 0u32;
    let mut max = 0.0f32;
    let mut max_gid = 0u64;
    for (gid, pc) in q.iter() {
        count += 1;
        let m = pc.pos.length();
        if m > max {
            max = m;
            max_gid = gid.map_or(0, |g| g.get());
        }
    }
    if count > 0 {
        info!(
            "[net-diag corr] {count} bodies mid-correction, max residual={max:.3}m (gid={max_gid:x})"
        );
    }
}

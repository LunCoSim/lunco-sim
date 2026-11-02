//! Shared runtime health snapshots.
//!
//! Diagnostics are produced once at the engine boundary and then consumed by
//! telemetry, the HUD, and API adapters.  Consumers must not independently
//! scan `DiagnosticsStore` for the same facts at their own cadence.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use std::collections::VecDeque;

/// Retained frame-health horizon used by diagnostics, the app health snapshot,
/// and the application telemetry channel. At 60 Hz this is approximately four seconds.
pub const ENGINE_HEALTH_HISTORY_LEN: usize = 240;

/// Latest presentation/render health facts published by the engine.
///
/// Values are canonical `f64` diagnostics.  UI adapters may narrow at their
/// explicit text/GPU boundary, but telemetry and other runtime consumers read
/// this resource without another diagnostic-store scan.
#[derive(Resource, Reflect, Debug, Clone, Default)]
#[reflect(Resource, Debug)]
pub struct EngineHealthSnapshot {
    /// Smoothed frames per second, when a frame diagnostic is installed.
    pub fps: Option<f64>,
    /// Smoothed frame time in milliseconds, when available.
    pub frame_time_ms: Option<f64>,
    /// Latest unsmoothed frame time in milliseconds, when available.
    ///
    /// The headline uses `frame_time_ms`; telemetry history uses this value so
    /// short hitches remain visible in a generic sparkline or plot.
    pub raw_frame_time_ms: Option<f64>,
    /// Recent raw frame times for application-level performance views.
    ///
    /// This history advances with rendered frames, including while no Twin is
    /// active. Simulation telemetry remains sampled on its simulation clock.
    pub frame_time_history: VecDeque<f64>,
    /// Number of raw frame samples published over this application's lifetime.
    /// Together with the retained history length this identifies each sample.
    pub frame_sample_count: u64,
    /// Monotonic revision of the published values.
    pub revision: u64,
}

/// Latest physics-cycle timing fact.
#[derive(Resource, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Resource, Debug)]
pub struct PhysicsHealthSnapshot {
    /// Measured Avian step time in milliseconds.
    pub step_time_ms: Option<f64>,
    /// Avian's step counter at publication time.
    pub step_number: u64,
    /// Monotonic revision of the published values.
    pub revision: u64,
}

/// Scene-scoped Modelica worker cost and completion-throughput measurements.
#[derive(Resource, Reflect, Debug, Clone, Copy, Default, PartialEq)]
#[reflect(Resource, Debug)]
pub struct ModelicaHealthSnapshot {
    /// Minimum completed steps per second across instances with live samples.
    /// Paused instances contribute zero; absent measurements remain `None`.
    pub fps: Option<f64>,
    /// Largest per-instance mean worker service time over this sample window.
    pub worker_step_ms: Option<f64>,
    /// Summed worker service time divided by completed physics ticks in the window.
    /// Parallel worker contributions overlap; this is work cost, not barrier latency.
    pub worker_ms_per_physics_step: Option<f64>,
    /// Exact physics-tick denominator for the measured window, absent without a clock.
    pub sample_physics_steps: Option<u64>,
    pub participants: usize,
    pub sample_wall_secs: Option<f64>,
}

impl lunco_core::SceneResource for ModelicaHealthSnapshot {}

/// Publish the engine diagnostics after the frame's Update systems have
/// recorded them, before the UI and render hand-off consume the snapshot.
pub fn publish_engine_health(
    diagnostics: Option<Res<DiagnosticsStore>>,
    mut snapshot: ResMut<EngineHealthSnapshot>,
) {
    let Some(diagnostics) = diagnostics else {
        return;
    };

    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed());
    let frame_time_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|diagnostic| diagnostic.smoothed());

    let raw_frame_time_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|diagnostic| diagnostic.value());

    if snapshot.fps != fps
        || snapshot.frame_time_ms != frame_time_ms
        || snapshot.raw_frame_time_ms != raw_frame_time_ms
    {
        snapshot.fps = fps;
        snapshot.frame_time_ms = frame_time_ms;
        snapshot.raw_frame_time_ms = raw_frame_time_ms;
        snapshot.revision = snapshot.revision.wrapping_add(1);
    }

    if let Some(frame_time_ms) = raw_frame_time_ms.filter(|value| value.is_finite()) {
        if snapshot.frame_time_history.len() == ENGINE_HEALTH_HISTORY_LEN {
            snapshot.frame_time_history.pop_front();
        }
        snapshot.frame_time_history.push_back(frame_time_ms);
        snapshot.frame_sample_count = snapshot.frame_sample_count.wrapping_add(1);
    }
}

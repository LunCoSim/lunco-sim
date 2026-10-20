//! Read-only projection of the application's canonical render-frame history.

use bevy::prelude::*;
use lunco_api_core::{ApiErrorCode, ApiQuerySchema, ApiValue, api_value};
use lunco_core_runtime::{ENGINE_HEALTH_HISTORY_LEN, EngineHealthSnapshot};

use super::{ApiQueryError, ApiQueryProvider, ApiQueryResult, query_schema};

/// Bounded render-frame samples, independent of simulation telemetry cadence.
pub struct EnginePerformanceProvider;

impl ApiQueryProvider for EnginePerformanceProvider {
    fn name(&self) -> &'static str {
        "EnginePerformance"
    }

    fn schema(&self) -> ApiQuerySchema {
        query_schema(
            self.name(),
            "Read the canonical application frame history in chronological order. Samples advance on render frames, including while simulation is paused. Missing diagnostics remain unavailable.",
            Vec::new(),
            "{ available: bool, fps: f64 | (), frame_time_ms: f64 | (), raw_frame_time_ms: f64 | (), frame_sample_count: u64, history_capacity: usize, frame_time_samples_ms: f64[] }. frame_sample_count identifies the last retained sample; subtract the history length to identify its preceding sample. The counter wraps at u64::MAX and is scoped to the application process.",
        )
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let snapshot = world
            .get_resource::<EngineHealthSnapshot>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "EnginePerformance: EngineHealthSnapshot is not installed",
                )
            })?;
        Ok(Some(api_value!({
            "available": !snapshot.frame_time_history.is_empty(),
            "fps": snapshot.fps,
            "frame_time_ms": snapshot.frame_time_ms,
            "raw_frame_time_ms": snapshot.raw_frame_time_ms,
            "frame_sample_count": snapshot.frame_sample_count,
            "history_capacity": ENGINE_HEALTH_HISTORY_LEN,
            "frame_time_samples_ms": snapshot.frame_time_history.iter().copied().collect::<Vec<_>>(),
        })))
    }
}

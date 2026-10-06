//! Shared trajectory and artifact admission budgets.

use crate::RunResult;
use serde::{Deserialize, Serialize};

/// Configured limits captured by a run or persistence operation at admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunResultLimits {
    /// Maximum scalar f64 values, including the time vector.
    pub max_values: usize,
    /// Serialized artifact byte budget for persistence admission.
    pub max_artifact_bytes: usize,
}

impl Default for RunResultLimits {
    fn default() -> Self {
        Self {
            max_values: 8_000_000,
            max_artifact_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResultError(pub String);

impl std::fmt::Display for RunResultError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for RunResultError {}

impl RunResultLimits {
    pub fn validate(self) -> Result<(), RunResultError> {
        if self.max_values == 0 || self.max_artifact_bytes == 0 {
            return Err(RunResultError(
                "experiment result limits must be positive".into(),
            ));
        }
        Ok(())
    }

    /// `outputs` is the actual scalar output-column count, excluding the
    /// separate time vector. Call before allocating trajectory storage.
    pub fn validate_dimensions(self, outputs: usize, samples: usize) -> Result<(), RunResultError> {
        self.validate()?;
        let values = outputs
            .checked_add(1)
            .and_then(|columns| columns.checked_mul(samples))
            .ok_or_else(|| RunResultError("experiment output dimensions overflow".into()))?;
        if values > self.max_values {
            return Err(RunResultError(format!(
                "experiment output requires {values} scalar values; configured limit is {}",
                self.max_values
            )));
        }
        Ok(())
    }
}

impl RunResult {
    /// Validate a complete trajectory at its registry/persistence boundary.
    /// Partial streams retain their explicit NaN hole-padding contract.
    /// Equal event timestamps are valid; time must never run backwards.
    pub fn validate_complete(&self, limits: RunResultLimits) -> Result<(), RunResultError> {
        limits.validate_dimensions(self.series.len(), self.times.len())?;
        if self.times.is_empty() {
            return Err(RunResultError(
                "complete experiment result has no samples".into(),
            ));
        }
        if self.meta.sample_count != self.times.len() {
            return Err(RunResultError(
                "experiment result sample_count does not match its time vector".into(),
            ));
        }
        for (index, &time) in self.times.iter().enumerate() {
            if !time.is_finite() {
                return Err(RunResultError(format!(
                    "experiment result time at sample {index} is nonfinite"
                )));
            }
            if index > 0 && time < self.times[index - 1] {
                return Err(RunResultError(format!(
                    "experiment result time moves backwards at sample {index}"
                )));
            }
        }
        for (name, values) in &self.series {
            if values.len() != self.times.len() {
                return Err(RunResultError(format!(
                    "experiment result series `{name}` has {} samples, expected {}",
                    values.len(),
                    self.times.len()
                )));
            }
            if let Some(index) = values.iter().position(|value| !value.is_finite()) {
                return Err(RunResultError(format!(
                    "complete experiment result series `{name}` is nonfinite at sample {index}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RunMeta;
    use std::collections::BTreeMap;

    #[test]
    fn complete_result_admission_rejects_malformed_and_over_budget_storage() {
        let limits = RunResultLimits {
            max_values: 6,
            max_artifact_bytes: 1024,
        };
        let valid = RunResult {
            times: vec![0.0, 1.0, 1.0],
            series: BTreeMap::from([("x".into(), vec![1.0, 2.0, 3.0])]),
            meta: RunMeta {
                sample_count: 3,
                ..Default::default()
            },
        };
        assert!(valid.validate_complete(limits).is_ok());
        let mut invalid = valid.clone();
        invalid.meta.sample_count = 2;
        assert!(
            invalid
                .validate_complete(limits)
                .unwrap_err()
                .0
                .contains("sample_count")
        );
        invalid = valid.clone();
        invalid.series.get_mut("x").unwrap().pop();
        assert!(
            invalid
                .validate_complete(limits)
                .unwrap_err()
                .0
                .contains("expected 3")
        );
        invalid = valid.clone();
        invalid.times[2] = 0.5;
        assert!(
            invalid
                .validate_complete(limits)
                .unwrap_err()
                .0
                .contains("backwards")
        );
        invalid = valid.clone();
        invalid.times[1] = f64::INFINITY;
        assert!(
            invalid
                .validate_complete(limits)
                .unwrap_err()
                .0
                .contains("time at sample")
        );
        invalid = valid.clone();
        invalid.series.get_mut("x").unwrap()[1] = f64::NAN;
        assert!(
            invalid
                .validate_complete(limits)
                .unwrap_err()
                .0
                .contains("nonfinite")
        );
        assert!(
            valid
                .validate_complete(RunResultLimits {
                    max_values: 5,
                    ..limits
                })
                .unwrap_err()
                .0
                .contains("configured limit")
        );
        assert!(
            limits
                .validate_dimensions(usize::MAX, 2)
                .unwrap_err()
                .0
                .contains("overflow")
        );
        assert!(
            RunResultLimits {
                max_values: 0,
                ..limits
            }
            .validate()
            .is_err()
        );
        assert!(
            RunResultLimits {
                max_artifact_bytes: 0,
                ..limits
            }
            .validate()
            .is_err()
        );
    }
}

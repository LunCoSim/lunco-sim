//! Runtime adapter for the authored USD document's stage-convention types.
//!
//! The data model and conversion math live in `lunco-usd-document`. This
//! module only adapts the composed Bevy reader to that document seam.

use crate::read::UsdReadObject;
use lunco_usd_document::units::{
    ConventionTransform, StageMetadataReader, StageMetrics, StageMetricsError,
};

impl StageMetadataReader for dyn UsdReadObject + '_ {
    fn stage_metadata_value(&self, name: &str) -> Option<openusd::sdf::Value> {
        UsdReadObject::stage_metadata_value(self, name)
    }
}

impl StageMetadataReader for crate::StageView<'_> {
    fn stage_metadata_value(&self, name: &str) -> Option<openusd::sdf::Value> {
        UsdReadObject::stage_metadata_value(self, name)
    }
}

/// Convert the composed reader's declared stage convention to canonical space.
pub fn stage_convention(
    reader: &dyn UsdReadObject,
) -> Result<ConventionTransform, StageMetricsError> {
    Ok(ConventionTransform::from_stage_metrics(
        &StageMetrics::from_reader(reader)?,
    ))
}

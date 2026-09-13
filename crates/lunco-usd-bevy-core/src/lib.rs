//! Headless composed-USD substrate shared by visual and domain projections.
//!
//! This package owns the reader contract, live composed view, send-safe
//! projection snapshot, composition entry points, and USD-only policy helpers.
//! It deliberately contains no mesh, light, camera, window, renderer, or UI
//! projection. `lunco-usd-bevy` is the visual adapter built on this substrate.

mod material_binding;
mod purpose;
mod transform;

pub mod asset;
pub mod authoring;
pub mod canonical;
pub mod compose;
pub mod instance;
pub mod mount;
pub mod point_instancer;
pub mod program;
pub mod projection_plan;
pub mod read;
pub mod source;
pub mod units;
pub mod variants;
pub mod view;

pub use asset::{UsdLoader, UsdStageAsset};
pub use authoring::{layer_default_prim, DefaultPrim};
pub use instance::{UsdInstanceMember, UsdInstanceProjection, UsdInstanceRoot};
pub use material_binding::{
    parent_prim_path, resolve_bound_material, resolve_bound_shader, MaterialPurpose,
};
pub use projection_plan::{UsdPrimProjectionPlan, UsdStageProjectionPlan};
pub use purpose::{
    effective_purpose, is_descendant_or_self, resolve_stage_prim_path, stage_default_prim, Purpose,
};
pub use read::{UsdRead, UsdReadObject, UsdReadSource};
pub(crate) use transform::{compose_live_xform_order_at, stage_prim_is_invisible_or_guide};
pub use transform::{
    compose_xform_order_at, local_transform_at, read_transform_from_usd, read_xform_op_order,
    TransformReadError, RESET_XFORM_STACK,
};
pub use units::stage_convention;
pub use view::StageView;

//! Composed OpenUSD reading and immutable projection plans.
//!
//! This package is the stage-side owner of the USD contract: composition,
//! canonical views, schema-aware reads, transform/unit conventions, material
//! resolution, and send-safe plans. It has no ECS projection systems. Runtime
//! binders and live-edit consumers live in [`lunco_usd_bevy_core`], which
//! depends on this package.

mod material_binding;
mod purpose;
mod transform;

pub mod asset;
pub mod authoring;
pub mod canonical;
pub mod compose;
pub mod instance;
pub mod projection_plan;
pub mod read;
pub mod source;
pub mod units;
pub mod variants;
pub mod view;

pub use asset::{UsdLoader, UsdStageAsset};
pub use authoring::{DefaultPrim, layer_default_prim};
pub use canonical::UsdWiringDirty;
pub use instance::{UsdInstanceMember, UsdInstanceProjection, UsdInstanceRoot};
pub use material_binding::{
    MaterialPurpose, parent_prim_path, resolve_bound_material, resolve_bound_shader,
};
pub use projection_plan::{UsdPrimProjectionPlan, UsdStageProjectionPlan};
pub use purpose::{
    Purpose, effective_purpose, is_descendant_or_self, resolve_stage_prim_path, stage_default_prim,
};
pub use read::{UsdRead, UsdReadObject, UsdReadSource};
pub use transform::{
    RESET_XFORM_STACK, TransformReadError, compose_xform_order_at, euler_xyz_deg_to_quat,
    grid_translation_d_at, local_transform_at, read_transform_from_usd, read_xform_op_order,
    transform_in_body_frame, world_transform,
};
pub(crate) use transform::{compose_live_xform_order_at, stage_prim_is_invisible_or_guide};
pub use units::stage_convention;
pub use view::StageView;

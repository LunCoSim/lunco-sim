//! Inspector and authored USD panels for the LunCoSim scene editor.
//!
//! The sibling [`lunco_luncosim_edit_ui`] package owns viewport interaction,
//! selection, gizmos, and the editor session boundary. This package owns the
//! panels that inspect and author composed USD state. Keeping the two layers
//! separate means interaction-only hosts do not compile the large inspector
//! and USD authoring surface.

#![forbid(unsafe_code)]

use bevy::prelude::*;
use lunco_luncosim_edit_ui::ui::{usd_selection_view_changed, ViewModelAppExt};
use lunco_workbench::WorkbenchAppExt;

pub mod inspector;
pub mod usd_animation;
pub mod usd_joint;
pub mod usd_mount;
pub mod usd_params;
pub mod usd_prim_tree;
pub mod usd_variants;

/// Installs the Inspector and authored USD panels used by the Editor
/// perspective.
pub struct SceneEditPanelsUiPlugin;

impl Plugin for SceneEditPanelsUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<lunco_luncosim_edit_ui::InspectorTarget>();

        app.register_panel(inspector::Inspector)
            .register_panel(inspector::EnvironmentPanel)
            .register_panel(usd_prim_tree::UsdPrimTreePanel);

        app.init_resource::<inspector::InspectorView>()
            .init_resource::<inspector::ShaderSchemaCache>();
        app.add_observer(inspector::on_inspector_component_edit)
            .add_observer(inspector::on_projection_edit_requested)
            .add_observer(inspector::on_usd_attribute_edit_requested)
            .add_observer(inspector::on_usd_attribute_batch_edit_requested)
            .add_observer(inspector::on_usd_variant_edit_requested)
            .add_observer(inspector::on_mount_snap_requested)
            .add_observer(inspector::on_mount_detach_requested)
            .add_observer(inspector::on_shader_swap_requested)
            .add_observer(inspector::on_shader_create_requested)
            .add_observer(inspector::on_shader_import_requested)
            .add_observer(inspector::on_shader_parameters_requested)
            .add_observer(inspector::on_pbr_material_requested)
            .add_observer(inspector::on_modelica_parameter_requested);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_observer(inspector::on_attach_at_socket_requested);
        app.add_view_model(
            inspector::populate_inspector_view,
            inspector::inspector_inputs_changed,
        );

        app.init_resource::<usd_prim_tree::UsdPrimTreeView>();
        app.add_view_model(
            usd_prim_tree::produce_usd_prim_tree,
            usd_prim_tree::editor_prim_tree_changed,
        );

        app.init_resource::<usd_params::UsdParamView>()
            .init_resource::<usd_params::UsdParamDrafts>();
        app.add_view_model(
            usd_params::produce_usd_param_view,
            usd_selection_view_changed,
        );

        app.init_resource::<usd_variants::UsdVariantView>();
        app.add_view_model(
            usd_variants::produce_usd_variant_view,
            usd_selection_view_changed,
        );

        app.init_resource::<usd_mount::UsdMountView>();
        app.add_view_model(
            usd_mount::produce_usd_mount_view,
            usd_selection_view_changed,
        );

        app.init_gizmo_group::<usd_joint::UsdJointPreviewGizmoConfigGroup>()
            .init_resource::<usd_joint::UsdJointView>();
        app.add_view_model(
            usd_joint::produce_usd_joint_view,
            usd_selection_view_changed,
        );
        app.add_systems(
            PostUpdate,
            (
                usd_joint::sync_usd_joint_preview_gizmo_config,
                usd_joint::draw_usd_joint_preview_viz
                    .after(bevy::transform::TransformSystems::Propagate)
                    .before(bevy::camera::CameraUpdateSystems),
            )
                .chain()
                .after(lunco_core::SceneViewportSet::Reconcile),
        );

        app.init_resource::<usd_animation::UsdAnimationView>();
        app.add_view_model(
            usd_animation::produce_usd_animation_view,
            usd_selection_view_changed,
        );

        app.add_systems(Update, inspector::delete_selected_on_intent);
    }
}

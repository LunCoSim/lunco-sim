//! USD-backed scene authoring mechanisms.
//!
//! This package owns document-backed property and shader edits, including the
//! shader document registry and the shared entity-to-document ownership
//! resolver. It is a production command surface, not an editor-only helper:
//! the headless scene command host installs it beside the runtime mutation and
//! query packages.

pub mod doc_resolve;
pub mod properties;
pub mod shader_doc;

use bevy::prelude::*;
use lunco_core::register_commands;

/// Installs property/shader commands and the shader journal bridge.
pub struct SceneAuthoringPlugin;

register_commands!(
    properties::on_create_shader,
    properties::on_delete_shader,
    properties::on_import_shader,
    properties::on_reload_shader,
    properties::on_set_object_property,
    properties::on_set_shader_source,
);

impl Plugin for SceneAuthoringPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
        app.init_resource::<shader_doc::ShaderRegistry>();
        app.add_observer(properties::persist_wheel_to_runtime_layer);
        app.add_systems(
            Update,
            shader_doc::wire_shader_journal_handle
                .run_if(resource_added::<lunco_doc_bevy::JournalResource>),
        );
    }
}

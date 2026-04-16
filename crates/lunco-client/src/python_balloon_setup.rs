//! Python balloon setup — initializes ScriptedModel and wires it to AvianSim.

use bevy::prelude::*;
use lunco_assets::assets_dir;
use lunco_cosim::{SimComponent, SimStatus, SimConnection};
use lunco_scripting::{
    ScriptRegistry, doc::{ScriptDocument, ScriptedModel, ScriptLanguage},
};
use lunco_doc::{DocumentId, DocumentHost};
use lunco_sandbox_edit::catalog::PythonBalloonMarker;

/// System that initializes the Python script for new green balloons.
pub fn setup_python_balloon(
    mut commands: Commands,
    q_new: Query<(Entity, &Name), Added<PythonBalloonMarker>>,
    mut registry: ResMut<ScriptRegistry>,
) {
    for (entity, name) in &q_new {
        let script_path = assets_dir().join("scripts/green_balloon.py");
        let source = match std::fs::read_to_string(&script_path) {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to read green_balloon.py: {e}");
                continue;
            }
        };

        let doc_id = DocumentId::new(entity.index().index() as u64 + 10000); // Offset to avoid collision
        let doc = ScriptDocument {
            id: doc_id.raw(),
            generation: 0,
            language: ScriptLanguage::Python,
            source: source.clone(),
            inputs: vec!["height".to_string(), "velocity".to_string()],
            outputs: vec!["netForce".to_string()],
        };

        registry.documents.insert(doc_id, DocumentHost::new(doc));

        // Attach ScriptedModel
        commands.entity(entity).insert(ScriptedModel {
            document_id: Some(doc_id.raw()),
            language: Some(ScriptLanguage::Python),
            paused: false,
            inputs: Default::default(),
            outputs: Default::default(),
        });

        // Create SimComponent
        let comp = SimComponent {
            model_name: "Green Balloon (Python)".into(),
            parameters: Default::default(),
            inputs: Default::default(),
            outputs: Default::default(),
            status: SimStatus::Running,
            is_stepping: false,
        };
        commands.entity(entity).insert(comp);

        // Wires (same as Modelica balloon)
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "netForce".into(),
            end_element: entity, end_connector: "force_y".into(), scale: 1.0,
        });
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "height".into(),
            end_element: entity, end_connector: "height".into(), scale: 1.0,
        });
        commands.spawn(SimConnection {
            start_element: entity, start_connector: "velocity_y".into(),
            end_element: entity, end_connector: "velocity".into(), scale: 1.0,
        });

        info!("Balloon '{name}' — Python script and wires initialized");
        commands.entity(entity).remove::<PythonBalloonMarker>();
    }
}

/// Syncs ScriptedModel.outputs → SimComponent.outputs.
pub fn sync_script_outputs(
    mut q_models: Query<(&ScriptedModel, &mut SimComponent)>,
) {
    for (model, mut comp) in &mut q_models {
        for (name, val) in &model.outputs {
            comp.outputs.insert(name.to_string(), *val);
        }
    }
}

/// Syncs wire inputs → ScriptedModel.inputs.
pub fn sync_inputs_to_script(
    mut q_models: Query<(&SimComponent, &mut ScriptedModel)>,
) {
    for (comp, mut model) in &mut q_models {
        for (name, val) in &comp.inputs {
            model.inputs.insert(name.to_string(), *val);
        }
    }
}

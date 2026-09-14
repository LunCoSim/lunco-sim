//! Runtime metadata for Modelica source generated from composed USD networks.
//!
//! The projection owns this state because it describes an ephemeral runtime
//! document, not the compiler's authored-document registry. UI code may read
//! the metadata, but it does not own or publish it.

use bevy::prelude::Resource;
use lunco_doc::{DocumentId, DocumentOrigin};

/// The single provenance classifier for ephemeral generated Modelica docs.
///
/// Generated documents are bundled-origin documents with a reserved filename
/// prefix. The source itself remains in the document registry; this predicate
/// only classifies its lifecycle origin.
pub fn is_generated_origin(origin: &DocumentOrigin) -> bool {
    matches!(
        origin,
        DocumentOrigin::Bundled { filename } if filename.starts_with("generated/")
    )
}

/// Runtime-generated Modelica documents projected from composed USD networks.
#[derive(Resource, Default, Clone, Debug)]
pub struct GeneratedModelicaSources {
    /// Current generated network documents.
    pub entries: Vec<GeneratedModelicaSourceEntry>,
    /// Change-driven publication gate. Producers set this when generated
    /// document links or entities change; generated-source changes are
    /// detected by the publisher's ECS query. Solver output is not this
    /// registry's invalidation input.
    pub dirty: bool,
}

/// A synthesized composite unit exposed in a generated Modelica document.
#[derive(Clone, Debug, Default)]
pub struct GeneratedModelicaUnit {
    /// Generated Modelica class name.
    pub name: String,
    /// Generated Modelica instance name used by the root and telemetry map.
    pub instance: String,
    /// Composed USD member paths absorbed by the unit.
    pub members: Vec<String>,
    /// Root boundary inputs consumed by this unit.
    pub inputs: Vec<String>,
    /// Root boundary outputs produced by this unit.
    pub outputs: Vec<String>,
}

/// One ephemeral Modelica source document available to runtime consumers.
#[derive(Clone, Debug)]
pub struct GeneratedModelicaSourceEntry {
    /// The ordinary Modelica document backing this generated scene network.
    /// It is read-only, but otherwise opens in the standard Modelica view.
    pub document: DocumentId,
    /// Stable generated URI used by solver diagnostics.
    pub uri: String,
    /// Composed USD network that produced the document.
    pub network_root: String,
    /// Generated root class name shown when the document is selected.
    pub model_name: String,
    /// Exact source sent to the compiler.
    pub source: String,
    /// All composed USD components absorbed by the generated projection.
    pub component_paths: Vec<String>,
    /// Composite units emitted by the synthesis policy.
    pub units: Vec<GeneratedModelicaUnit>,
    /// `(member USD path, source asset, Modelica class)` attribution copied
    /// from the generated source contract.
    pub members: Vec<(String, String, String)>,
    /// Bundled Modelica roots requested by the synthesis policy.
    pub source_roots: Vec<String>,
    /// Inputs exposed by the generated root class.
    pub boundary_inputs: Vec<String>,
    /// Outputs exposed by the generated root class.
    pub boundary_outputs: Vec<String>,
    /// Promoted member telemetry as `(member path, member output, alias)`.
    pub member_output_aliases: Vec<(String, String, String)>,
    /// Error produced while projecting the USD network, if any. Compiler and
    /// solver errors remain on the linked Modelica model/document state.
    pub projection_error: Option<String>,
}

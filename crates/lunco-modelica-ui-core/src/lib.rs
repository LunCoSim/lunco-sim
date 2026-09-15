//! Render-independent contracts shared by Modelica-facing UI packages.
//!
//! This crate owns the command payloads and persisted identifiers that cross
//! the boundary between the Modelica workbench and other application UI. It
//! intentionally contains no panels, observers, workbench shell, compiler,
//! or renderer dependency. Concrete behavior stays in the package that owns
//! the corresponding UI surface.

use bevy::ecs::reflect::ReflectEvent;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;

/// Stable panel-kind string for the Modelica time-series plot instances.
pub const MODELICA_PLOT_KIND_ID: &str = "modelica_plot";

/// Stable instance number for the singleton Modelica time-series plot.
pub const DEFAULT_MODELICA_GRAPH_ID: u64 = 1;

/// Focus the first open Modelica document whose title contains `pattern`.
///
/// The Modelica UI owns the observer and tab-resolution policy. Keeping only
/// the payload here lets other UI packages request focus without depending on
/// the complete Modelica workbench.
#[Command(default)]
pub struct FocusDocumentByName {
    /// Case-insensitive substring of the document title. Empty is a no-op.
    pub pattern: String,
}

/// Request an authored and runtime update of one Modelica parameter.
///
/// The emitting inspector owns only the gesture. The Modelica UI owns the
/// document registry, journaled source edit, and worker update that fulfill
/// the request.
#[derive(bevy::prelude::Event, Clone, Debug)]
pub struct SetModelicaParameter {
    /// Modelica model entity whose parameter is being edited.
    pub entity: bevy::prelude::Entity,
    /// Parameter key as exposed by the Modelica runtime.
    pub key: String,
    /// New numeric parameter value.
    pub value: f64,
}

/// How an authored or library Modelica class should be opened.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default, bevy::reflect::Reflect)]
#[serde(tag = "kind")]
pub enum ClassAction {
    /// Open the class as a read-only view.
    #[default]
    View,
    /// Copy the class into an editable document with the requested name.
    Duplicate {
        /// Requested name for the editable copy.
        name: String,
    },
}

/// Open a Modelica class by its fully-qualified name.
///
/// The Modelica UI owns class lookup, duplication, and tab creation. This
/// payload is shared so URI handlers, application boot routing, and panels all
/// use one command contract.
#[Command(default)]
pub struct OpenClass {
    /// Fully-qualified class path, for example
    /// `Modelica.Blocks.Examples.PID_Controller`.
    pub qualified: String,
    /// Whether to view or duplicate the class.
    #[serde(default)]
    pub action: ClassAction,
}

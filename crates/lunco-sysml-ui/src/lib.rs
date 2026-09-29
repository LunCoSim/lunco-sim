//! Twin-scoped SysML requirements browsing, verification status, and source editing.

mod panel;
#[cfg(not(target_arch = "wasm32"))]
mod verification;
#[cfg(target_arch = "wasm32")]
#[path = "verification_wasm.rs"]
mod verification;
mod view_model;

use bevy::prelude::*;
use lunco_workbench_core::WorkbenchPanelAppExt;

pub use panel::SysmlRequirementsPanel;
pub use view_model::SysmlRequirementsViewModel;

/// Installs the SysML requirements browser, docked detail pane, and typed read model.
pub struct SysmlUiPlugin;

impl Plugin for SysmlUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<view_model::SysmlRequirementsViewModel>()
            .init_resource::<view_model::SysmlVerificationEvidence>()
            .init_resource::<panel::SysmlRequirementPanelState>()
            .init_resource::<verification::SysmlVerificationRuns>()
            .register_panel(panel::SysmlRequirementsPanel::default())
            .register_panel(panel::SysmlRequirementDetailsPanel)
            .add_systems(Update, view_model::produce_sysml_requirements_view_model)
            .add_systems(Update, verification::poll_sysml_verification_run)
            .add_observer(verification::start_sysml_verification)
            .add_observer(verification::start_sysml_verification_suite)
            .add_observer(verification::cancel_sysml_verification)
            .add_observer(verification::cancel_sysml_verification_suite)
            .add_observer(verification::clear_sysml_verification_results)
            .add_observer(view_model::capture_verification_evidence)
            .add_observer(view_model::clear_verification_evidence_on_twin_closed);
    }
}

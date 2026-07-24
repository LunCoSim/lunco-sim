//! One construction boundary for rumoca simulation sessions.

use rumoca_compile::compile::Dae;
use rumoca_sim::{SimOptions, SimulationDiagnosticError, SimulationSession};

/// Build the real-time co-simulation session.
///
/// The worker owns the fixed-step live solver policy; this boundary owns the
/// rumoca construction so the live path cannot grow a second constructor.
pub fn live(
    dae: &Dae,
    options: SimOptions,
) -> Result<SimulationSession, SimulationDiagnosticError> {
    construct(dae, options)
}

/// Build an interactive workbench/experiment session.
pub fn interactive(
    dae: &Dae,
    options: SimOptions,
) -> Result<SimulationSession, SimulationDiagnosticError> {
    construct(dae, options)
}

/// Build an explicit command-line or diagnostic-probe session.
pub fn cli(dae: &Dae, options: SimOptions) -> Result<SimulationSession, SimulationDiagnosticError> {
    construct(dae, options)
}

/// The sole production construction of a rumoca simulation session.
fn construct(
    dae: &Dae,
    options: SimOptions,
) -> Result<SimulationSession, SimulationDiagnosticError> {
    SimulationSession::new(dae, options)
}

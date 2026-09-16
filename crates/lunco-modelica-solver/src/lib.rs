//! Renderer-free solver capability for Modelica.
//!
//! Modelica compilation and document lifecycle stay in the Modelica compiler
//! host. This package owns the Rumoca-specific solver
//! mapping and the live integration sessions that consume a lowered solve
//! model. Keeping that boundary separate prevents solver implementation
//! changes from enlarging the compiler host's source and test target.

pub mod fixed_step;
pub mod simulation_session;
pub mod solver_backends;

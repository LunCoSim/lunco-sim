//! Modelica worker engine and Bevy co-simulation bridge.
//!
//! The worker package owns the expensive, stateful execution mechanism: live
//! steppers, command dispatch, worker-local artifact caches, native worker
//! loops, and the Bevy systems that exchange results with the simulation.
//! [`lunco_modelica_execution`] owns host assembly and browser transport.

pub mod worker;

//! Shared ECS port surfaces and backend registry for LunCoSim.
//!
//! The port substrate is independent of the general engine core. Domains add
//! their own backends to [`ports::PortRegistry`], while shared endpoint and
//! lifecycle components remain in the engine's control-surface contract.

pub mod ports;

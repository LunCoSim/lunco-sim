//! Shared ECS port surfaces and backend registry for LunCoSim.
//!
//! The port substrate is independent of the general engine core. Domains add
//! their own backends to [`ports::PortRegistry`], while shared endpoint and
//! control-surface lifecycle components are defined in [`endpoints`].

pub mod contracts;
pub mod endpoints;
pub mod ports;
pub mod units;
pub use units::EngineUnit;

pub use contracts::{ScalarPortContract, ScalarPortContracts, ScalarPortContractsPlugin};

pub use endpoints::{
    CausalStateSink, InputPorts, OutputPorts, Port, PortSurface, PortSurfacePending,
    PortSurfacePort, PortSurfaceReady, owning_input_ports, register_endpoint_types,
};
pub use ports::{PortMap, ScalarPortMap};

/// Fixed scalar connector name of a [`Port`] endpoint.
pub const PORT_NAME: &str = "value";

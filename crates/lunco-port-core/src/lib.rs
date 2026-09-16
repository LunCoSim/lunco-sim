//! Shared ECS port surfaces and backend registry for LunCoSim.
//!
//! The port substrate is independent of the general engine core. Domains add
//! their own backends to [`ports::PortRegistry`], while shared endpoint and
//! control-surface lifecycle components are defined in [`endpoints`].

pub mod endpoints;
pub mod ports;

pub use endpoints::{
    CausalStateSink, InputPorts, OutputPorts, Port, PortSurface, PortSurfacePending,
    PortSurfaceReady, owning_input_ports, register_endpoint_types, safe_stop_control_surface,
};

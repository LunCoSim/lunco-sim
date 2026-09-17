//! Shared Modelica source-library and execution-transport contracts.
//!
//! The compiler host consumes the source-library artifact and worker bridge
//! from this package. Keeping browser fetch/decode and transport callbacks
//! here prevents changes to those adapters from rebuilding the compiler's
//! document and Rumoca integration.

pub mod settings;
pub mod source_library;
pub mod worker_bridge;

pub use settings::LibrarySettings;
pub use source_library::SourceLibraryPlugin;

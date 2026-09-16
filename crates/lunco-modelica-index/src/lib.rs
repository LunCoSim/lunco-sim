//! Reusable Modelica metadata and editor-index contracts.
//!
//! This package owns the parse-shaped projection consumed by document views,
//! source-library indexing, package browsers, and diagram metadata. It is
//! deliberately separate from the compiler host: changing worker or solver
//! orchestration does not rebuild these consumers, and asset tooling can use
//! the serialized index without depending on the document runtime.

pub mod annotation_source;
pub mod class_lookup;
pub mod doc_extract;
pub mod index;
pub mod package_tree;
pub mod visual_diagram;

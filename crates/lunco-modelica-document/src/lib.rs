//! Headless Modelica document mechanics.
//!
//! This package owns the source document, typed source-edit operations, parse
//! cache, and Modelica index synchronization. Compiler sessions, worker
//! orchestration, library provisioning, and UI remain in their owning
//! packages, so consumers that only need document state do not depend on the
//! Modelica simulation engine.

pub mod document;

pub use document::{
    AstCache, CHANGE_HISTORY_CAPACITY, FreshAst, ModelicaChange, ModelicaDocument, ModelicaOp,
    OpKind, SyntaxCache, parse_diag_from_error,
};

/// The Modelica operation's canonical Twin-journal domain is defined with the
/// operation itself, next to its serialization contract.
impl lunco_twin_journal::OpPayload for ModelicaOp {
    fn domain(&self) -> lunco_twin_journal::DomainKind {
        lunco_twin_journal::DomainKind::Modelica
    }
}

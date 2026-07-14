//! Structural AST mutation — as **text splices**, never as re-emission.
//!
//! Every mutation here does two things:
//!
//! 1. mutates a *clone* of the AST, which the document layer installs as the
//!    fresh syntax cache and uses to patch its index; and
//! 2. records the byte-level [`edit::Splice`]s that make the same change to the
//!    source text.
//!
//! (2) is the source of truth for what lands on disk. It exists because
//! round-tripping a class through rumoca's emitter is lossy — see
//! [`edit`] for the failure it caused. Mutations must therefore **never**
//! call `to_modelica()` on a node that already exists in the source; they splice
//! the bytes they mean to change and leave every other byte alone. New nodes are
//! rendered by [`crate::pretty`].
//!
//! `tests/ast_mut_preserves_untouched_source.rs` enforces this: for each op it
//! asserts that every line the op did not target is byte-identical afterwards.

pub mod errors;
pub mod parsing;
pub mod text;
pub mod edit;
pub mod clause;
pub mod components;
pub mod connections;
pub mod classes;
pub mod graphics;
pub mod equations;
pub mod util;

pub use errors::AstMutError;
pub use edit::Edit;
pub use components::*;
pub use connections::*;
pub use classes::*;
pub use graphics::*;
pub use equations::*;
pub use util::{lookup_class_mut, synth_token};

use std::ops::Range;
use std::sync::Arc;
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};

/// Run a mutation against one class and return its minimal text patch.
///
/// The closure gets the class (a clone — its spans still point into `source`,
/// which is what makes splicing possible) and an [`Edit`] to record byte edits
/// on. The returned range covers only the bytes between the first and last
/// splice; anything in between that no splice claimed is copied through
/// verbatim.
pub fn class_patch<F>(
    source: &str,
    parsed: &StoredDefinition,
    class: &str,
    mutate: F,
) -> Result<(Range<usize>, String, Arc<StoredDefinition>), AstMutError>
where
    F: FnOnce(&mut ClassDef, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let mut sd_clone = parsed.clone();
    let class_def = util::lookup_class_mut(&mut sd_clone, class)?;
    let mut edit = Edit::new(source);
    mutate(class_def, &mut edit)?;
    let (range, replacement) = edit.into_patch()?;
    Ok((range, replacement, Arc::new(sd_clone)))
}

/// Run a mutation against the whole document (class add/remove, which change
/// the top-level class list rather than any single class).
pub fn document_patch<F>(
    source: &str,
    parsed: &StoredDefinition,
    mutate: F,
) -> Result<(Range<usize>, String, Arc<StoredDefinition>), AstMutError>
where
    F: FnOnce(&mut StoredDefinition, &mut Edit<'_>) -> Result<(), AstMutError>,
{
    let mut sd_clone = parsed.clone();
    let mut edit = Edit::new(source);
    mutate(&mut sd_clone, &mut edit)?;
    let (range, replacement) = edit.into_patch()?;
    Ok((range, replacement, Arc::new(sd_clone)))
}

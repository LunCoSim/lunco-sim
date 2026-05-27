//! Structural AST mutation helpers.

pub mod errors;
pub mod parsing;
pub mod components;
pub mod connections;
pub mod classes;
pub mod graphics;
pub mod equations;
pub mod util;

pub use errors::AstMutError;
pub use components::*;
pub use connections::*;
pub use classes::*;
pub use graphics::*;
pub use equations::*;
pub use util::{lookup_class_mut, synth_token};

use std::ops::Range;
use std::sync::Arc;
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
use util::*;

/// Run an AST mutation against a class and return a `(byte_range,
/// replacement)` patch suitable for `Document::apply_patch`.
pub fn regenerate_class_patch<F>(
    source: &str,
    parsed: &StoredDefinition,
    class: &str,
    mutate: F,
) -> Result<(Range<usize>, String, Arc<StoredDefinition>), AstMutError>
where
    F: FnOnce(&mut ClassDef) -> Result<(), AstMutError>,
{
    let mut sd_clone = parsed.clone();
    let class_def = lookup_class_mut(&mut sd_clone, class)?;
    mutate(class_def)?;

    let raw_start = class_def.location.start as usize;
    let raw_end = class_def.location.end as usize;
    let start = rewind_to_class_header_start(source, raw_start);
    let end = advance_past_trailing_semicolon(source, raw_end);

    let indent = leading_indent(source, start);
    let mut regen = class_def.to_modelica(&indent);
    if regen.starts_with(&indent) {
        regen.drain(..indent.len());
    }
    if !ends_with_newline(source, end) && regen.ends_with('\n') {
        regen.pop();
    }

    Ok((start..end, regen, Arc::new(sd_clone)))
}

/// Run an AST mutation against the whole `StoredDefinition` and
/// return a `(0..source.len(), regen)` whole-document patch.
pub fn regenerate_document_patch<F>(
    source: &str,
    parsed: &StoredDefinition,
    mutate: F,
) -> Result<(Range<usize>, String, Arc<StoredDefinition>), AstMutError>
where
    F: FnOnce(&mut StoredDefinition) -> Result<(), AstMutError>,
{
    let mut sd_clone = parsed.clone();
    mutate(&mut sd_clone)?;
    let regen = sd_clone.to_modelica();
    Ok((0..source.len(), regen, Arc::new(sd_clone)))
}

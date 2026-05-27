//! Class definition mutation helpers.

use rumoca_compile::parsing::ast::StoredDefinition;
use super::errors::AstMutError;
use super::util::lookup_class_mut;
use super::parsing::parse_stub_cached;
use crate::pretty;

/// Add a new (empty) class definition inside `parent`.
pub fn add_class(
    sd: &mut StoredDefinition,
    parent: &str,
    name: &str,
    kind: pretty::ClassKindSpec,
    description: &str,
    partial: bool,
) -> Result<(), AstMutError> {
    let stub_text = pretty::class_block_empty(name, kind, description, partial);
    let parsed = parse_stub_cached(&stub_text)
        .ok_or_else(|| AstMutError::ValueParseFailed { value: stub_text.clone() })?;
    let new_class = parsed
        .classes
        .get(name)
        .cloned()
        .ok_or_else(|| AstMutError::ValueParseFailed { value: stub_text.clone() })?;
    if parent.is_empty() {
        if sd.classes.contains_key(name) {
            return Err(AstMutError::DuplicateClass {
                parent: String::from("(top-level)"),
                name: name.to_string(),
            });
        }
        sd.classes.insert(name.to_string(), new_class);
    } else {
        let parent_class = lookup_class_mut(sd, parent)?;
        if parent_class.classes.contains_key(name) {
            return Err(AstMutError::DuplicateClass {
                parent: parent.to_string(),
                name: name.to_string(),
            });
        }
        parent_class.classes.insert(name.to_string(), new_class);
    }
    Ok(())
}

/// Add a short-class definition inside `parent`.
pub fn add_short_class(
    sd: &mut StoredDefinition,
    parent: &str,
    name: &str,
    kind: pretty::ClassKindSpec,
    base: &str,
    prefixes: &[String],
    modifications: &[(String, String)],
) -> Result<(), AstMutError> {
    let stub_text = pretty::short_class_decl(name, kind, base, prefixes, modifications);
    let parsed = parse_stub_cached(&stub_text)
        .ok_or_else(|| AstMutError::ValueParseFailed { value: stub_text.clone() })?;
    let new_class = parsed
        .classes
        .get(name)
        .cloned()
        .ok_or_else(|| AstMutError::ValueParseFailed { value: stub_text.clone() })?;
    if parent.is_empty() {
        if sd.classes.contains_key(name) {
            return Err(AstMutError::DuplicateClass {
                parent: String::from("(top-level)"),
                name: name.to_string(),
            });
        }
        sd.classes.insert(name.to_string(), new_class);
    } else {
        let parent_class = lookup_class_mut(sd, parent)?;
        if parent_class.classes.contains_key(name) {
            return Err(AstMutError::DuplicateClass {
                parent: parent.to_string(),
                name: name.to_string(),
            });
        }
        parent_class.classes.insert(name.to_string(), new_class);
    }
    Ok(())
}

/// Remove a class by qualified path.
pub fn remove_class(sd: &mut StoredDefinition, qualified: &str) -> Result<(), AstMutError> {
    if qualified.is_empty() {
        return Err(AstMutError::ClassNotFound(qualified.to_string()));
    }
    if let Some((parent, leaf)) = qualified.rsplit_once('.') {
        let parent_class = lookup_class_mut(sd, parent)?;
        if parent_class.classes.shift_remove(leaf).is_none() {
            return Err(AstMutError::ClassNotFound(qualified.to_string()));
        }
    } else if sd.classes.shift_remove(qualified).is_none() {
        return Err(AstMutError::ClassNotFound(qualified.to_string()));
    }
    Ok(())
}

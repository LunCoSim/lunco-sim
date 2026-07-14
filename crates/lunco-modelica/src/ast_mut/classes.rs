//! Class definition mutation helpers.

use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};

use super::edit::Edit;
use super::errors::AstMutError;
use super::parsing::parse_stub_cached;
use super::text;
use super::util::lookup_class_mut;
use crate::pretty;

/// Add a new (empty) class definition inside `parent`.
pub fn add_class(
    sd: &mut StoredDefinition,
    edit: &mut Edit<'_>,
    parent: &str,
    name: &str,
    kind: pretty::ClassKindSpec,
    description: &str,
    partial: bool,
) -> Result<(), AstMutError> {
    let rendered = pretty::class_block_empty(name, kind, description, partial);
    insert_class(sd, edit, parent, name, &rendered)
}

/// Add a short-class definition inside `parent`.
pub fn add_short_class(
    sd: &mut StoredDefinition,
    edit: &mut Edit<'_>,
    parent: &str,
    name: &str,
    kind: pretty::ClassKindSpec,
    base: &str,
    prefixes: &[String],
    modifications: &[(String, String)],
) -> Result<(), AstMutError> {
    let rendered = pretty::short_class_decl(name, kind, base, prefixes, modifications);
    insert_class(sd, edit, parent, name, &rendered)
}

/// Splice a rendered class in — at the end of `parent`'s body, or at the end of
/// the document for a top-level class — and mirror it into the AST.
fn insert_class(
    sd: &mut StoredDefinition,
    edit: &mut Edit<'_>,
    parent: &str,
    name: &str,
    rendered: &str,
) -> Result<(), AstMutError> {
    let parsed = parse_stub_cached(rendered).ok_or_else(|| AstMutError::ValueParseFailed {
        value: rendered.to_string(),
    })?;
    let new_class = parsed
        .classes
        .get(name)
        .cloned()
        .ok_or_else(|| AstMutError::ValueParseFailed {
            value: rendered.to_string(),
        })?;

    if parent.is_empty() {
        if sd.classes.contains_key(name) {
            return Err(AstMutError::DuplicateClass {
                parent: String::from("(top-level)"),
                name: name.to_string(),
            });
        }
        // A top-level class goes after the last one in the file. `class_extent`
        // is exact, so trailing content (a `within`, a comment block) is safe.
        let at = sd
            .classes
            .values()
            .filter_map(|c| text::class_extent(edit.source(), c).map(|r| r.end))
            .max()
            .unwrap_or(edit.source().len());
        // A top-level rendered class carries the pretty indent; strip it.
        let body = rendered.trim_start_matches([' ', '\t']);
        edit.insert(at, format!("\n\n{}", body.trim_end_matches('\n')));
        sd.classes.insert(name.to_string(), new_class);
        return Ok(());
    }

    let parent_class = lookup_class_mut(sd, parent)?;
    if parent_class.classes.contains_key(name) {
        return Err(AstMutError::DuplicateClass {
            parent: parent.to_string(),
            name: name.to_string(),
        });
    }
    let at = nested_class_insert_point(edit.source(), parent_class).ok_or_else(|| {
        AstMutError::AnchorNotFound {
            what: format!("body of class `{parent}`"),
        }
    })?;
    edit.insert(at, format!("\n{}", rendered.trim_end_matches('\n')));
    parent_class.classes.insert(name.to_string(), new_class);
    Ok(())
}

/// Where a nested class goes: after the last nested class, else after the last
/// component, else at the top of the parent's body.
fn nested_class_insert_point(source: &str, parent: &ClassDef) -> Option<usize> {
    if let Some(end) = parent
        .classes
        .values()
        .filter_map(|c| text::class_extent(source, c).map(|r| r.end))
        .max()
    {
        return Some(end);
    }
    text::component_insert_point(source, parent)
}

/// Remove a class by qualified path.
pub fn remove_class(
    sd: &mut StoredDefinition,
    edit: &mut Edit<'_>,
    qualified: &str,
) -> Result<(), AstMutError> {
    if qualified.is_empty() {
        return Err(AstMutError::ClassNotFound(qualified.to_string()));
    }
    let local: String =
        crate::diagram::strip_within_prefix(qualified, sd.within.as_ref()).to_string();

    let (parent, leaf) = match local.rsplit_once('.') {
        Some((p, l)) => (Some(p.to_string()), l.to_string()),
        None => (None, local.clone()),
    };

    let target = match &parent {
        Some(p) => lookup_class_mut(sd, p)?
            .classes
            .get(&leaf)
            .ok_or_else(|| AstMutError::ClassNotFound(qualified.to_string()))?,
        None => sd
            .classes
            .get(&leaf)
            .ok_or_else(|| AstMutError::ClassNotFound(qualified.to_string()))?,
    };

    let extent =
        text::class_extent(edit.source(), target).ok_or_else(|| AstMutError::AnchorNotFound {
            what: format!("definition of class `{qualified}`"),
        })?;
    edit.delete(text::line_extent(edit.source(), extent));

    match &parent {
        Some(p) => {
            lookup_class_mut(sd, p)?.classes.shift_remove(&leaf);
        }
        None => {
            sd.classes.shift_remove(&leaf);
        }
    }
    Ok(())
}

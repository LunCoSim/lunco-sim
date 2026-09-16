//! Name lookup over one parsed Modelica stored definition.

use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};

/// Find a class by its qualified or local name in one stored definition.
///
/// A dotted name is walked through nested class definitions. A `within`
/// prefix is tolerated because callers may hold the fully qualified library
/// name while the parsed file stores only its local root.
pub fn find_class_by_qualified_name<'a>(
    ast: &'a StoredDefinition,
    name: &str,
) -> Option<&'a ClassDef> {
    if !name.contains('.') {
        if let Some(class) = ast.classes.get(name) {
            return Some(class);
        }
        for class in ast.classes.values() {
            if let Some(nested) = class.classes.get(name) {
                return Some(nested);
            }
        }
        return None;
    }

    let path = lunco_modelica_ast::strip_within_prefix(name, ast.within.as_ref());
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut current = ast.classes.get(first)?;
    for segment in segments {
        current = current.classes.get(segment)?;
    }
    Some(current)
}

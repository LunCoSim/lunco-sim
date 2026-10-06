//! Modelica source-library document construction.
//!
//! Library provisioning and parsed-bundle access belong to the compiler
//! integration. This module adapts those resources into a read-only document
//! for the UI's drill-in flow; the headless document package remains unaware
//! of library storage and compiler caches.

use std::path::Path;

use lunco_doc::{DocumentId, DocumentOrigin};
use lunco_modelica_document::ModelicaDocument;
use rumoca_compile::parsing::ast::StoredDefinition;

/// Read one source-library class with immutable file admission, then construct
/// its read-only document and exact runtime owner.
///
/// The parsed source bundle is preferred because it avoids reparsing a large
/// package wrapper for every drill-in. Native can repair a bundle miss by
/// parsing the source once; WebAssembly reports the missing bundle so the
/// worker-owned loader can finish and the caller can retry visibly.
pub async fn load_library_class(
    id: DocumentId,
    path: &Path,
    qualified: &str,
    admission: lunco_workspace::FileDocumentAdmission,
) -> Result<(ModelicaDocument, lunco_workspace::DocumentRuntimeOwner), String> {
    #[cfg(target_arch = "wasm32")]
    lunco_modelica_library::source_library::ensure_library_source_unpacked();

    let (resolved, full_source) =
        if let Some(bytes) = lunco_assets_runtime::library::library_read(path) {
            (
                lunco_workspace::ResolvedFileDocument {
                    path: path.to_path_buf(),
                    runtime: lunco_workspace::DocumentRuntimeOwner::Application,
                },
                String::from_utf8(bytes).map_err(|e| format!("non-UTF-8 source: {e}"))?,
            )
        } else {
            lunco_modelica_runtime::source_asset::read_admitted_file(path, admission).await?
        };
    let path = resolved.path.as_path();
    let key = path.to_string_lossy().to_string();
    let bundled_ast =
        lunco_modelica_library::source_library::parsed_source_bundle().and_then(|bundle| {
            bundle
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, ast)| ast.clone())
        });
    let bundle_hit = bundled_ast.is_some();

    if !bundle_hit {
        bevy::log::warn!(
            "[load_library_class] parsed-bundle MISS for `{key}` ({} bytes) — native source repair or worker retry required",
            full_source.len()
        );
    }

    #[cfg(target_arch = "wasm32")]
    let ast: StoredDefinition = bundled_ast.ok_or_else(|| {
        format!(
            "parsed source library AST unavailable for `{key}`; wait for the Web Worker source library load and retry"
        )
    })?;
    #[cfg(not(target_arch = "wasm32"))]
    let ast: StoredDefinition = match bundled_ast {
        Some(ast) => ast,
        None => lunco_modelica_ast::parse_to_ast(&full_source, &key)
            .map_err(|e| format!("parse failed `{}`: {e}", path.display()))?,
    };

    let source = selected_class_source(&ast, &full_source, qualified)
        .map_err(|error| format!("{error} in `{}` (bundle_hit={bundle_hit})", path.display()))?;
    Ok((
        ModelicaDocument::with_origin(
            id,
            source,
            DocumentOrigin::File {
                path: path.to_path_buf(),
                writable: false,
            },
        ),
        resolved.runtime,
    ))
}

fn selected_class_source(
    ast: &StoredDefinition,
    full_source: &str,
    qualified: &str,
) -> Result<String, String> {
    let class_def =
        lunco_modelica_index::class_lookup::find_class_by_qualified_name(ast, qualified)
            .ok_or_else(|| format!("class `{qualified}` not found"))?;
    let (full_start, full_end) =
        lunco_modelica_ast::ast_extract::class_full_text_span(class_def, &full_source);
    if full_start > full_end
        || full_end > full_source.len()
        || !full_source.is_char_boundary(full_start)
        || !full_source.is_char_boundary(full_end)
    {
        return Err(format!(
            "class `{qualified}` span {full_start}..{full_end} invalid for source of {} bytes",
            full_source.len(),
        ));
    }

    let class_slice = &full_source[full_start..full_end];
    let parent_pkg = lunco_modelica_ast::ast_extract::parent_qualified(qualified);
    Ok(if parent_pkg.is_empty() {
        class_slice.to_string()
    } else {
        format!("within {parent_pkg};\n{class_slice}")
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn library_class_selection_requires_exact_qualified_identity() {
        let source = "package Root package A model Part Real fromA; end Part; end A; package B model Part Real fromB; end Part; end B; end Root;";
        let syntax = lunco_modelica_ast::parse_to_syntax(source, "siblings.mo");
        let ast = syntax.parsed().expect("valid inline source");
        let selected =
            super::selected_class_source(ast, source, "Root.B.Part").expect("exact class");
        assert!(selected.starts_with("within Root.B;\nmodel Part"));
        assert!(selected.contains("fromB"));
        assert!(!selected.contains("fromA"));
        assert!(!lunco_modelica_ast::parse_to_syntax(&selected, "selected.mo").has_errors());
        assert!(super::selected_class_source(ast, source, "Root.C.Part").is_err());
        let source = "within Root.B; model Part Real fromB; end Part;";
        let syntax = lunco_modelica_ast::parse_to_syntax(source, "within.mo");
        let ast = syntax.parsed().expect("valid inline source");
        assert!(super::selected_class_source(ast, source, "Root.B.Part").is_ok());
        assert!(super::selected_class_source(ast, source, "Root.BPart").is_err());
    }
}

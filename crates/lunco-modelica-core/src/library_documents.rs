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

/// Build a read-only document containing one class from a source-library file.
///
/// The parsed source bundle is preferred because it avoids reparsing a large
/// package wrapper for every drill-in. Native can repair a bundle miss by
/// parsing the source once; WebAssembly reports the missing bundle so the
/// worker-owned loader can finish and the caller can retry visibly.
pub fn load_library_class(
    id: DocumentId,
    path: &Path,
    qualified: &str,
) -> Result<ModelicaDocument, String> {
    #[cfg(target_arch = "wasm32")]
    crate::library_remote::ensure_library_source_unpacked();

    let full_source = if let Some(bytes) = lunco_assets_core::library::library_read(path) {
        String::from_utf8(bytes)
            .map_err(|e| format!("non-utf8 source `{}`: {e}", path.display()))?
    } else {
        lunco_modelica_runtime::source_asset::read_text_sync(path)?
    };

    let short_name = qualified.rsplit('.').next().unwrap_or(qualified);
    let parent_pkg = qualified.rsplit_once('.').map_or("", |(parent, _)| parent);
    let key = path.to_string_lossy().to_string();
    let bundled_ast = crate::library_remote::parsed_source_bundle().and_then(|bundle| {
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

    let class_def = lunco_modelica_ast::ast_extract::find_class_by_short_name(&ast, short_name)
        .ok_or_else(|| format!("class `{qualified}` not found in `{}`", path.display()))?;
    let (full_start, full_end) =
        lunco_modelica_ast::ast_extract::class_full_text_span(class_def, &full_source);
    if full_start > full_end
        || full_end > full_source.len()
        || !full_source.is_char_boundary(full_start)
        || !full_source.is_char_boundary(full_end)
    {
        return Err(format!(
            "class `{qualified}` span {full_start}..{full_end} invalid for source of {} bytes in `{}` (bundle_hit={bundle_hit}) — likely a stale/mismatched parsed bundle or wrong-file resolution",
            full_source.len(),
            path.display()
        ));
    }

    let class_slice = &full_source[full_start..full_end];
    let source = if parent_pkg.is_empty() {
        class_slice.to_string()
    } else {
        format!("within {parent_pkg};\n{class_slice}")
    };
    Ok(ModelicaDocument::with_origin(
        id,
        source,
        DocumentOrigin::File {
            path: path.to_path_buf(),
            writable: false,
        },
    ))
}

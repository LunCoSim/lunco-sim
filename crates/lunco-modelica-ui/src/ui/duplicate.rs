//! Pure parsing/text helpers for the class-duplication flow.
//!
//! Admitted tasks extract the exact qualified class, collect its enclosing
//! package imports, and rewrite the captured source with a new name. Native
//! tasks use the task pool; browser scheduling does not imply another thread.
//!
//! Scheduling and publication stay in `ui::commands` and the document loader.

/// Class-name and end-token byte spans, plus the parsed declaration slice.
/// All offsets refer to the exact source given to `parse_to_syntax`.
/// `rewrite_inject_in_one_pass`
/// re-anchors them against its `src` slice (the caller passes
/// `source[full_start..full_end]`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateExtract {
    /// Declaration slice within the source, from `class_full_text_span`.
    pub full_start: usize,
    pub full_end: usize,
    /// Class-name-token span (absolute in source).
    pub name_start: usize,
    pub name_end: usize,
    /// `end Name` token span (absolute in source).
    pub end_start: usize,
    pub end_end: usize,
}

/// Parse the exact source snapshot and select the supplied qualified class.
pub(crate) fn extract_class_spans_inline(
    source: &str,
    class_name: &str,
) -> Option<DuplicateExtract> {
    let syntax = lunco_modelica_ast::parse_to_syntax(source, "duplicate-inline.mo");
    if syntax.has_errors() {
        return None;
    }
    spans_from_ast(syntax.parsed()?, source, class_name)
}

pub(crate) fn spans_from_ast(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
    source: &str,
    class_name: &str,
) -> Option<DuplicateExtract> {
    let class = lunco_modelica_index::class_lookup::find_class_by_qualified_name(ast, class_name)?;
    let end_tok = class.end_name_token.as_ref()?;
    // rumoca's `ClassDef.location` spans only NAME → `end <Name>`, omitting
    // the prefix keyword and the trailing `;`. `class_full_text_span` widens
    // it to the real declaration bounds (the canonical helper, shared with
    // `load_library_class`). `rewrite_inject_in_one_pass` re-anchors these
    // absolute spans by `full_start`, so the caller must pass the matching
    // `source[full_start..full_end]` slice.
    let (full_start, full_end) =
        lunco_modelica_ast::ast_extract::class_full_text_span(class, source);
    Some(DuplicateExtract {
        full_start,
        full_end,
        name_start: class.name.location.start as usize,
        name_end: class.name.location.end as usize,
        end_start: end_tok.location.start as usize,
        end_end: end_tok.location.end as usize,
    })
}

/// Walk from a class file's directory up through the filesystem,
/// collecting `import` statements from every `package.mo` on the
/// way. These are the imports that were in scope for the class at
/// its original location — once the class is extracted into a
/// standalone workspace file, it loses that scope, so the imports
/// must be injected into the class body itself (Modelica allows
/// class-local imports).
///
/// Stops walking as soon as a directory has no `package.mo` — that
/// marks the boundary of the enclosing package hierarchy. Returns
/// imports in outer-to-inner order, deduplicated while preserving
/// first-seen position.
///
/// Covers the SI/unit shortcuts that break duplication of source library
/// examples: e.g. `Modelica/Blocks/package.mo` declares
/// `import Modelica.Units.SI;` which is why `SI.Angle` resolves
/// inside `Modelica.Blocks.Examples.PID_Controller` but not in a
/// naïvely extracted copy.
pub(crate) fn collect_parent_imports(class_file: &std::path::Path) -> Result<Vec<String>, String> {
    // Browser package imports are supplied by the compiler's resident library bundle.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = class_file;
        Ok(Vec::new())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut chain = Vec::new();
        let mut dir = class_file.parent();
        while let Some(parent) = dir {
            let package = parent.join("package.mo");
            match lunco_storage::entry_kind_file_sync(&package) {
                Err(lunco_storage::StorageError::NotFound) => break,
                Ok(lunco_storage::StorageEntryKind::File) => {}
                Ok(kind) => {
                    return Err(format!(
                        "enclosing package {} is {kind:?}, not a file",
                        package.display()
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "cannot inspect enclosing package {}: {error}",
                        package.display()
                    ));
                }
            }
            // AST spans always refer to these exact admitted bytes, never a second path read.
            let source = lunco_modelica_runtime::source_asset::read_text_sync(&package).map_err(
                |error| {
                    format!(
                        "cannot read enclosing package {}: {error}",
                        package.display()
                    )
                },
            )?;
            let mut level = package_imports(&source)
                .map_err(|error| format!("enclosing package {}: {error}", package.display()))?;
            level.append(&mut chain);
            chain = level;
            dir = parent.parent();
        }
        let mut seen = std::collections::HashSet::new();
        chain.retain(|import| seen.insert(import.clone()));
        Ok(chain)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn package_imports(source: &str) -> Result<Vec<String>, String> {
    let syntax = lunco_modelica_ast::parse_to_syntax(source, "duplicate-package.mo");
    if syntax.has_errors() {
        return Err("package has syntax errors".into());
    }
    let ast = syntax.parsed().ok_or("package has no valid syntax")?;
    let class = ast
        .classes
        .values()
        .next()
        .ok_or("package contains no class")?;
    let mut imports = Vec::new();
    for import in &class.imports {
        use rumoca_compile::parsing::ast::Import;
        let location = match import {
            Import::Qualified { location, .. }
            | Import::Renamed { location, .. }
            | Import::Unqualified { location, .. }
            | Import::Selective { location, .. } => location,
        };
        let mut text = source
            .get(location.start as usize..location.end as usize)
            .ok_or("package has invalid import spans")?
            .trim()
            .to_owned();
        if !text.ends_with(';') {
            text.push(';');
        }
        imports.push(text);
    }
    Ok(imports)
}

/// Rename and inject imports using spans parsed from the exact source snapshot.
/// Invalid or non-UTF-8-boundary spans reject the rewrite without slicing.
pub(crate) fn rewrite_inject_in_one_pass(
    src: &str,
    new_name: &str,
    imports: &[String],
    spans: &DuplicateExtract,
) -> Option<String> {
    // Spans are absolute in the original file. Re-anchor against the
    // class-only `src` slice (caller passes `source[full_start..full_end]`).
    let base = spans.full_start;
    let name_start = spans.name_start.checked_sub(base)?;
    let name_end = spans.name_end.checked_sub(base)?;
    let end_start = spans.end_start.checked_sub(base)?;
    let end_end = spans.end_end.checked_sub(base)?;
    if !(name_start <= name_end
        && name_end <= end_start
        && end_start <= end_end
        && end_end <= src.len())
    {
        return None;
    }
    // Invalid byte boundaries reject the rewrite before any source slicing.
    for &idx in &[name_start, name_end, end_start, end_end] {
        if !src.is_char_boundary(idx) {
            bevy::log::warn!(
                "[rewrite_inject_in_one_pass] span index {idx} not on char \
                 boundary in {}-byte source; skipping rewrite",
                src.len()
            );
            return None;
        }
    }

    // Inject anchor: position in `src` immediately after the class
    // name's optional description string(s).
    let bytes = src.as_bytes();
    let skip_ws = |mut i: usize| {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    let mut anchor = name_end;
    let mut scan = skip_ws(anchor);
    while scan < bytes.len() && bytes[scan] == b'"' {
        let mut j = scan + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' if j + 1 < bytes.len() => j += 2,
                b'"' => {
                    j += 1;
                    break;
                }
                _ => j += 1,
            }
        }
        anchor = j;
        scan = skip_ws(j);
    }
    if anchor > end_start {
        return None;
    }
    let want_inject = !imports.is_empty();
    let inject_block: String = if want_inject {
        imports.iter().map(|i| format!("  {i}\n")).collect()
    } else {
        String::new()
    };

    let mut out = String::with_capacity(src.len() + inject_block.len() + 4);
    out.push_str(&src[..name_start]);
    // Replace class name.
    out.push_str(new_name);
    // Description / whitespace between class name and inject anchor.
    out.push_str(&src[name_end..anchor]);
    if want_inject {
        let needs_leading_newline = !out.ends_with('\n');
        if needs_leading_newline {
            out.push('\n');
        }
        out.push_str(&inject_block);
    }
    // Body from inject anchor up to end-token.
    out.push_str(&src[anchor..end_start]);
    // Replace end-token name.
    out.push_str(new_name);
    // Tail.
    out.push_str(&src[end_end..]);
    Some(out)
}

/// Extract and rename one class. Missing or invalid spans are terminal errors;
/// the source package scope is retained through its explicit `within` clause.
pub(crate) fn build_duplicate_source(
    source: &str,
    spans: Option<&DuplicateExtract>,
    new_name: &str,
    origin_fqn: Option<&str>,
    imports: &[String],
) -> Result<String, String> {
    let spans =
        spans.ok_or_else(|| "duplicate source contains no valid selected class".to_owned())?;
    let slice = source
        .get(spans.full_start..spans.full_end)
        .ok_or_else(|| "duplicate source has invalid class spans".to_owned())?;
    let renamed = rewrite_inject_in_one_pass(slice, new_name, imports, spans)
        .ok_or_else(|| "duplicate source has invalid rewrite spans".to_owned())?;
    // Keep the `within` clause: it gives the copied body the origin package's
    // lexical scope (e.g. the `SI` unit alias the source library examples rely on), which
    // a top-level lift would lose — `unresolved type reference: 'SI.Angle'`.
    // The cost is that the copy's real class name is `<origin_pkg>.<new_name>`,
    // so the run/compile path must dispatch that QUALIFIED name (see
    // `within_package_of_source` + its use in `dispatch_experiment`); dispatching the bare
    // leaf fails `model not found` in Instantiate.
    let source = match origin_fqn {
        Some(fqn) => {
            let origin_pkg = lunco_modelica_ast::ast_extract::parent_qualified(fqn);
            if origin_pkg.is_empty() {
                renamed
            } else {
                format!("within {origin_pkg};\n{renamed}")
            }
        }
        None => renamed,
    };
    let syntax = lunco_modelica_ast::parse_to_syntax(&source, "duplicate-result.mo");
    if syntax.has_errors() || syntax.parsed().is_none() {
        return Err("rewritten duplicate contains invalid Modelica syntax".to_owned());
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact oracle the engine uses to gate diagram projection: the
    /// lenient parser's recovery error set. `parse_to_ast(..).is_ok()`
    /// would lie — rumoca recovers from errors and still returns a tree.
    fn parses_clean(src: &str) -> bool {
        !lunco_modelica_ast::parse_to_syntax(src, "dup-test.mo").has_errors()
    }

    /// Mirror the read-only duplicate flow: extract spans from the full
    /// source, then build the duplicate source.
    fn duplicate(source: &str, origin_short: &str, new_name: &str, fqn: Option<&str>) -> String {
        let spans = extract_class_spans_inline(source, fqn.unwrap_or(origin_short));
        build_duplicate_source(source, spans.as_ref(), new_name, fqn, &[])
            .expect("valid inline duplicate")
    }

    #[test]
    fn duplicate_rejects_missing_and_malformed_selected_class() {
        for (source, selected) in [
            ("model Source end Source;", "Missing"),
            ("model Source Real ; end Source;", "Source"),
        ] {
            let spans = extract_class_spans_inline(source, selected);
            assert!(
                build_duplicate_source(source, spans.as_ref(), "Copy", Some(selected), &[])
                    .is_err()
            );
        }
    }

    #[test]
    fn duplicate_rejects_invalid_spans_and_destination_name() {
        let source = "// λ
model Source Real x; end Source;";
        let valid = extract_class_spans_inline(source, "Source").expect("valid inline source");
        let mut reversed = valid;
        reversed.name_start = reversed.name_end + 1;
        assert!(
            build_duplicate_source(source, Some(&reversed), "Copy", Some("Source"), &[]).is_err()
        );
        let mut split_unicode = valid;
        split_unicode.full_start = source.find('λ').unwrap() + 1;
        assert!(
            build_duplicate_source(source, Some(&split_unicode), "Copy", Some("Source"), &[])
                .is_err()
        );
        assert!(
            build_duplicate_source(source, Some(&valid), "Bad Name", Some("Source"), &[]).is_err()
        );
    }

    #[test]
    fn duplicate_selects_exact_qualified_sibling() {
        let source = "package Root package A model Part Real fromA; end Part; end A; package B model Part Real fromB; end Part; end B; end Root;";
        let output = duplicate(source, "Part", "Copy", Some("Root.B.Part"));
        assert!(output.contains("within Root.B;"));
        assert!(output.contains("Real fromB;"));
        assert!(!output.contains("fromA"));
        let spans = extract_class_spans_inline(source, "Root.C.Part");
        assert!(
            build_duplicate_source(source, spans.as_ref(), "Copy", Some("Root.C.Part"), &[])
                .is_err()
        );
        let within = "within Root.B; model Part Real fromB; end Part;";
        let output = duplicate(within, "Part", "Copy", Some("Root.B.Part"));
        assert!(output.contains("within Root.B;"));
        assert!(output.contains("model Copy"));
    }

    #[test]
    fn duplicate_preserves_quoted_identifier_syntax() {
        let source = "model 'Part Name' Real value; end 'Part Name';";
        let output = duplicate(
            source,
            "'Part Name'",
            "'Part NameCopy'",
            Some("'Part Name'"),
        );
        assert!(output.contains("model 'Part NameCopy'"));
        assert!(output.contains("end 'Part NameCopy';"));
        assert!(parses_clean(&output));
        let source = "package Root model 'Part.Name' Real value; end 'Part.Name'; end Root;";
        let output = duplicate(
            source,
            "'Part.Name'",
            "'Part.NameCopy'",
            Some("Root.'Part.Name'"),
        );
        assert!(output.starts_with("within Root;"));
        assert!(output.contains("model 'Part.NameCopy'"));
        assert!(output.contains("end 'Part.NameCopy';"));
        assert!(parses_clean(&output));
        let source = "within Root; model 'Part.Name' Real value; end 'Part.Name';";
        let output = duplicate(
            source,
            "'Part.Name'",
            "'Part.NameCopy'",
            Some("Root.'Part.Name'"),
        );
        assert!(output.starts_with("within Root;"));
        assert!(parses_clean(&output));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn duplicate_package_imports_use_exact_bytes_and_reject_invalid_source() {
        let imports =
            package_imports("package P import Alias = Other.Value; import Other.*; end P;")
                .expect("valid package");
        assert_eq!(
            imports,
            vec!["import Alias = Other.Value;", "import Other.*;"]
        );
        assert!(package_imports("package P import ; end P;").is_err());
        assert!(package_imports("// missing package").is_err());
    }

    #[test]
    fn duplicate_package_with_leading_comment_header_parses() {
        // Multibyte banner text lies outside the declaration's absolute spans.
        let src = "\
// banner line one
// banner line two ──►│  (multibyte, lives before full_start)
package Foo
  model Bar
    Real x;
  equation
    x = 1;
  end Bar;
end Foo;
";
        let out = duplicate(src, "Foo", "FooCopy", None);
        assert!(parses_clean(&out), "renamed source must parse:\n{out}");
        assert!(out.contains("package FooCopy"), "header renamed:\n{out}");
        assert!(out.contains("end FooCopy;"), "end token renamed:\n{out}");
        assert!(out.contains("model Bar"), "nested class preserved:\n{out}");
    }

    #[test]
    fn duplicate_nested_composite_model_keeps_within_scope() {
        // Keep this fixture small and local. The production bundled example is
        // exercised through its authored runtime scene; this test only pins the
        // source-preserving duplicate mechanism and its nested-package shape.
        let src = "\
// multibyte banner ──►│ before the declaration
package CompositeFixture
  model RocketStage
    Tank tank;
    Valve valve;
  equation
    tank.m = 1;
    valve.opening = 0;
  end RocketStage;
  model Tank
    Real m;
  end Tank;
  model Valve
    input Real opening;
  end Valve;
end CompositeFixture;
";
        let out = duplicate(
            src,
            "RocketStage",
            "RocketStageCopy",
            Some("CompositeFixture.RocketStage"),
        );
        assert!(
            parses_clean(&out),
            "nested-model duplicate must parse:\n{out}"
        );
        assert!(out.contains("model RocketStageCopy"), "renamed:\n{out}");
        assert!(out.contains("end RocketStageCopy;"), "end renamed:\n{out}");
        assert!(out.starts_with("within CompositeFixture;"));
        assert!(out.contains("Tank tank;"), "sibling reference preserved");
        assert_eq!(
            lunco_modelica_ast::ast_extract::within_package_of_source(&out).as_deref(),
            Some("CompositeFixture")
        );
    }

    #[test]
    fn duplicate_flat_model_keeps_keyword_and_semicolon() {
        // The core regression isolated: `ClassDef.location` omits the
        // `model` keyword and the trailing `;`. Pre-fix this produced
        // `BallCopy … end BallCopy` (no keyword, no semicolon).
        let src = "model Ball\n  Real h;\nequation\n  h = 1;\nend Ball;\n";
        let out = duplicate(src, "Ball", "BallCopy", None);
        assert!(parses_clean(&out), "must parse:\n{out}");
        assert!(out.contains("model BallCopy"), "keyword kept:\n{out}");
        assert!(out.contains("end BallCopy;"), "semicolon kept:\n{out}");
    }

    #[test]
    fn duplicate_partial_connector_keeps_qualifier() {
        // `class_full_text_span` rewinds over the `partial` qualifier too,
        // so the whole keyword chain survives the rename.
        let src = "partial connector Pin\n  Real v;\n  flow Real i;\nend Pin;\n";
        let out = duplicate(src, "Pin", "PinCopy", None);
        assert!(parses_clean(&out), "must parse:\n{out}");
        assert!(
            out.contains("partial connector PinCopy"),
            "qualifier + keyword kept:\n{out}"
        );
        assert!(out.contains("end PinCopy;"));
    }
}

//! Pure parsing/text helpers for the class-duplication flow.
//!
//! Admitted tasks rename the exact qualified class and preserve its package
//! scope through `within`. Rumoca resolves the admitted package context. Native
//! tasks use the task pool; browser scheduling does not imply another thread.
//!
//! Scheduling and publication stay in `ui::commands` and the document loader.

/// Class-name and end-token byte spans, plus the parsed declaration slice.
/// All offsets refer to the exact source given to `parse_to_syntax`.
/// `rename_class_tokens`
/// re-anchors them against its `src` slice (the caller passes
/// `source[full_start..full_end]`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateExtract {
    /// Complete declaration slice from Rumoca's source location.
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
    let syntax = rumoca_phase_parse::parse_to_syntax(source, "duplicate-inline.mo");
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
    let class = ast.get_class_by_qualified_name(class_name)?;
    let end_tok = class.end_name_token.as_ref()?;
    let full_start = class.location.start as usize;
    let full_end = class.location.end as usize;
    source.get(full_start..full_end)?;
    Some(DuplicateExtract {
        full_start,
        full_end,
        name_start: class.name.location.start as usize,
        name_end: class.name.location.end as usize,
        end_start: end_tok.location.start as usize,
        end_end: end_tok.location.end as usize,
    })
}

/// Rename the two name tokens using spans from the exact source snapshot.
/// Invalid or non-UTF-8-boundary spans reject the rewrite without slicing.
pub(crate) fn rename_class_tokens(
    src: &str,
    new_name: &str,
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
                "[rename_class_tokens] span index {idx} not on char \
                 boundary in {}-byte source; skipping rewrite",
                src.len()
            );
            return None;
        }
    }

    let mut out = String::with_capacity(src.len() + new_name.len() * 2);
    out.push_str(&src[..name_start]);
    out.push_str(new_name);
    out.push_str(&src[name_end..end_start]);
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
) -> Result<String, String> {
    let spans =
        spans.ok_or_else(|| "duplicate source contains no valid selected class".to_owned())?;
    let slice = source
        .get(spans.full_start..spans.full_end)
        .ok_or_else(|| "duplicate source has invalid class spans".to_owned())?;
    let renamed = rename_class_tokens(slice, new_name, spans)
        .ok_or_else(|| "duplicate source has invalid rewrite spans".to_owned())?;
    // Preserve the origin package's lexical scope. The document index and
    // run-target resolver retain the copied class's qualified identity.
    let source = match origin_fqn {
        Some(fqn) => {
            let origin_pkg = rumoca_core::parent_scope(fqn).unwrap_or("");
            if origin_pkg.is_empty() {
                renamed
            } else {
                format!("within {origin_pkg};\n{renamed}")
            }
        }
        None => renamed,
    };
    let syntax = rumoca_phase_parse::parse_to_syntax(&source, "duplicate-result.mo");
    if syntax.has_errors() || syntax.parsed().is_none() {
        return Err("rewritten duplicate contains invalid Modelica syntax".to_owned());
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gate diagram projection on the recovering syntax diagnostics.
    fn parses_clean(src: &str) -> bool {
        !rumoca_phase_parse::parse_to_syntax(src, "dup-test.mo").has_errors()
    }

    /// Mirror the read-only duplicate flow: extract spans from the full
    /// source, then build the duplicate source.
    fn duplicate(source: &str, origin_short: &str, new_name: &str, fqn: Option<&str>) -> String {
        let spans = extract_class_spans_inline(source, fqn.unwrap_or(origin_short));
        build_duplicate_source(source, spans.as_ref(), new_name, fqn)
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
                build_duplicate_source(source, spans.as_ref(), "Copy", Some(selected)).is_err()
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
        assert!(build_duplicate_source(source, Some(&reversed), "Copy", Some("Source")).is_err());
        let mut split_unicode = valid;
        split_unicode.full_start = source.find('λ').unwrap() + 1;
        assert!(
            build_duplicate_source(source, Some(&split_unicode), "Copy", Some("Source")).is_err()
        );
        assert!(build_duplicate_source(source, Some(&valid), "Bad Name", Some("Source")).is_err());
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
            build_duplicate_source(source, spans.as_ref(), "Copy", Some("Root.C.Part")).is_err()
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
        let ast = rumoca_phase_parse::parse_to_ast(&out, "duplicate-fixture.mo").unwrap();
        assert_eq!(
            lunco_modelica_ast::ast_extract::within_package(&ast).as_deref(),
            Some("CompositeFixture")
        );
    }

    #[test]
    fn duplicate_flat_model_keeps_keyword_and_semicolon() {
        // The complete declaration survives the token-span rename.
        let src = "model Ball\n  Real h;\nequation\n  h = 1;\nend Ball;\n";
        let out = duplicate(src, "Ball", "BallCopy", None);
        assert!(parses_clean(&out), "must parse:\n{out}");
        assert!(out.contains("model BallCopy"), "keyword kept:\n{out}");
        assert!(out.contains("end BallCopy;"), "semicolon kept:\n{out}");
    }

    #[test]
    fn duplicate_partial_connector_keeps_qualifier() {
        // The declaration location includes the partial qualifier.
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

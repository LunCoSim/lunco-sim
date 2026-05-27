//! Pure parsing/text helpers for the class-duplication flow.
//!
//! These run on the bg thread of `spawn_duplicate_class_task` (and the
//! UI-side `on_duplicate_model_from_read_only` observer) to extract a
//! single named class out of a (possibly multi-class) `.mo` file,
//! collect its in-scope imports from the enclosing package chain, and
//! rewrite the slice with a new name + injected imports — all in one
//! pass, no Bevy / no `World` access.
//!
//! Lives under `document::` because it operates on Modelica source +
//! AST only; the world-mut orchestration that schedules the task and
//! wires its output into tabs stays in `ui::commands`.

/// Class-name + end-token byte spans, plus the full class slice (with
/// leading comments). All offsets are absolute in the source `parse_to_ast`
/// / `parse_files_parallel` was given. `rewrite_inject_in_one_pass`
/// re-anchors them against its `src` slice (the caller passes
/// `source[full_start..full_end]`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateExtract {
    /// Class slice within the source (full_span_with_leading_comments).
    pub full_start: usize,
    pub full_end: usize,
    /// Class-name-token span (absolute in source).
    pub name_start: usize,
    pub name_end: usize,
    /// `end Name` token span (absolute in source).
    pub end_start: usize,
    pub end_end: usize,
}

/// Look up a class's `(start, end)` byte range in the source from the
/// parsed AST. Walks `ast.classes` recursively (top-level packages
/// often contain the class we're after as a nested entry, e.g.
/// `Modelica.Blocks.Continuous` → `LimPID`). The match is by short
/// name — first hit wins, which is fine in practice since MSL keeps
/// short names unique within a package.
///
/// Replaces an earlier regex-on-text approach that mis-extracted when
/// the source contained a docstring with a literal `block <Name>` line.
/// The AST has no such hazard.
/// Path-aware variant that also returns the class-name-token span and
/// the end-token span (both **absolute** in `source`), so the bg
/// duplicate flow can splice without re-parsing the same bytes a
/// second time.
pub(crate) fn extract_class_spans_via_path(
    path: &std::path::Path,
    source: &str,
    class_name: &str,
) -> Option<DuplicateExtract> {
    // `parse_files_parallel` resolves a per-file artifact cache rooted
    // under `std::env::temp_dir()`, which on wasm32-unknown-unknown
    // panics with "no filesystem on this platform" — `temp_dir()`'s
    // libstd stub is fatal there. On wasm we already have the source
    // bytes in memory (caller fetched them from the in-memory MSL
    // bundle), so the cache buys us nothing; parse the in-memory
    // source directly via `parse_to_ast`, same `StoredDefinition`,
    // no fs touch.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        return extract_class_spans_inline(source, class_name);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut parsed =
            rumoca_compile::parsing::parse_files_parallel(&[path.to_path_buf()]).ok()?;
        let (_uri, ast) = parsed.drain(..).next()?;
        spans_from_ast(&ast, source, class_name)
    }
}

/// In-memory variant: parses `source` directly (no path / cache) and
/// returns the splice spans needed by `rewrite_inject_in_one_pass`.
/// Use when the caller has source text but no on-disk URI — e.g.,
/// duplicating a workspace doc whose source lives in
/// `ModelicaDocumentRegistry`.
pub(crate) fn extract_class_spans_inline(
    source: &str,
    class_name: &str,
) -> Option<DuplicateExtract> {
    let ast = rumoca_phase_parse::parse_to_ast(source, "duplicate-inline.mo").ok()?;
    spans_from_ast(&ast, source, class_name)
}

pub(crate) fn spans_from_ast(
    ast: &rumoca_compile::parsing::ast::StoredDefinition,
    source: &str,
    class_name: &str,
) -> Option<DuplicateExtract> {
    let class = crate::ast_extract::find_class_by_short_name(ast, class_name)?;
    // `full_span_with_leading_comments` removed in rumoca main;
    // fall back to the class location span.
    let full_start = class.location.start as usize;
    let full_end = class.location.end as usize;
    let end_tok = class.end_name_token.as_ref()?;
    Some(DuplicateExtract {
        full_start,
        full_end,
        name_start: class.name.location.start as usize,
        name_end: class.name.location.end as usize,
        end_start: end_tok.location.start as usize,
        end_end: end_tok.location.end as usize,
    })
}

// Class-by-short-name lookup lives in `crate::ast_extract::find_class_by_short_name`.
// Previously duplicated here as `find_top_or_nested_class_by_short_name` +
// `find_nested_by_short_name`; collapsed to the canonical helper so the
// three short-name lookups can't silently disagree (same shape as the
// `walk_qualified` / `find_class_by_qualified_name` bug).

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
/// Covers the SI/unit shortcuts that break duplication of MSL
/// examples: e.g. `Modelica/Blocks/package.mo` declares
/// `import Modelica.Units.SI;` which is why `SI.Angle` resolves
/// inside `Modelica.Blocks.Examples.PID_Controller` but not in a
/// naïvely extracted copy.
pub(crate) fn collect_parent_imports(class_file: &std::path::Path) -> Vec<String> {
    // Wasm has no filesystem, and the MSL bundle is pre-parsed and
    // already in `GLOBAL_PARSED_MSL` with all its imports. The
    // parent-walk + `read_to_string(<relative>)` chain panics on
    // wasm32-unknown-unknown ("no filesystem on this platform")
    // because libstd resolves relative paths through `current_dir()`.
    // No-op on web; rumoca's session-level resolver fills the same
    // role.
    #[cfg(target_arch = "wasm32")]
    {
        let _ = class_file;
        return Vec::new();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut chain: Vec<String> = Vec::new();
        let mut dir = class_file.parent();
        while let Some(d) = dir {
            let pkg = d.join("package.mo");
            if !pkg.exists() {
                break;
            }
            // Parse the package.mo and walk the outer package class's
            // typed `imports` list. Nested-class imports stay scoped to
            // their own ClassDef.imports — only the package preamble's
            // imports leak into duplicated children, matching the prior
            // regex's "first opener through second opener" boundary.
            // `parse_files_parallel` hits rumoca's content-hash artifact
            // cache, so walking up a deep MSL hierarchy is cheap on
            // repeat duplications.
            let pairs = if std::env::var_os("LUNCO_NO_PARSE").is_some() {
                None
            } else {
                rumoca_compile::parsing::parse_files_parallel(&[pkg.clone()]).ok()
            };
            if let Some(mut pairs) = pairs {
                // Re-read source so we can slice each import's location
                // back into its original `import ...;` text — preserves
                // alias / wildcard / selective forms verbatim.
                let src = match std::fs::read_to_string(&pkg) {
                    Ok(s) => s,
                    Err(_) => {
                        dir = d.parent();
                        continue;
                    }
                };
                let stored = pairs.pop().map(|(_, s)| s);
                let pkg_class = stored.as_ref().and_then(|s| s.classes.values().next());
                let mut level: Vec<String> = Vec::new();
                if let Some(class) = pkg_class {
                    use rumoca_compile::parsing::ast::Import;
                    for imp in &class.imports {
                        let loc = match imp {
                            Import::Qualified { location, .. }
                            | Import::Renamed { location, .. }
                            | Import::Unqualified { location, .. }
                            | Import::Selective { location, .. } => location,
                        };
                        let start = loc.start as usize;
                        let end = loc.end as usize;
                        let Some(slice) = src.get(start..end) else {
                            continue;
                        };
                        let mut text = slice.trim().to_string();
                        // Rumoca's import location ranges sometimes omit
                        // the trailing `;`. Normalise so the injected
                        // `import ...;` lines parse uniformly downstream.
                        if !text.ends_with(';') {
                            text.push(';');
                        }
                        level.push(text);
                    }
                }
                // Level is the outer-relative-to-previous step. Prepend
                // so the final chain is outer-first, inner-last.
                let mut merged = level;
                merged.extend(chain.drain(..));
                chain = merged;
            }
            dir = d.parent();
        }
        let mut seen = std::collections::HashSet::new();
        chain.retain(|s| seen.insert(s.clone()));
        chain
    }
}

/// One-parse rewrite: rename + within-strip + inject imports in a
/// single span splice over the original source. Replaces the prior
/// `rewrite_duplicated_source` + `inject_class_imports` pair, each of
/// which re-parsed the same bytes — measured at ~370ms each in dev
/// builds for a 7.9 KB extracted MSL class. This single pass parses
/// once and emits final text.
///
/// Returns `None` if the parse fails so the caller can fall back to
/// the source unchanged. (Unlikely — the caller's
/// `extract_class_spans_via_path` already parsed this same source
/// successfully via the cached path.)
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
    if !(name_end <= end_start && end_end <= src.len()) {
        return None;
    }
    // Guard: every index we'll slice with must land on a UTF-8 char
    // boundary, otherwise `&src[a..b]` panics. Rumoca's spans have
    // historically been byte-correct on the source it parsed, but a
    // mismatch shows up the moment the caller's slice contains
    // multi-byte chars (e.g. `─` `►` `│` from pasted comments) — we'd
    // rather return None and let the caller keep the source unchanged
    // than abort the wasm thread.
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

    // Class slice extracted by `full_span_with_leading_comments` does
    // not include the file-level `within` clause (within precedes the
    // first class header). Empty range.
    let (wstart, wend) = (0usize, 0usize);

    // Inject anchor: position in `src` immediately after the class
    // name's optional description string(s). Same scan
    // `inject_class_imports` did.
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
    // Source up to within-strip start.
    out.push_str(&src[..wstart]);
    // Skip [wstart..wend) — within clause.
    out.push_str(&src[wend..name_start]);
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

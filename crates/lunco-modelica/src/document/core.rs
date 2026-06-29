//! The core Document representation of one Modelica source file.

use std::collections::VecDeque;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lunco_doc::{Diagnostic, Document, DocumentError, DocumentId, DocumentOrigin};
use rumoca_phase_parse::parse_to_syntax;
use rumoca_compile::parsing::ast::StoredDefinition;

use super::ops::{ModelicaChange, ModelicaOp, FreshAst, CHANGE_HISTORY_CAPACITY};
use crate::index::ModelicaIndex;

// ---------------------------------------------------------------------------
// SyntaxCache
// ---------------------------------------------------------------------------

/// Resolve a rumoca [`rumoca_phase_parse::ParseError`] into the unified
/// [`lunco_doc::Diagnostic`], converting its byte span to a 1-based (line,
/// column) over `source` when it has one. This is the single place the lenient
/// parser's structured errors are turned into the panel's clickable form —
/// every parse path (file-open, native async, wasm worker) funnels through it.
pub fn parse_diag_from_error(e: &rumoca_phase_parse::ParseError, source: &str) -> Diagnostic {
    use rumoca_phase_parse::ParseError;
    match e {
        ParseError::SyntaxError { message, span, .. } => {
            let (line, column) = byte_offset_to_line_col(source, span.start.0);
            Diagnostic::error(message.clone(), Some(line), Some(column))
        }
        // No-span variants — render a human description instead of the
        // raw `{:?}` debug dump (part of the rumoca-diagnostics
        // migration: "better description of errors").
        ParseError::NoAstProduced => {
            Diagnostic::message_only("parser produced no AST (empty or unrecoverable source)")
        }
        ParseError::IoError { path, message } => {
            Diagnostic::message_only(format!("I/O error reading `{path}`: {message}"))
        }
    }
}

/// Convert a 0-based byte offset into a 1-based (line, column) of
/// **char** positions over `source`. Char-based (not byte / UTF-16) so
/// the column lines up with the editor's char-indexed caret jump
/// ([`crate::ui::panels::code_editor`]). Clamped to the buffer end.
pub(crate) fn byte_offset_to_line_col(source: &str, byte_offset: usize) -> (u32, u32) {
    let clamped = byte_offset.min(source.len());
    let mut line = 1u32;
    let mut col = 1u32;
    for (idx, ch) in source.char_indices() {
        if idx >= clamped {
            break;
        }
        if ch == '\n' {
            line = line.saturating_add(1);
            col = 1;
        } else {
            col = col.saturating_add(1);
        }
    }
    (line, col)
}

/// Single parse cache attached to a [`ModelicaDocument`].
#[derive(Debug, Clone)]
pub struct SyntaxCache {
    /// Document generation at which this cache was produced.
    pub generation: u64,
    /// Best-effort parsed AST.
    pub ast: Arc<StoredDefinition>,
    /// Parse diagnostics, located where the parser gave us a span.
    pub errors: Vec<Diagnostic>,
}

pub type AstCache = SyntaxCache;

impl SyntaxCache {
    pub fn empty(generation: u64) -> Self {
        Self {
            generation,
            ast: Arc::new(StoredDefinition::default()),
            errors: Vec::new(),
        }
    }

    /// THE single canonical way to turn Modelica source text into a
    /// [`SyntaxCache`]. Parses with error **recovery** (`parse_to_syntax` →
    /// `best_effort`) so a syntax error in one class never wipes the healthy
    /// classes around it — the browser/index keep showing siblings mid-edit.
    /// Every parse of an **editable** document's source funnels through here —
    /// the async worker (via `install_parse_results`) and the synchronous
    /// `refresh_ast_now`. Do NOT add a second editable source→cache path that
    /// parses strictly: that reintroduces the "broken edit empties the whole
    /// class tree" regression. (Read-only MSL *library* files are the lone
    /// exception — `load_msl_file` eager-parses them once via rumoca's
    /// multi-file `parse_files_parallel`; they can't be edited into a broken
    /// state, so recovery doesn't apply.)
    pub fn from_source(source: &str, generation: u64) -> Self {
        if std::env::var_os("LUNCO_NO_PARSE").is_some() {
            return Self::empty(generation);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = source;
            return Self::empty(generation);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let recovery = parse_to_syntax(source, "model.mo");
            let errors = recovery
                .parse_errors()
                .iter()
                .map(|e| parse_diag_from_error(e, source))
                .collect();
            let ast = Arc::new(recovery.best_effort().clone());
            Self {
                generation,
                ast,
                errors,
            }
        }
    }

    pub fn install_from_worker(
        &mut self,
        ast: Arc<StoredDefinition>,
        errors: Vec<Diagnostic>,
    ) {
        self.ast = ast;
        self.errors = errors;
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub fn first_error(&self) -> Option<&str> {
        self.errors.first().map(|d| d.message.as_str())
    }

    pub fn ast(&self) -> &StoredDefinition {
        &self.ast
    }
}

// ---------------------------------------------------------------------------
// ModelicaDocument
// ---------------------------------------------------------------------------

/// The canonical Document representation of one Modelica source file.
#[derive(Debug)]
pub struct ModelicaDocument {
    id: DocumentId,
    source: String,
    /// Lazy `Arc<str>` view of [`Self::source`]. Populated on first
    /// call to [`Self::source_arc`]; invalidated by every source
    /// mutation. Lets bg-task spawn sites move source ownership
    /// across thread boundaries with an O(1) `Arc` clone instead of
    /// an O(n) string copy.
    source_arc: std::sync::OnceLock<Arc<str>>,
    syntax: Arc<SyntaxCache>,
    index: ModelicaIndex,
    generation: u64,
    origin: DocumentOrigin,
    last_saved_generation: Option<u64>,
    /// Ring buffer of structured changes. The leading u64 is a
    /// monotonic *change index* (not source generation): every
    /// `push_change` consumes a fresh value from
    /// [`Self::next_change_idx`]. Decoupling the change-ordering
    /// key from `generation` lets the index rebuild emit follow-up
    /// changes at the same source generation as the text edit that
    /// triggered the parse, without colliding with the watermark
    /// that already advanced past that generation.
    changes: VecDeque<(u64, ModelicaChange)>,
    /// Next value to assign to a change-ring entry. Incremented
    /// after each `push_change` so consumers can filter strictly by
    /// `> last_seen_idx` and never miss or double-process an entry.
    next_change_idx: u64,
    last_source_edit_at: Option<web_time::Instant>,
}

impl ModelicaDocument {
    pub fn new(id: DocumentId, source: impl Into<String>) -> Self {
        Self::with_origin(
            id,
            source,
            DocumentOrigin::untitled(format!("Untitled-{}", id.raw())),
        )
    }

    pub fn with_origin(
        id: DocumentId,
        source: impl Into<String>,
        origin: DocumentOrigin,
    ) -> Self {
        let source = source.into();
        let syntax = Arc::new(SyntaxCache::empty(0));
        let mut doc = Self::from_parts(id, source, origin, syntax);
        doc.last_source_edit_at = Some(web_time::Instant::now());
        // Start at gen 1 so it mismatches the empty placeholder SyntaxCache
        // (gen 0) and the async parse / index rebuild fires. That bump is a
        // "needs parse" signal, NOT an edit — so for a file-backed doc the
        // saved baseline must follow it, else a freshly-OPENED, never-edited
        // file reports is_dirty()=true (gen 1 ≠ saved 0) and wrongly triggers
        // the unsaved-changes close prompt. Re-sync the baseline here; an
        // Untitled doc keeps last_saved_generation=None (genuinely unsaved).
        doc.generation = 1;
        if !doc.origin.is_untitled() {
            doc.last_saved_generation = Some(doc.generation);
        }
        doc
    }

    pub fn load_msl_class(
        id: DocumentId,
        path: &Path,
        qualified: &str,
    ) -> Result<Self, String> {
        // On wasm the MSL source tree is untarred lazily (boot no longer unpacks
        // it, to avoid a startup freeze). Materialise it before reading source.
        #[cfg(target_arch = "wasm32")]
        crate::msl_remote::ensure_msl_source_unpacked();

        let full_source = if let Some(bytes) = lunco_assets::msl::msl_read(path) {
            String::from_utf8(bytes)
                .map_err(|e| format!("non-utf8 source `{}`: {e}", path.display()))?
        } else {
            {
                // Native-disk fallback when the bundled MSL source doesn't
                // hold this path. Routed through lunco-storage — `std::fs`
                // is clippy-banned in domain crates and absent on wasm
                // (where `global_msl_source` is the primary path above).
                use lunco_storage::Storage;
                let bytes = lunco_storage::FileStorage::new()
                    .read_sync(&lunco_storage::StorageHandle::File(path.to_path_buf()))
                    .map_err(|e| format!("read failed `{}`: {e}", path.display()))?;
                String::from_utf8(bytes)
                    .map_err(|e| format!("non-utf8 source `{}`: {e}", path.display()))?
            }
        };

        let short_name = qualified.rsplit('.').next().unwrap_or(qualified);
        let parent_pkg: String = {
            let mut parts: Vec<&str> = qualified.split('.').collect();
            parts.pop();
            parts.join(".")
        };

        // Unified MSL-class AST source (native + wasm): prefer the
        // pre-parsed bundle — `parsed_msl_bundle` lazily materialises it
        // from `parsed-msl.bin` on native (one ~1–3 s decode, then every
        // drill-in is an in-memory hit), and the chunked decoder fills it
        // on wasm. Keyed by the file path exactly as the indexer wrote it
        // (`indexer::ingest_file`). On a bundle miss, fall back to a
        // direct parse of the source text we already read above — this
        // works identically on both targets (no fs / rayon dependency,
        // unlike the old native-only `parse_files_parallel`, which paid a
        // full rumoca parse of the whole `package.mo` wrapper — tens of
        // seconds for `Modelica/Blocks/package.mo`).
        let key = path.to_string_lossy().to_string();
        let bundle_hit = crate::msl_remote::parsed_msl_bundle()
            .map(|b| b.iter().any(|(k, _)| *k == key))
            .unwrap_or(false);
        // A bundle MISS here means we fall back to fully parsing `full_source`
        // — for a package wrapper like `Modelica/Blocks/package.mo` (~150 KB,
        // the whole Blocks package inlined) that is the "tens of seconds" parse
        // the fallback comment warns about, and on single-threaded wasm it can
        // stall the drill-in task long enough to look like a hang. Worth a
        // breadcrumb when it happens.
        if !bundle_hit {
            bevy::log::warn!(
                "[load_msl_class] parsed-bundle MISS for `{key}` ({} bytes) — \
                 full reparse (slow for large package.mo wrappers)",
                full_source.len()
            );
        }
        let ast: StoredDefinition = match crate::msl_remote::parsed_msl_bundle()
            .and_then(|b| b.iter().find(|(k, _)| *k == key).map(|(_, a)| a.clone()))
        {
            Some(ast) => ast,
            None => rumoca_phase_parse::parse_to_ast(&full_source, &key)
                .map_err(|e| format!("parse failed `{}`: {e}", path.display()))?,
        };

        let class_def = crate::ast_extract::find_class_by_short_name(&ast, short_name)
            .ok_or_else(|| format!("class `{qualified}` not found in `{}`", path.display()))?;
        // `ClassDef.location` omits the prefix keyword and trailing `;` (see
        // `class_full_text_span`); slicing by it alone drops both and yields
        // invalid Modelica. Use the canonical full-declaration span.
        let (full_start, full_end) =
            crate::ast_extract::class_full_text_span(class_def, &full_source);
        // Defensive: the AST may come from the pre-parsed bundle while
        // `full_source` was re-read separately (msl_read). If their byte
        // offsets ever disagree (different line endings, a stale bundle, a
        // wrong-file resolution), slicing would panic — and on wasm that
        // panic happens inside the AsyncComputeTaskPool task, which dies
        // SILENTLY (no install, no error log), leaving the drill-in tab
        // stuck on "loading" forever. Validate the span and surface a real
        // error instead so the failure is visible and recoverable.
        if full_start > full_end
            || full_end > full_source.len()
            || !full_source.is_char_boundary(full_start)
            || !full_source.is_char_boundary(full_end)
        {
            return Err(format!(
                "class `{qualified}` span {full_start}..{full_end} invalid for \
                 source of {} bytes in `{}` (bundle_hit={bundle_hit}) — likely a \
                 stale/mismatched parsed bundle or wrong-file resolution",
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

        let origin = DocumentOrigin::File {
            path: path.to_path_buf(),
            writable: false,
        };
        Ok(Self::with_origin(id, source, origin))
    }

    pub fn load_msl_file(
        id: DocumentId,
        path: &Path,
    ) -> Result<Self, String> {
        // Lazily untar the MSL source tree on first drill-in (see load_msl_class).
        #[cfg(target_arch = "wasm32")]
        crate::msl_remote::ensure_msl_source_unpacked();

        let source = if let Some(bytes) = lunco_assets::msl::msl_read(path) {
            String::from_utf8(bytes)
                .map_err(|e| format!("non-utf8 source `{}`: {e}", path.display()))?
        } else {
            {
                // Native-disk fallback when the bundled MSL source doesn't
                // hold this path. Routed through lunco-storage — `std::fs`
                // is clippy-banned in domain crates and absent on wasm
                // (where `global_msl_source` is the primary path above).
                use lunco_storage::Storage;
                let bytes = lunco_storage::FileStorage::new()
                    .read_sync(&lunco_storage::StorageHandle::File(path.to_path_buf()))
                    .map_err(|e| format!("read failed `{}`: {e}", path.display()))?;
                String::from_utf8(bytes)
                    .map_err(|e| format!("non-utf8 source `{}`: {e}", path.display()))?
            }
        };

        let parsed: Result<Arc<StoredDefinition>, String> =
            if std::env::var_os("LUNCO_NO_PARSE").is_some() {
                Err("LUNCO_NO_PARSE diagnostic — parse skipped".into())
            } else {
                #[cfg(target_arch = "wasm32")]
                {
                    let key = path.to_string_lossy().to_string();
                    crate::msl_remote::global_parsed_msl()
                        .and_then(|b| b.iter()
                            .find(|(k, _)| k == &key)
                            .map(|(_, a)| Arc::new(a.clone())))
                        .ok_or_else(|| format!(
                            "load_msl_file: pre-parsed AST missing for `{key}`"
                        ))
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    match rumoca_compile::parsing::parse_files_parallel(&[path.to_path_buf()]) {
                        Ok(mut pairs) if !pairs.is_empty() => {
                            let (_, stored) = pairs.remove(0);
                            Ok(Arc::new(stored))
                        }
                        Ok(_) => Err("rumoca returned no parse result".into()),
                        Err(e) => Err(e.to_string()),
                    }
                }
            };

        let syntax = Arc::new(match parsed {
            Ok(strict) => SyntaxCache {
                generation: 0,
                ast: strict,
                errors: Vec::new(),
            },
            Err(msg) => SyntaxCache {
                generation: 0,
                ast: Arc::new(StoredDefinition::default()),
                errors: vec![Diagnostic::message_only(msg)],
            },
        });

        let origin = DocumentOrigin::File {
            path: path.to_path_buf(),
            writable: false,
        };
        Ok(Self::from_parts(id, source, origin, syntax))
    }

    pub fn from_parts(
        id: DocumentId,
        source: String,
        origin: DocumentOrigin,
        syntax: Arc<SyntaxCache>,
    ) -> Self {
        debug_assert_eq!(
            syntax.generation, 0,
            "from_parts expects a freshly-parsed SyntaxCache"
        );
        let last_saved_generation = if origin.is_untitled() {
            None
        } else {
            Some(0)
        };
        let has_errors = syntax.has_errors();
        let mut index = ModelicaIndex::new();
        index.rebuild_with_errors(&syntax.ast, &source, has_errors);
        Self {
            id,
            source,
            source_arc: std::sync::OnceLock::new(),
            syntax,
            index,
            generation: 0,
            origin,
            last_saved_generation,
            changes: VecDeque::with_capacity(CHANGE_HISTORY_CAPACITY),
            next_change_idx: 0,
            last_source_edit_at: None,
        }
    }

    pub fn id_owned(&self) -> DocumentId { self.id }
    pub fn generation_owned(&self) -> u64 { self.generation }

    pub fn index(&self) -> &ModelicaIndex { &self.index }
    pub fn source(&self) -> &str { &self.source }
    /// Shared `Arc<str>` view of the current source. First call after
    /// any edit performs one `Arc::from(self.source.as_str())`
    /// allocation; subsequent calls before the next edit are free
    /// clones. Use this in bg-task spawn sites instead of
    /// `source().to_string()` to skip the per-spawn buffer copy.
    pub fn source_arc(&self) -> Arc<str> {
        self.source_arc
            .get_or_init(|| Arc::from(self.source.as_str()))
            .clone()
    }
    pub fn ast(&self) -> &SyntaxCache { &self.syntax }
    pub fn ast_is_stale(&self) -> bool { self.syntax.generation != self.generation }
    pub fn last_source_edit_at(&self) -> Option<web_time::Instant> { self.last_source_edit_at }

    pub fn waive_ast_debounce(&mut self) {
        if self.last_source_edit_at.is_some() {
            let backdate_ms = (crate::engine_resource::AST_DEBOUNCE_MS as u64).saturating_add(1);
            self.last_source_edit_at = Some(
                web_time::Instant::now() - std::time::Duration::from_millis(backdate_ms),
            );
        }
    }

    pub fn syntax(&self) -> &SyntaxCache { &self.syntax }
    pub fn syntax_arc(&self) -> &Arc<SyntaxCache> { &self.syntax }

    pub fn strict_ast(&self) -> Option<Arc<StoredDefinition>> {
        if !self.syntax.has_errors() {
            Some(Arc::clone(&self.syntax.ast))
        } else {
            None
        }
    }

    pub fn syntax_is_stale(&self) -> bool { self.syntax.generation != self.generation }

    pub fn install_parse_results(&mut self, syntax: SyntaxCache) {
        if syntax.generation != self.generation {
            return;
        }
        self.syntax = Arc::new(syntax);
        self.rebuild_index();
        self.last_source_edit_at = None;
    }

    pub fn install_fresh_ast(&mut self, ast: Arc<StoredDefinition>) {
        self.syntax = Arc::new(SyntaxCache {
            generation: self.generation,
            ast,
            errors: Vec::new(),
        });
        self.last_source_edit_at = None;
    }

    fn rebuild_index(&mut self) {
        // Snapshot the prior class-name set + per-class shape
        // signatures so we can diff after the rebuild and emit
        // `ClassAdded` / `ClassRemoved` / `ClassRenamed` changes.
        // Without this, structural edits made through the text
        // editor (re-typing a class header, pasting a new class…)
        // silently mutate the index — and every downstream consumer
        // that keys by class name (open tabs, experiment records,
        // parameter drafts) goes stale.
        let prior: std::collections::HashSet<String> =
            self.index.classes.keys().cloned().collect();
        let prior_signatures: std::collections::HashMap<String, (usize, usize)> =
            prior
                .iter()
                .map(|name| (name.clone(), class_shape_signature(&self.index, name)))
                .collect();
        let has_errors = self.syntax.has_errors();
        if has_errors && !prior.is_empty() {
            // Transient parse failure mid-edit (user typing through
            // an intermediate broken state). Keep the existing index
            // and emit no diffs — the next clean reparse will diff
            // against the same baseline and produce the correct
            // rename / add / remove changes. Without this, every
            // half-typed keystroke would wipe `classes` and the R4
            // observer would close drilled tabs that the user
            // immediately wants back.
            return;
        }
        self.index.rebuild_with_errors(
            &self.syntax.ast,
            &self.source,
            has_errors,
        );
        let now: std::collections::HashSet<String> =
            self.index.classes.keys().cloned().collect();
        let added: Vec<String> = now.difference(&prior).cloned().collect();
        let removed: Vec<String> = prior.difference(&now).cloned().collect();
        if added.is_empty() && removed.is_empty() {
            return;
        }
        // No generation bump needed: change ordering is keyed off
        // [`Self::next_change_idx`], which already gives the new
        // entries a strictly-higher key than the text edit's
        // `TextReplaced`. Generation continues to identify source
        // versions only.
        //
        // Pair removed ↔ added by shape signature (component count,
        // connection count). Each successful pair becomes a single
        // `ClassRenamed`; leftovers fall through to per-class
        // `ClassRemoved` / `ClassAdded`. Lets a "rename Foo→Bar +
        // add Baz" edit cycle preserve the Foo→Bar tab/experiment
        // bindings instead of treating Foo as deleted.
        let new_signatures: std::collections::HashMap<String, (usize, usize)> =
            added
                .iter()
                .map(|name| (name.clone(), class_shape_signature(&self.index, name)))
                .collect();
        let mut unmatched_added: Vec<String> = added;
        let mut unmatched_removed: Vec<String> = Vec::new();
        for old in removed {
            let want = prior_signatures.get(&old).copied();
            let pair_idx = want.and_then(|sig| {
                unmatched_added
                    .iter()
                    .position(|name| new_signatures.get(name).copied() == Some(sig))
            });
            match pair_idx {
                Some(i) => {
                    let new = unmatched_added.remove(i);
                    self.push_change(ModelicaChange::ClassRenamed {
                        old,
                        new,
                    });
                }
                None => unmatched_removed.push(old),
            }
        }
        for qualified in unmatched_removed {
            self.push_change(ModelicaChange::ClassRemoved { qualified });
        }
        for qualified in unmatched_added {
            let kind = self
                .index
                .classes
                .get(&qualified)
                .map(|c| super::apply::index_kind_to_class_kind_spec(c.kind))
                .unwrap_or(crate::pretty::ClassKindSpec::Model);
            self.push_change(ModelicaChange::ClassAdded { qualified, kind });
        }
    }
}

/// Cheap shape signature for the class identified by `qualified`,
/// used to pair removed/added entries as renames when more than one
/// class changes in the same parse cycle. Hashing the component +
/// connection counts is robust to the dominant rename case (header
/// edited, body untouched) and lets false-positives degrade
/// gracefully into separate add/remove changes.
fn class_shape_signature(
    index: &crate::index::ModelicaIndex,
    qualified: &str,
) -> (usize, usize) {
    let comps = index
        .components_by_class
        .get(qualified)
        .map(|v| v.len())
        .unwrap_or(0);
    let conns = index
        .connections_by_class
        .get(qualified)
        .map(|v| v.len())
        .unwrap_or(0);
    (comps, conns)
}

impl ModelicaDocument {

    pub fn source_snapshot(&self) -> String { self.source.clone() }

    pub fn refresh_ast_now(&mut self) {
        if !self.ast_is_stale() && !self.syntax_is_stale() {
            return;
        }
        // The global engine is only needed for cross-file resolution
        // (it learns about this doc's AST so other docs can `extends` it).
        // A standalone / headless doc — and unit tests — can still refresh
        // their OWN AST + index without it, so the handle is best-effort
        // rather than a hard precondition. (Previously a missing handle
        // returned early, leaving the doc permanently stale in any context
        // that hadn't installed the global engine.)
        let handle = crate::engine_resource::global_engine_handle();

        // There is exactly ONE way to turn live source into a SyntaxCache:
        // `SyntaxCache::from_source`, which parses with error RECOVERY so a
        // broken class never wipes its healthy siblings. refresh_ast_now MUST
        // NOT parse the source any other way — the old divergence here was a
        // strict `parse_to_ast` that returned an empty AST on any error,
        // silently emptying the class tree (browser/index) while the async
        // worker path (`from_source` → `install_parse_results`) kept it. The
        // only shortcut is a pre-parsed MSL bundle AST for file-backed docs,
        // which is trusted and needs no reparse.
        let bundle_ast: Option<Arc<StoredDefinition>> = match &self.origin {
            DocumentOrigin::File { path, .. } => {
                let key = path.to_string_lossy().to_string();
                crate::msl_remote::global_parsed_msl().and_then(|b| {
                    b.iter()
                        .find(|(k, _)| k == &key)
                        .map(|(_, ast)| Arc::new(ast.clone()))
                })
            }
            _ => None,
        };

        let syntax = match bundle_ast {
            Some(ast) => SyntaxCache {
                generation: self.generation,
                ast,
                errors: Vec::new(),
            },
            None => SyntaxCache::from_source(&self.source, self.generation),
        };

        // Teach the global engine about a CLEAN AST so other docs can `extends`
        // it. Skip on parse error: never feed the shared session a partial
        // recovery AST (matches the prior behaviour, which upserted only a
        // fully successful parse). Best-effort — a headless context / unit
        // test may have no engine installed, which is fine.
        if !syntax.has_errors() {
            if let Some(h) = handle.as_ref() {
                h.lock().upsert_document_with_ast(self.id, (*syntax.ast).clone());
            }
        }

        self.syntax = Arc::new(syntax);
        self.rebuild_index();
        self.last_source_edit_at = None;
    }

    pub fn changes_since(
        &self,
        last_seen: u64,
    ) -> Option<impl Iterator<Item = &(u64, ModelicaChange)>> {
        if let Some((earliest, _)) = self.changes.front() {
            if *earliest > last_seen + 1 {
                return None;
            }
        }
        Some(self.changes.iter().filter(move |(g, _)| *g > last_seen))
    }

    /// Lowest change index still resident in the ring. Consumers
    /// compare their watermark against this — `last_seen + 1 <
    /// earliest` means the ring rolled past them and they must
    /// re-anchor instead of trusting `changes_since`.
    pub fn earliest_retained_generation(&self) -> u64 {
        self.changes
            .front()
            .map(|(idx, _)| *idx)
            .unwrap_or(self.next_change_idx)
    }

    fn push_change(&mut self, change: ModelicaChange) {
        if self.changes.len() >= CHANGE_HISTORY_CAPACITY {
            self.changes.pop_front();
        }
        self.next_change_idx = self.next_change_idx.saturating_add(1);
        self.changes.push_back((self.next_change_idx, change));
    }

    pub fn len(&self) -> usize { self.source.len() }
    pub fn is_empty(&self) -> bool { self.source.is_empty() }
    pub fn origin(&self) -> &DocumentOrigin { &self.origin }
    pub fn canonical_path(&self) -> Option<&Path> { self.origin.canonical_path() }
    pub fn is_read_only(&self) -> bool { !self.origin.accepts_mutations() }
    pub fn set_origin(&mut self, origin: DocumentOrigin) { self.origin = origin; }

    pub fn set_canonical_path(&mut self, path: Option<PathBuf>) {
        match path {
            Some(p) => {
                let writable = self.origin.is_writable() || self.origin.is_untitled();
                self.origin = DocumentOrigin::File { path: p, writable };
            }
            None => {
                self.origin = DocumentOrigin::untitled(self.origin.display_name());
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        match self.last_saved_generation {
            Some(g) => g != self.generation,
            None => true,
        }
    }

    pub fn mark_saved(&mut self) {
        self.last_saved_generation = Some(self.generation);
    }

    pub(crate) fn apply_patch(
        &mut self,
        range: Range<usize>,
        replacement: String,
        change: ModelicaChange,
        fresh_ast: FreshAst,
    ) -> Result<ModelicaOp, DocumentError> {
        if range.start > range.end || range.end > self.source.len() {
            return Err(DocumentError::ValidationFailed(format!(
                "text range {}..{} out of bounds (len={})",
                range.start,
                range.end,
                self.source.len()
            )));
        }
        if !self.source.is_char_boundary(range.start)
            || !self.source.is_char_boundary(range.end)
        {
            return Err(DocumentError::ValidationFailed(format!(
                "text range {}..{} not on char boundaries",
                range.start, range.end
            )));
        }
        let removed: String = self.source[range.clone()].to_string();
        self.source.replace_range(range.clone(), &replacement);
        // Cached Arc<str> view is now stale.
        self.source_arc = std::sync::OnceLock::new();
        self.generation = self.generation.saturating_add(1);
        self.last_source_edit_at = Some(web_time::Instant::now());

        if let FreshAst::Mutated(ast) = fresh_ast {
            self.install_fresh_ast(ast);
        }

        match &change {
            ModelicaChange::ComponentAdded { class, name } => {
                self.index.patch_component_added(class, name, "");
            }
            ModelicaChange::ComponentRemoved { class, name } => {
                self.index.patch_component_removed(class, name);
            }
            ModelicaChange::PlacementChanged {
                class,
                component,
                placement,
            } => {
                self.index
                    .patch_placement_changed(class, component, *placement);
            }
            ModelicaChange::ConnectionAdded { class, from, to } => {
                let from_port = if from.port.is_empty() { None } else { Some(from.port.as_str()) };
                let to_port = if to.port.is_empty() { None } else { Some(to.port.as_str()) };
                self.index.patch_connection_added(
                    class,
                    &from.component,
                    from_port,
                    &to.component,
                    to_port,
                );
            }
            ModelicaChange::ConnectionRemoved { class, from, to } => {
                let from_port = if from.port.is_empty() { None } else { Some(from.port.as_str()) };
                let to_port = if to.port.is_empty() { None } else { Some(to.port.as_str()) };
                self.index.patch_connection_removed(
                    class,
                    &from.component,
                    from_port,
                    &to.component,
                    to_port,
                );
            }
            ModelicaChange::ParameterChanged {
                class,
                component,
                param,
                value,
            } => {
                self.index.patch_parameter_changed(class, component, param, value);
            }
            ModelicaChange::ClassAdded { qualified, kind } => {
                self.index
                    .patch_class_added(qualified, super::apply::class_kind_spec_to_index_kind(*kind));
            }
            ModelicaChange::ClassRemoved { qualified } => {
                self.index.patch_class_removed(qualified);
            }
            _ => {}
        }
        self.push_change(change);
        let inverse_range = range.start..(range.start + replacement.len());
        Ok(ModelicaOp::EditText {
            range: inverse_range,
            replacement: removed,
        })
    }
}

impl Document for ModelicaDocument {
    type Op = ModelicaOp;

    fn id(&self) -> DocumentId { self.id }
    fn generation(&self) -> u64 { self.generation }

    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError> {
        if !self.origin.accepts_mutations() {
            return Err(DocumentError::ReadOnly);
        }
        let kind = op.classify();
        let (range, replacement, change, fresh_ast) =
            super::apply::op_to_patch(&self.source, &self.syntax, &self.syntax.ast, op)?;
        
        debug_assert!(
            match (&fresh_ast, kind) {
                (FreshAst::Mutated(_), super::ops::OpKind::Structured) => true,
                (FreshAst::TextEdit, super::ops::OpKind::Text) => true,
                _ => false,
            },
            "ModelicaOp classification mismatch with FreshAst variant"
        );
        self.apply_patch(range, replacement, change, fresh_ast)
    }
}

//! Per-Twin Modelica domain engine.
//!
//! Wraps a long-lived [`rumoca_compile::Session`] populated with the
//! source of every open Modelica document in the active Twin.
//! Cross-file queries (inheritance-merged components, name resolution,
//! completion) read from the session's fingerprinted phase caches
//! instead of running their own AST walkers in lunco-modelica.
//!
//! ## Where this fits architecturally
//!
//! - **`lunco-twin`** stays domain-agnostic — it doesn't import rumoca.
//! - **`lunco-doc::DomainEngine`** is the trait Twin/UI talks through.
//! - **`ModelicaEngine`** (this file) is the Modelica-specific impl
//!   that owns rumoca state. Per-Twin in scope; today there's a single
//!   instance because the workbench hosts a single Twin.
//!
//! When multi-Twin lands, this resource becomes
//! `Map<TwinId, ModelicaEngine>` and the trait dispatch routes
//! (twin_id, doc_id) to the right engine. The internal API stays the
//! same.
//!
//! ## What's wired today
//!
//! - `Self::upsert_document` / `Self::close_document` — add or
//!   replace a document's source in the session.
//! - `Self::inherited_components` — calls
//!   `Session::class_component_members_query` so panels get
//!   inheritance-merged member lists for free (no per-panel
//!   reimplementation of `extract_*_inherited`).
//!
//! ## What's deferred (next commits)
//!
//! - Auto-sync system: a Bevy `Update` system that mirrors changes
//!   from `ModelicaDocumentRegistry` into the session. Today callers
//!   call `upsert_document` explicitly.
//! - Library-parent session for MSL (`Session::with_library_parent`)
//!   so cross-Twin MSL state is shared once multi-Twin lands.

use lunco_doc::DocumentId;
use rumoca_compile::Session;
use std::collections::{HashMap, HashSet};

/// Inherited member info with variability + causality.
/// Note: `class_component_members_typed_query` was removed from rumoca main.
/// This stub struct preserves the public API until the upstream feature returns.
#[derive(Debug, Clone)]
pub struct InheritedMember {
    pub name: String,
    pub type_name: String,
    pub variability: InheritedVariability,
    pub causality: InheritedCausality,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InheritedVariability {
    Constant,
    Parameter,
    Discrete,
    Continuous,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InheritedCausality {
    Input,
    Output,
    Internal,
}

fn map_variability_inherited(
    v: &rumoca_compile::parsing::Variability,
) -> InheritedVariability {
    use rumoca_compile::parsing::Variability as V;
    match v {
        V::Empty | V::Continuous(_) => InheritedVariability::Continuous,
        V::Constant(_) => InheritedVariability::Constant,
        V::Discrete(_) => InheritedVariability::Discrete,
        V::Parameter(_) => InheritedVariability::Parameter,
    }
}

fn map_causality_inherited(
    c: &rumoca_compile::parsing::Causality,
) -> InheritedCausality {
    use rumoca_compile::parsing::Causality as C;
    match c {
        C::Empty => InheritedCausality::Internal,
        C::Input(_) => InheritedCausality::Input,
        C::Output(_) => InheritedCausality::Output,
    }
}

/// Workspace-wide rumoca state for one Twin's Modelica content.
///
/// Plain Rust — **not** a Bevy `Resource`. Bevy users wrap this in
/// [`crate::engine::ModelicaEngineRes`] (below) which is the actual `Resource`.
/// The split keeps the engine usable from headless contexts
/// (`lunco-twin-server`, CLI, AI-agent runtimes, WASM thin clients)
/// without forcing Bevy into the dependency graph of every consumer.
///
/// Holds a single [`rumoca_compile::Session`] populated with the
/// source of every open Modelica document; cross-file queries route
/// through the session's caches.
pub struct ModelicaEngine {
    session: Session,
    /// `DocumentId` → URI used inside the session. Stable for the
    /// document's lifetime; freed on `Self::close_document`.
    uri_for_doc: HashMap<DocumentId, String>,
    /// Documents whose async parse is currently in flight. Prevents
    /// double-spawning the same parse while a worker is mid-flight.
    /// Inserted by `mark_pending`; cleared by the worker on completion.
    pending: HashSet<DocumentId>,
    /// Async-parse completions ready for the Bevy adapter to drain.
    /// Each entry carries the doc's generation **at parse spawn**, so
    /// the adapter can ignore stale results when the doc moved on.
    completed: Vec<(DocumentId, u64)>,
    /// Located parse diagnostics from the most recent async parse,
    /// keyed by doc. Set off-lock by the native spawn (which holds the
    /// source + recovery) and taken by `drive_engine_sync` on drain to
    /// fill the doc's `SyntaxCache`. Lets the native live-edit path
    /// surface clickable parse errors instead of a generic string.
    parse_diags: HashMap<DocumentId, Vec<crate::document::ParseDiag>>,
}

impl Default for ModelicaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelicaEngine {
    pub fn new() -> Self {
        Self {
            session: Session::default(),
            uri_for_doc: HashMap::new(),
            pending: HashSet::new(),
            completed: Vec::new(),
            parse_diags: HashMap::new(),
        }
    }

    /// Stash the located parse diagnostics for `doc_id` produced by an
    /// off-lock async parse. Overwrites any prior set — only the latest
    /// parse's diagnostics matter.
    pub fn set_parse_diags(
        &mut self,
        doc_id: DocumentId,
        diags: Vec<crate::document::ParseDiag>,
    ) {
        self.parse_diags.insert(doc_id, diags);
    }

    /// Take (and clear) the parse diagnostics stashed for `doc_id`.
    /// Returns an empty vec when none were recorded.
    pub fn take_parse_diags(
        &mut self,
        doc_id: DocumentId,
    ) -> Vec<crate::document::ParseDiag> {
        self.parse_diags.remove(&doc_id).unwrap_or_default()
    }

    /// URI we'd use for `doc_id` — same value `uri()` would produce, but
    /// callable from `&self` contexts where holding `self` mutably
    /// across a parse would block other engine queries.
    pub fn uri_for(&self, doc_id: DocumentId) -> String {
        self.uri(doc_id)
    }

    /// Reserve an async-parse slot for `doc_id`. Returns `true` if the
    /// caller now owns the spawn (no parse was in flight); `false` if
    /// another parse is already running for this doc.
    pub fn mark_pending(&mut self, doc_id: DocumentId) -> bool {
        self.pending.insert(doc_id)
    }

    /// Worker reports its parse finished. Clears the in-flight slot
    /// and queues the result for the adapter to drain.
    pub fn finish_parse(&mut self, doc_id: DocumentId, gen: u64) {
        self.pending.remove(&doc_id);
        self.completed.push((doc_id, gen));
    }

    /// Whether `doc_id` has an async parse in flight right now.
    pub fn is_doc_pending(&self, doc_id: DocumentId) -> bool {
        self.pending.contains(&doc_id)
    }

    /// Number of async parses currently in flight. Used by
    /// `drive_engine_sync` on wasm to throttle the cooperative
    /// `AsyncComputeTaskPool` to one parse at a time so a 5 s rumoca
    /// parse for a hidden tab can't starve the active tab.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Clear all pending parses. Used to unwedge the queue when a worker crashes.
    pub fn clear_all_pending(&mut self) {
        self.pending.clear();
    }

    /// Take all completions accumulated since the last drain. Bevy
    /// adapter calls this once per `Update` tick.
    pub fn drain_completed(&mut self) -> Vec<(DocumentId, u64)> {
        std::mem::take(&mut self.completed)
    }

    /// Install a strict AST under `doc_id`'s session URI without
    /// touching pending/completed bookkeeping. Used by the async
    /// worker after it parses off-lock.
    pub fn install_parsed_ast(
        &mut self,
        doc_id: DocumentId,
        ast: rumoca_compile::parsing::ast::StoredDefinition,
    ) {
        let uri = self.uri(doc_id);
        self.uri_for_doc.entry(doc_id).or_insert_with(|| uri.clone());
        self.session.add_parsed_batch(vec![(uri, ast)]);
    }

    /// Lenient install — used when the strict parse fails. Falls back
    /// to `Session::add_document` so the recovered tree (if any) lands
    /// in the session for partial-data queries.
    pub fn install_lenient(&mut self, doc_id: DocumentId, source: &str) {
        let uri = self.uri(doc_id);
        self.uri_for_doc.entry(doc_id).or_insert_with(|| uri.clone());
        let _ = self.session.add_document(&uri, source);
    }

    /// URI we use for `doc_id` inside the session. Untitled / on-disk
    /// docs share the same naming scheme so cross-doc references
    /// work uniformly.
    fn uri(&self, doc_id: DocumentId) -> String {
        format!("doc-{}.mo", doc_id.raw())
    }

    /// Install a document whose AST has already been parsed elsewhere
    /// (typically by `ModelicaDocument`). Bypasses the parser entirely
    /// via `Session::add_parsed_batch`. Use this in steady-state sync
    /// paths to avoid re-parsing the same bytes the doc just parsed.
    pub fn upsert_document_with_ast(
        &mut self,
        doc_id: DocumentId,
        ast: rumoca_compile::parsing::ast::StoredDefinition,
    ) {
        let uri = self.uri(doc_id);
        self.uri_for_doc.entry(doc_id).or_insert_with(|| uri.clone());
        self.session.add_parsed_batch(vec![(uri, ast)]);
    }

    /// The engine's view of `doc_id`'s parsed AST, or `None` if the
    /// document hasn't been upserted yet (or its parse failed).
    ///
    /// This is the canonical accessor for code that wants the
    /// engine-canonical AST of a doc — replaces direct
    /// `ModelicaDocument::ast()` reads in code paths that should
    /// follow the engine as source of truth (notably the per-doc
    /// `Index` rebuild in `Document::rebuild_index`).
    pub fn parsed_for_doc(
        &mut self,
        doc_id: DocumentId,
    ) -> Option<&rumoca_compile::parsing::ast::StoredDefinition> {
        let uri = self.uri_for_doc.get(&doc_id)?.clone();
        self.session.parsed_file_query(&uri)
    }

    /// Forget a document. Drops the URI map entry **and** removes the
    /// document from rumoca's session — its parsed AST, per-file
    /// caches, and any resolved-tree state referencing it are
    /// invalidated. Reopening the same `DocumentId` starts fresh.
    pub fn close_document(&mut self, doc_id: DocumentId) {
        self.parse_diags.remove(&doc_id);
        if let Some(uri) = self.uri_for_doc.remove(&doc_id) {
            self.session.remove_document(&uri);
        }
    }

    /// Resolved + merged Icon for `qualified`.
    ///
    /// **Single AST-aware entry point for icon resolution.** Panels
    /// must use this — never call [`crate::annotations::extract_icon`]
    /// or [`crate::annotations::extract_icon_via_engine`] directly.
    /// Pure delegation: rumoca's `class_lookup_query` resolves bare
    /// names by suffix-match across session docs (MLS § 5), and
    /// `class_inherited_annotations_query` is fingerprint-cached
    /// inside rumoca. No secondary cache here — duplicating rumoca's
    /// memoisation costs memory + invalidation complexity for no win.
    ///
    /// Off-thread-safe: never reads from disk, never spawns a parse.
    /// If `qualified` isn't in the session yet, returns `None` and
    /// the caller renders a default icon. A subsequent render after
    /// the class lands picks up the resolved icon.
    ///
    /// AST-as-source-of-truth: the session IS the AST store;
    /// consulting it is consulting the AST.
    pub fn icon_for(&mut self, qualified: &str) -> Option<crate::annotations::Icon> {
        crate::annotations::extract_icon_via_engine(qualified, self)
    }

    /// Load a library (MSL or third-party) into the session as a
    /// `DurableExternal` source root. Once loaded, every class in
    /// `files` is resolvable through the session's normal queries —
    /// no separate cache, no path lookup. Cross-file inheritance
    /// walks user docs + this library uniformly.
    ///
    /// `set_id` is a stable identifier (e.g. `"msl"`); `label` is a
    /// log-friendly name (e.g. `"in-memory:msl"`); `files` is the
    /// already-loaded `(uri, source)` pairs (typically decoded from
    /// the `msl_indexer` bincode bundle).
    ///
    /// Diagnostics from per-file parse failures are returned via the
    /// session's load report; we surface only the high-level
    /// "files inserted" count for callers that just want to know if
    /// it worked.
    pub fn load_library_files(
        &mut self,
        _set_id: &str,
        _label: &str,
        files: Vec<(String, String)>,
    ) -> usize {
        // rumoca main removed `load_source_root_in_memory`. Add files
        // individually via add_document.
        let mut count = 0;
        for (uri, text) in &files {
            if self.session.add_document(uri, text).is_ok() {
                count += 1;
            }
        }
        count
    }

    /// Inheritance-merged component members for a fully-qualified
    /// class. Returns `(name, type)` pairs walking the `extends`
    /// chain — including across files when the bases are in other
    /// open documents.
    ///
    /// This is the call panels SHOULD make instead of running their
    /// own `extract_*_inherited` walker. Cached inside the session
    /// (per [`rumoca_compile::Session::class_component_members_query`]).
    pub fn inherited_components(&mut self, qualified: &str) -> Vec<(String, String)> {
        self.session.class_component_members_query(qualified)
    }

    /// Inheritance-merged component members with variability +
    /// causality preserved. Same `extends` walk as
    /// `Self::inherited_components` but consumers don't have to
    /// re-walk the AST to bucket parameters / inputs / outputs.
    ///
    /// rumoca main dropped the typed `class_component_members_typed_query`,
    /// so we take the authoritative (scope-resolved) membership list from
    /// `class_component_members_query` and enrich each member with the
    /// variability / causality / binding we read directly off the
    /// `ClassDef` of the class (and its `extends` bases). Extraction
    /// mirrors `index::insert_class_recursive`, the canonical path the
    /// inspector already trusts.
    pub fn inherited_members_typed(&mut self, qualified: &str) -> Vec<InheritedMember> {
        // Authoritative, scope-resolved membership (name, type) — handles
        // the full `extends` walk including MSL bases.
        let members = self.session.class_component_members_query(qualified);

        // For each member resolve the class that actually *declares* it
        // (`class_component_member_info_query` returns
        // `(declaring_class, member_type)`), so we can read the typed
        // `ast::Component` from that class's own declarations. We avoid
        // `class_def`/`class_lookup_query` here: in rumoca main those
        // return the bare class name, not a `parsed_file_query` URI, so
        // the AST never comes back. `class_components_in_class_query`
        // routes through the working `lookup_query_class_target`.
        let resolved: Vec<(String, String, String)> = members
            .into_iter()
            .map(|(name, type_name)| {
                let declaring = self
                    .session
                    .class_component_member_info_query(qualified, &name)
                    .map(|(declaring, _ty)| declaring)
                    .unwrap_or_else(|| qualified.to_string());
                (name, type_name, declaring)
            })
            .collect();

        // Pre-fetch typed components per unique declaring class (one
        // query each), then build — keeps the `&mut self.session`
        // borrows out of the build closure.
        let mut comps_cache: HashMap<String, Vec<rumoca_compile::parsing::ast::Component>> =
            HashMap::new();
        let unique: HashSet<String> =
            resolved.iter().map(|(_, _, d)| d.clone()).collect();
        for declaring in unique {
            let comps = self
                .session
                .class_components_in_class_query(&declaring)
                .unwrap_or_default();
            comps_cache.insert(declaring, comps);
        }

        resolved
            .into_iter()
            .map(|(name, type_name, declaring)| {
                let comp = comps_cache
                    .get(&declaring)
                    .and_then(|cs| cs.iter().find(|c| c.name == name));
                let (variability, causality, default_value) = match comp {
                    Some(c) => (
                        map_variability_inherited(&c.variability),
                        map_causality_inherited(&c.causality),
                        // `parameter Real R = 100;` — the `= 100` lands in
                        // `binding`; `start` holds the type's default
                        // (0.0) unless a `start=` modifier set it. Prefer
                        // the binding, fall back to a start *modification*.
                        c.binding
                            .as_ref()
                            .map(|e| format!("{e}"))
                            .or_else(|| {
                                if c.start_is_modification {
                                    Some(format!("{}", c.start))
                                } else {
                                    None
                                }
                            }),
                    ),
                    None => (
                        InheritedVariability::Continuous,
                        InheritedCausality::Internal,
                        None,
                    ),
                };
                InheritedMember {
                    name,
                    type_name,
                    variability,
                    causality,
                    default_value,
                }
            })
            .collect()
    }

    /// Inheritance chain of annotation lists for a class.
    ///
    /// Note: `class_inherited_annotations_query` was removed from rumoca main.
    /// Returns empty until the upstream feature returns.
    pub fn inherited_annotations(
        &mut self,
        _qualified: &str,
    ) -> Vec<Vec<rumoca_compile::parsing::ast::Expression>> {
        Vec::new()
    }

    /// Read-only access to the underlying session for advanced queries
    /// not yet wrapped here. Use sparingly — prefer growing this
    /// crate's API over leaking the session through panels.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Resolve `qualified` to its `ClassDef` inside the session's
    /// already-parsed sources. Walks the dotted path through nested
    /// classes the same way rumoca's internal lookup does.
    ///
    /// Returns `None` if no document containing the class has been
    /// upserted (or loaded via `load_library_files`). Callers that
    /// need filesystem-backed lazy loading should check `has_class`
    /// first, push the file via `session_mut().add_document`, then
    /// call `class_def`.
    pub fn class_def(
        &mut self,
        qualified: &str,
    ) -> Option<rumoca_compile::parsing::ast::ClassDef> {
        let Some(uri) = self.session.class_lookup_query(qualified) else {
            bevy::log::warn!("[engine] class_def: class_lookup_query failed for {}", qualified);
            return None;
        };
        let Some(parsed) = self.session.parsed_file_query(&uri) else {
            bevy::log::warn!("[engine] class_def: parsed_file_query failed for uri {}", uri);
            return None;
        };
        // Route through the canonical within-aware lookup so this
        // path can't silently disagree with the read path when the
        // file carries a `within Foo;` clause and the caller asks
        // for `Foo.Bar` (the segment walk would look for "Foo" in
        // `parsed.classes`, which is keyed under "Bar"). Same bug
        // class as `walk_qualified` and `lookup_class_mut` had.
        let found = crate::diagram::find_class_by_qualified_name(&parsed, qualified).cloned();
        if found.is_none() {
            bevy::log::warn!("[engine] class_def: find_class_by_qualified_name failed for {} in uri {}", qualified, uri);
        }
        found
    }

    /// Whether `qualified` resolves to a class currently in the
    /// session. Cheap — uses rumoca's existing
    /// `class_lookup_query`. Used as the first step in lazy MSL
    /// loading: if the class isn't here, the caller resolves a
    /// file path, reads it, and pushes via
    /// `session_mut().add_document(...)`. Subsequent calls then
    /// return `true` without touching the filesystem.
    pub fn has_class(&mut self, qualified: &str) -> bool {
        self.session.class_lookup_query(qualified).is_some()
    }
}

// No Bevy adapter here yet. When the auto-sync system lands (it
// needs `ResMut<...>` to mirror document changes into the engine),
// the right home is a sibling crate `lunco-modelica-bevy` — same
// pattern as the existing `lunco-doc` / `lunco-doc-bevy` split.
// Until then this file stays plain Rust and headless-friendly.

#[cfg(test)]
mod tests {
    use super::*;

    /// Test convenience: parse `src` and install the resulting AST into
    /// `engine` under `id`. The engine surface only accepts pre-parsed
    /// ASTs (Step 4 of the AST-canonical roadmap); tests opt in to the
    /// parse cost explicitly via this helper rather than via a
    /// source-taking method on the engine. Production code does the
    /// same parse-then-upsert dance directly at its call sites — see
    /// `document::ModelicaDocument::refresh_ast_now` for the canonical
    /// pattern.
    fn upsert_test(engine: &mut ModelicaEngine, id: DocumentId, src: &str) {
        let ast = rumoca_phase_parse::parse_to_ast(src, "test.mo")
            .expect("test source must parse");
        engine.upsert_document_with_ast(id, ast);
    }

    #[test]
    fn inherited_components_walks_extends_across_docs() {
        let mut engine = ModelicaEngine::new();
        let base = "model Base\n  Real x;\n  Real y;\nend Base;\n";
        let derived = "model Derived\n  extends Base;\n  Real z;\nend Derived;\n";
        upsert_test(&mut engine, DocumentId::new(1), base);
        upsert_test(&mut engine, DocumentId::new(2), derived);

        let members = engine.inherited_components("Derived");
        let names: Vec<&str> = members.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"x") && names.contains(&"y"),
            "expected inherited x + y, got {names:?}"
        );
        assert!(names.contains(&"z"), "expected own z, got {names:?}");
    }

    #[test]
    fn upsert_overwrites_previous_source() {
        let mut engine = ModelicaEngine::new();
        let v1 = "model M\n  Real a;\nend M;\n";
        let v2 = "model M\n  Real a;\n  Real b;\nend M;\n";
        upsert_test(&mut engine, DocumentId::new(1), v1);
        let n1 = engine.inherited_components("M").len();
        upsert_test(&mut engine, DocumentId::new(1), v2);
        let n2 = engine.inherited_components("M").len();
        assert!(n2 > n1, "second upsert should replace v1; n1={n1}, n2={n2}");
    }

    #[test]
    fn close_document_drops_uri_mapping() {
        let mut engine = ModelicaEngine::new();
        upsert_test(&mut engine, DocumentId::new(1), "model M\nend M;\n");
        assert!(engine.uri_for_doc.contains_key(&DocumentId::new(1)));
        engine.close_document(DocumentId::new(1));
        assert!(!engine.uri_for_doc.contains_key(&DocumentId::new(1)));
    }

    #[test]
    fn has_class_reflects_session_contents() {
        let mut engine = ModelicaEngine::new();
        assert!(!engine.has_class("Foo"), "empty session reports no class");

        upsert_test(&mut engine, DocumentId::new(1), "model Foo\nend Foo;\n");
        assert!(engine.has_class("Foo"), "Foo present after upsert");

        engine.close_document(DocumentId::new(1));
        assert!(!engine.has_class("Foo"), "Foo gone after close");
    }

    #[test]
    fn load_library_files_makes_classes_resolvable() {
        let mut engine = ModelicaEngine::new();
        // Pretend we have a tiny "library" with a Base class.
        let library_files = vec![(
            "lib/Base.mo".to_string(),
            "model Base\n  parameter Real k = 5;\n  Real x;\nend Base;\n".to_string(),
        )];
        let inserted = engine.load_library_files("test_lib", "in-memory:test", library_files);
        assert_eq!(inserted, 1, "library file should be inserted");

        // A user doc that extends a class from the library — without
        // any explicit upsert wiring it together. Cross-file inheritance
        // walks user + library uniformly through the same session.
        upsert_test(
            &mut engine,
            DocumentId::new(99),
            "model UserMod\n  extends Base;\n  Real y;\nend UserMod;\n",
        );

        let members = engine.inherited_components("UserMod");
        let names: Vec<&str> = members.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"k"),
            "library Base.k must be resolved across the user doc + library: {names:?}"
        );
        assert!(names.contains(&"x"), "library Base.x must be resolved");
        assert!(names.contains(&"y"), "user-doc UserMod.y must be present");
    }

    #[test]
    fn close_document_purges_session_state() {
        let mut engine = ModelicaEngine::new();
        // Two docs, where Derived inherits from Base across files.
        upsert_test(
            &mut engine,
            DocumentId::new(1),
            "model Base\n  Real x;\nend Base;\n",
        );
        upsert_test(
            &mut engine,
            DocumentId::new(2),
            "model Derived\n  extends Base;\n  Real y;\nend Derived;\n",
        );
        // Sanity: inheritance resolves while Base is still open.
        let before = engine.inherited_components("Derived");
        assert!(
            before.iter().any(|(n, _)| n == "x"),
            "x should be inherited before close: {before:?}"
        );

        // Close Base — its source must actually leave rumoca's
        // session, not just our URI map. After this, Derived's
        // inherited member walk shouldn't find `x` anymore.
        engine.close_document(DocumentId::new(1));
        let after = engine.inherited_components("Derived");
        assert!(
            !after.iter().any(|(n, _)| n == "x"),
            "x should NOT be inherited after Base is closed: {after:?}"
        );
    }

    #[test]
    fn inherited_members_typed_preserves_variability_and_causality() {
        let mut engine = ModelicaEngine::new();
        let src = "model Base\n  parameter Real k = 1;\n  input Real u;\n  output Real y;\nend Base;\n\nmodel Derived\n  extends Base;\n  Real x;\nend Derived;\n";
        upsert_test(&mut engine, DocumentId::new(1), src);

        let members = engine.inherited_members_typed("Derived");
        let by_name: HashMap<&str, &InheritedMember> =
            members.iter().map(|m| (m.name.as_str(), m)).collect();

        assert_eq!(
            by_name["k"].variability,
            InheritedVariability::Parameter,
            "k should be a parameter"
        );
        assert_eq!(
            by_name["u"].causality,
            InheritedCausality::Input,
            "u should be an input"
        );
        assert_eq!(
            by_name["y"].causality,
            InheritedCausality::Output,
            "y should be an output"
        );
        assert_eq!(
            by_name["x"].variability,
            InheritedVariability::Continuous,
            "x should be continuous"
        );
        assert_eq!(by_name["x"].causality, InheritedCausality::Internal);
    }

    #[test]
    fn inherited_members_typed_carries_default_values() {
        let mut engine = ModelicaEngine::new();
        let src = "model Base\n  parameter Real R = 100;\n  parameter Real C = 0.001;\n  Real free;\nend Base;\n\nmodel Derived\n  extends Base;\n  parameter Real extra = 42;\nend Derived;\n";
        upsert_test(&mut engine, DocumentId::new(1), src);

        let members = engine.inherited_members_typed("Derived");
        let by_name: HashMap<&str, &InheritedMember> =
            members.iter().map(|m| (m.name.as_str(), m)).collect();

        // Inherited parameter values come through the extends chain.
        assert_eq!(
            by_name["R"].default_value.as_deref(),
            Some("100"),
            "R from Base should carry its default"
        );
        assert_eq!(
            by_name["C"].default_value.as_deref(),
            Some("0.001"),
            "C from Base should carry its default"
        );
        // Local Derived members.
        assert_eq!(
            by_name["extra"].default_value.as_deref(),
            Some("42"),
            "Derived.extra has its own default"
        );
        // Free variables (no binding) report None.
        assert!(
            by_name["free"].default_value.is_none(),
            "free has no default: {:?}",
            by_name["free"].default_value
        );
    }
}

//! Modelica authoring engine for the active Twin.
//!
//! The shared Rumoca session holds open documents and admitted source libraries.
//! Generation-checked document synchronization and source-root admission update
//! that session; typed member queries and diagram snapshots consume its caches.
//! The separate compiler/worker host owns checked compilation and simulation.

use lunco_doc::DocumentId;
use rumoca_compile::Session;
use std::collections::{HashMap, HashSet};

/// Inherited member info with variability + causality.
#[derive(Debug, Clone)]
pub struct InheritedMember {
    pub name: String,
    pub type_name: String,
    pub variability: InheritedVariability,
    pub causality: InheritedCausality,
    pub default_value: Option<String>,
    /// Authored metadata combined with Rumoca's resolved type facts.
    pub metadata: lunco_modelica_ast::ast_extract::TypedComponent,
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

fn map_variability_inherited(v: &lunco_modelica_ast::Variability) -> InheritedVariability {
    use lunco_modelica_ast::Variability as V;
    match v {
        V::Empty | V::Continuous(_) => InheritedVariability::Continuous,
        V::Constant(_) => InheritedVariability::Constant,
        V::Discrete(_) => InheritedVariability::Discrete,
        V::Parameter(_) => InheritedVariability::Parameter,
    }
}

fn map_causality_inherited(c: &lunco_modelica_ast::Causality) -> InheritedCausality {
    use lunco_modelica_ast::Causality as C;
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
///
/// Holds a single [`rumoca_compile::Session`] populated with the
/// source of every open Modelica document; cross-file queries route
/// through the session's caches.
pub struct ModelicaEngine {
    session: Session,
    /// `DocumentId` → URI used inside the session. Stable for the
    /// document's lifetime; freed on `Self::close_document`.
    uri_for_doc: HashMap<DocumentId, String>,
    /// Merged icon projection keyed by exact class identity. Source memo
    /// invalidation retires this projection and its paint-side texture together.
    icon_cache: lunco_modelica_ast::source_memo::SourceMemo<lunco_modelica_ast::annotations::Icon>,
    /// Documents whose async parse is currently in flight. Prevents
    /// double-spawning the same parse while a worker is mid-flight.
    /// Inserted by `mark_pending`; cleared by the worker on completion.
    pending: HashSet<DocumentId>,
    /// Async-parse completions ready for the Bevy adapter to drain.
    /// Each result owns the exact syntax and diagnostics for its spawn
    /// generation. Draining moves that result without copying the session AST.
    completed: Vec<(DocumentId, lunco_modelica_document::SyntaxCache)>,
    /// Bundled source roots currently being parsed by the async loader.
    pending_source_roots: HashSet<String>,
    /// Roots completed since the last Bevy adapter drain.
    completed_source_roots: Vec<String>,
    /// Bundled roots whose source could not produce any parsed definitions.
    /// This is terminal for the current engine lifetime so a resolver does not
    /// spin in `Loading` forever after a malformed packaged asset.
    failed_source_roots: HashMap<String, String>,
    /// Complete parsed source sets installed through the session source-set
    /// boundary. This is distinct from class lookup: a single lazily loaded
    /// document must not masquerade as a complete library installation.
    installed_source_sets: HashSet<String>,
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
            icon_cache: lunco_modelica_ast::source_memo::SourceMemo::default(),
            pending: HashSet::new(),
            completed: Vec::new(),
            pending_source_roots: HashSet::new(),
            completed_source_roots: Vec::new(),
            failed_source_roots: HashMap::new(),
            installed_source_sets: HashSet::new(),
        }
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
    pub fn finish_parse(
        &mut self,
        doc_id: DocumentId,
        syntax: lunco_modelica_document::SyntaxCache,
    ) {
        self.pending.remove(&doc_id);
        self.completed.push((doc_id, syntax));
    }

    /// Cancel a parse that was not admitted to a worker queue.
    ///
    /// Unlike `finish_parse`, rejection has no result to publish and must not
    /// create a completion that can never be installed by the adapter.
    pub fn cancel_parse(&mut self, doc_id: DocumentId) {
        self.pending.remove(&doc_id);
    }

    /// Whether `doc_id` has async parse work that the adapter has not fully
    /// consumed yet. A worker completion remains busy until it is drained;
    /// otherwise a bounded completion batch could requeue the same document
    /// while its result was still waiting for the main-thread install.
    pub fn has_parse_work(&self, doc_id: DocumentId) -> bool {
        self.pending.contains(&doc_id)
            || self
                .completed
                .iter()
                .any(|(completed, _)| *completed == doc_id)
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

    /// Take at most `limit` completions accumulated since the last drain.
    ///
    /// The Bevy adapter installs each AST into the document registry on the
    /// update schedule. Keeping this queue bounded per update prevents a burst of
    /// worker completions from turning one frame into an unbounded batch of
    /// AST/index rebuilds. FIFO order keeps document readiness deterministic.
    pub fn drain_completed_up_to(
        &mut self,
        limit: usize,
    ) -> Vec<(DocumentId, lunco_modelica_document::SyntaxCache)> {
        let count = limit.min(self.completed.len());
        self.completed.drain(..count).collect()
    }

    /// Whether the adapter still has completion notifications to drain.
    /// Called while the engine mutex is held when the async-sync wakeup latch
    /// is cleared.
    pub(crate) fn has_completed_work(&self) -> bool {
        !self.completed.is_empty() || !self.completed_source_roots.is_empty()
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
        self.uri_for_doc
            .entry(doc_id)
            .or_insert_with(|| uri.clone());
        lunco_modelica_ast::source_memo::invalidate_source_memos();
        self.session.add_parsed_batch(vec![(uri, ast)]);
    }

    /// Lenient install — used when the strict parse fails. Falls back
    /// to `Session::add_document` so the recovered tree (if any) lands
    /// in the session for partial-data queries.
    pub fn install_lenient(&mut self, doc_id: DocumentId, source: &str) {
        let uri = self.uri(doc_id);
        self.uri_for_doc
            .entry(doc_id)
            .or_insert_with(|| uri.clone());
        lunco_modelica_ast::source_memo::invalidate_source_memos();
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
        self.uri_for_doc
            .entry(doc_id)
            .or_insert_with(|| uri.clone());
        lunco_modelica_ast::source_memo::invalidate_source_memos();
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
        if let Some(uri) = self.uri_for_doc.remove(&doc_id) {
            self.session.remove_document(&uri);
            lunco_modelica_ast::source_memo::invalidate_source_memos();
        }
    }

    /// Resolved + merged Icon for `qualified`.
    ///
    /// **Single AST-aware entry point for icon resolution.** Panels
    /// must use this — never start a second inheritance walk from a panel.
    /// Rumoca supplies exact source declarations and scope-resolved member
    /// facts. The application caches its merged icon projection; every AST
    /// install invalidates that projection.
    ///
    /// The background projection reads resident session source; Rumoca owns
    /// any declaration materialization. No library filesystem discovery runs here.
    /// If `qualified` isn't in the session yet, returns `None` and
    /// the caller preserves an explicit unresolved/loading state. A
    /// subsequent projection after the class lands picks up the resolved
    /// authored icon.
    ///
    /// AST-as-source-of-truth: the session IS the AST store;
    /// consulting it is consulting the AST.
    pub fn icon_for(&mut self, qualified: &str) -> Option<lunco_modelica_ast::annotations::Icon> {
        let _span = bevy::log::info_span!("modelica_icon", class = qualified).entered();
        // Result cache — see `icon_cache`. The inheritance-walk +
        // per-base ClassDef clones in the engine adapter are far
        // too costly to repeat every frame for a static class. Cleared
        // on AST install so edits still reflect.
        if let Some(hit) = self.icon_cache.peek(qualified) {
            return hit;
        }
        let _members_span = bevy::log::info_span!("modelica_icon_members").entered();
        let falsy_params: HashSet<String> = self
            .inherited_members_typed(qualified)?
            .into_iter()
            .filter(|member| matches!(member.variability, InheritedVariability::Parameter))
            .filter(|member| {
                member.metadata.scalar_type
                    == Some(lunco_modelica_ast::PredefinedComponentType::Boolean)
                    && !member.metadata.has_array_dimensions
            })
            .filter(|member| member.default_value.as_deref() == Some("false"))
            .map(|member| member.name)
            .collect();
        drop(_members_span);
        let _merge_span = bevy::log::info_span!("modelica_icon_merge").entered();
        let top = self.class_def(qualified)?;
        let mut resolver = |scope: &str, reference: &str| {
            self.session
                .class_type_resolution_candidates_in_class_query(scope, reference)
                .into_iter()
                .find_map(|candidate| {
                    let resolved = self.session.class_lookup_query(&candidate)?;
                    (resolved == candidate).then_some(())?;
                    self.class_def(&resolved)
                        .map(|class| (resolved, std::sync::Arc::new(class)))
                })
        };
        let mut visited = HashSet::new();
        let icon = lunco_modelica_ast::annotations::project_inherited_icon(
            qualified,
            &top,
            &mut resolver,
            &falsy_params,
            &mut visited,
        );
        self.icon_cache.insert(qualified, icon.clone());
        icon
    }

    /// Read an icon only when the merged-icon memo already contains a result.
    ///
    /// This is the UI-side read contract: painting may consult a completed
    /// background resolution, but it must never start an inheritance walk or
    /// hold the shared engine mutex while doing so. `None` means either a
    /// cached negative or that the background resolver has not published a
    /// result yet; callers may use an icon authored directly on their local
    /// AST while waiting for the next projection.
    pub fn cached_icon_for(
        &mut self,
        qualified: &str,
    ) -> Option<Option<lunco_modelica_ast::annotations::Icon>> {
        self.icon_cache.peek(qualified)
    }

    /// Replace one complete parsed source set in the shared session.
    ///
    /// This is the canonical boundary for immutable library batches such as
    /// source library. Keeping the invalidation rules here makes the background bootstrap
    /// and a first class lookup observe the same engine state.
    pub fn replace_in_memory_source_set(
        &mut self,
        set_id: &str,
        kind: rumoca_compile::compile::SourceRootKind,
        files: Vec<rumoca_compile::compile::ParsedSourceDocument>,
    ) -> usize {
        let count = self
            .session
            .replace_in_memory_source_set(set_id, kind, files, None);
        if count > 0 {
            self.installed_source_sets.insert(set_id.to_string());
            lunco_modelica_ast::source_memo::invalidate_source_memos();
        }
        count
    }

    /// Whether a complete source set has crossed the shared-session boundary.
    pub fn source_set_installed(&self, set_id: &str) -> bool {
        self.installed_source_sets.contains(set_id)
    }

    /// Reserve a bundled source root for asynchronous loading. The caller
    /// performs the package read and parse off the update thread, then calls
    /// [`Self::finish_source_root_load`].
    pub fn begin_source_root_load(&mut self, root: &str) -> bool {
        if self.source_set_installed(root)
            || self.has_class(root)
            || self.pending_source_roots.contains(root)
            || self.failed_source_roots.contains_key(root)
        {
            return false;
        }
        self.pending_source_roots.insert(root.to_string())
    }

    /// Install an asynchronously parsed source root and publish one generic
    /// completion revision. The lock is held only for the pre-parsed session
    /// insertion; package I/O and rumoca parsing happen before this method.
    pub fn finish_source_root_load(
        &mut self,
        root: &str,
        files: Vec<rumoca_compile::compile::ParsedSourceDocument>,
        diagnostics: Vec<String>,
    ) -> usize {
        self.pending_source_roots.remove(root);
        if !diagnostics.is_empty() {
            self.failed_source_roots.insert(
                root.to_string(),
                format!(
                    "source root `{root}` has invalid Modelica members: {}",
                    diagnostics.join("; ")
                ),
            );
            return 0;
        }
        let count = self.load_parsed_library_files(files);
        if count > 0 {
            self.installed_source_sets.insert(root.to_string());
            self.failed_source_roots.remove(root);
            self.completed_source_roots.push(root.to_string());
        } else {
            self.failed_source_roots.insert(
                root.to_string(),
                format!("no Modelica source definitions could be loaded for `{root}`"),
            );
        }
        count
    }

    /// Complete a requested root from an admitted immutable source set.
    pub fn finish_resident_source_root(&mut self, root: &str) -> bool {
        self.pending_source_roots.remove(root);
        if self.has_class(root) {
            self.installed_source_sets.insert(root.to_owned());
            self.failed_source_roots.remove(root);
            self.completed_source_roots.push(root.to_owned());
            true
        } else {
            self.failed_source_roots.insert(
                root.to_owned(),
                format!("source root `{root}` is not declared in the installed source library"),
            );
            false
        }
    }

    /// Return the terminal loading error for a bundled source root, if any.
    pub fn source_root_failure(&self, root: &str) -> Option<&str> {
        self.failed_source_roots.get(root).map(String::as_str)
    }

    fn load_parsed_library_files(
        &mut self,
        files: Vec<rumoca_compile::compile::ParsedSourceDocument>,
    ) -> usize {
        let count = files.len();
        self.session.add_in_memory_parsed_batch(files);
        lunco_modelica_ast::source_memo::invalidate_source_memos();
        count
    }

    pub fn drain_completed_source_roots(&mut self) -> Vec<String> {
        std::mem::take(&mut self.completed_source_roots)
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

    /// Project Rumoca's complete inherited declarations into the application
    /// member ABI. Unresolved source facts have no complete member result.
    pub fn inherited_members_typed(&mut self, qualified: &str) -> Option<Vec<InheritedMember>> {
        let members = self.session.class_component_declarations_query(qualified)?;
        Some(
            members
                .iter()
                .map(|member| {
                    let component = &member.component;
                    let default_value = component
                        .binding
                        .as_ref()
                        .map(ToString::to_string)
                        .or_else(|| {
                            component
                                .start_is_modification
                                .then(|| component.start.to_string())
                        });
                    InheritedMember {
                        name: component.name.clone(),
                        type_name: member.type_name.clone(),
                        variability: map_variability_inherited(&component.variability),
                        causality: map_causality_inherited(&member.effective_causality),
                        default_value,
                        metadata: lunco_modelica_ast::ast_extract::component_metadata(
                            component,
                            member.scalar_type,
                            member.has_type_dimensions,
                        ),
                    }
                })
                .collect(),
        )
    }

    /// Capture semantic diagram facts and merged icons from the same session.
    /// Called by background projection; no source reads or parsing occur here.
    pub fn diagram_snapshot(
        &mut self,
        target: &str,
        scope: crate::diagram::DiagramScope,
    ) -> Result<crate::diagram::DiagramSnapshot, String> {
        let _span = bevy::log::info_span!("modelica_diagram_snapshot", class = target).entered();
        let _capture_span = bevy::log::info_span!("modelica_diagram_capture").entered();
        let mut snapshot =
            crate::diagram::DiagramSnapshot::capture(&mut self.session, target, scope)?;
        drop(_capture_span);
        for (name, class) in snapshot.classes_mut() {
            if scope == crate::diagram::DiagramScope::ComponentGraph || name == target {
                class.icon = self.icon_for(name);
            }
        }
        Ok(snapshot)
    }

    /// Read-only access to the underlying session for advanced queries
    /// not yet wrapped here. Use sparingly — prefer growing this
    /// crate's API over leaking the session through panels.
    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Retrieve a class declaration through Rumoca's current session index.
    /// The caller owns the returned definition independently of the engine lock.
    pub fn class_def(&mut self, qualified: &str) -> Option<rumoca_compile::parsing::ast::ClassDef> {
        self.session.class_definition_query(qualified).cloned()
    }

    /// Whether `qualified` resolves to a class currently in the
    /// session. Cheap — uses rumoca's existing
    /// `class_lookup_query`. Used as the first step in lazy source library
    /// loading: if the class isn't here, the caller resolves a
    /// file path, reads it, and pushes via
    /// `session_mut().add_document(...)`. Subsequent calls then
    /// return `true` without touching the filesystem.
    pub fn has_class(&mut self, qualified: &str) -> bool {
        self.session.class_lookup_query(qualified).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install an explicitly parsed inline source at the engine boundary.
    fn upsert_test(engine: &mut ModelicaEngine, id: DocumentId, src: &str) {
        let ast = rumoca_phase_parse::parse_to_ast(src, "test.mo").expect("test source must parse");
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
    fn completion_drain_is_bounded_and_fifo() {
        let mut engine = ModelicaEngine::new();
        let first = DocumentId::new(1);
        let second = DocumentId::new(2);
        let third = DocumentId::new(3);

        engine.finish_parse(
            first,
            lunco_modelica_document::SyntaxCache::from_source("model A end A;", 11),
        );
        engine.finish_parse(
            second,
            lunco_modelica_document::SyntaxCache::from_source("model B end B;", 22),
        );
        engine.finish_parse(
            third,
            lunco_modelica_document::SyntaxCache::from_source("model C end C;", 33),
        );

        assert!(engine.has_parse_work(first));

        assert_eq!(
            engine
                .drain_completed_up_to(2)
                .into_iter()
                .map(|(id, syntax)| (id, syntax.generation))
                .collect::<Vec<_>>(),
            vec![(first, 11), (second, 22)]
        );
        assert!(!engine.has_parse_work(first));
        assert_eq!(
            engine
                .drain_completed_up_to(2)
                .into_iter()
                .map(|(id, syntax)| (id, syntax.generation))
                .collect::<Vec<_>>(),
            vec![(third, 33)]
        );
        assert!(engine.drain_completed_up_to(1).is_empty());
    }

    #[test]
    fn class_def_follows_class_after_document_close() {
        let mut engine = ModelicaEngine::new();
        upsert_test(
            &mut engine,
            DocumentId::new(1),
            "model M\n  Real old;\nend M;\n",
        );
        assert!(engine.class_def("M").is_some());

        engine.close_document(DocumentId::new(1));
        upsert_test(
            &mut engine,
            DocumentId::new(2),
            "model M\n  Real replacement;\nend M;\n",
        );

        let class = engine
            .class_def("M")
            .expect("class_def must not retain the closed document URI");
        assert!(class.components.contains_key("replacement"));
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
    fn async_source_root_reservation_deduplicates_and_publishes_completion() {
        let mut engine = ModelicaEngine::new();
        assert!(engine.begin_source_root_load("DemoRoot"));
        assert!(!engine.begin_source_root_load("DemoRoot"));
        let files = vec![(
            "DemoRoot/Base.mo".to_string(),
            "within DemoRoot; model Base\n  Real x;\nend Base;\n".to_string(),
        )];
        let parsed = files
            .into_iter()
            .map(|(uri, source)| {
                rumoca_compile::compile::ParsedSourceDocument::parse(uri, source)
                    .expect("bundled source parses")
            })
            .collect();
        assert!(engine.finish_source_root_load("DemoRoot", parsed, Vec::new()) > 0);
        assert_eq!(engine.drain_completed_source_roots(), vec!["DemoRoot"]);
        assert!(!engine.begin_source_root_load("DemoRoot"));
    }

    #[test]
    fn failed_async_library_root_is_terminal_and_not_loading_forever() {
        let mut engine = ModelicaEngine::new();
        assert!(engine.begin_source_root_load("BrokenRoot"));
        assert_eq!(
            engine.finish_source_root_load("BrokenRoot", Vec::new(), Vec::new()),
            0
        );
        assert!(
            engine
                .source_root_failure("BrokenRoot")
                .is_some_and(|message| message.contains("no Modelica source definitions"))
        );
        assert!(!engine.begin_source_root_load("BrokenRoot"));
    }

    #[test]
    fn source_library_roots_complete_from_the_admitted_session() {
        let mut engine = ModelicaEngine::new();
        assert!(engine.begin_source_root_load("Library"));
        let document = rumoca_compile::compile::ParsedSourceDocument::parse("library.mo", "package Library connector Data=input Real; model Device Data demand; end Device; end Library;").unwrap();
        engine.replace_in_memory_source_set(
            "source-bundle",
            rumoca_compile::compile::SourceRootKind::DurableExternal,
            vec![document],
        );
        assert!(engine.finish_resident_source_root("Library"));
        assert_eq!(engine.drain_completed_source_roots(), vec!["Library"]);
        assert!(!engine.begin_source_root_load("Library"));
        let snapshot = engine
            .diagram_snapshot(
                "Library.Device",
                crate::diagram::DiagramScope::ComponentGraph,
            )
            .unwrap();
        assert_eq!(snapshot.members[0].type_name, "Library.Data");
        assert!(engine.begin_source_root_load("Missing"));
        assert!(!engine.finish_resident_source_root("Missing"));
        assert!(engine.source_root_failure("Missing").is_some());
        assert!(engine.drain_completed_source_roots().is_empty());
    }

    #[test]
    fn async_library_root_parse_diagnostics_are_terminal_and_visible() {
        let mut engine = ModelicaEngine::new();
        assert!(engine.begin_source_root_load("BrokenRoot"));
        assert_eq!(
            engine.finish_source_root_load(
                "BrokenRoot",
                Vec::new(),
                vec!["Broken.mo: invalid declaration".into()],
            ),
            0
        );
        assert!(
            engine
                .source_root_failure("BrokenRoot")
                .is_some_and(|message| message.contains("Broken.mo"))
        );
        assert!(!engine.begin_source_root_load("BrokenRoot"));
    }

    #[test]
    fn inherited_icon_uses_the_declaring_scope() {
        let mut engine = ModelicaEngine::new();
        upsert_test(
            &mut engine,
            DocumentId::new(1),
            r#"
model Base annotation(Icon(graphics={Rectangle(extent={{-10,-10},{10,10}})})); end Base;
model Imported annotation(Icon(graphics={Ellipse(extent={{-20,-20},{20,20}})})); end Imported;
package Local
  import Base = Imported;
  model Derived extends Base; end Derived;
end Local;
"#,
        );
        let icon = engine.icon_for("Local.Derived").expect("inherited icon");
        assert_eq!(icon.graphics.len(), 1);
        assert!(matches!(
            icon.graphics[0],
            lunco_modelica_ast::annotations::GraphicItem::Ellipse(_)
        ));
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

        let members = engine
            .inherited_members_typed("Derived")
            .expect("complete declarations");
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

        let members = engine
            .inherited_members_typed("Derived")
            .expect("complete declarations");
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

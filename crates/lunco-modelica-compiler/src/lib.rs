//! Headless Modelica compiler and source-admission host.
//!
//! This package owns the long-lived Rumoca session used by workers, runners,
//! asset tooling, and command-line hosts. It deliberately has no Bevy plugin:
//! the UI/document lifecycle belongs to `lunco-modelica-core`, while this
//! package is the shared production compiler used by both runtime and tests.

use rumoca_compile::{Session, SessionConfig};

const SOURCE_SET_REVISION_VERSION: u32 = 1;

fn source_set_revision(id: &str, files: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut entries = files
        .iter()
        .map(|(uri, source)| (uri.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    entries.sort_unstable();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    SOURCE_SET_REVISION_VERSION.hash(&mut hasher);
    id.hash(&mut hasher);
    for (uri, source) in entries {
        uri.hash(&mut hasher);
        source.hash(&mut hasher);
    }
    hasher.finish()
}

fn source_roots_from_parsed_docs(
    docs: &[(String, rumoca_compile::parsing::ast::StoredDefinition)],
) -> std::collections::HashSet<String> {
    docs.iter()
        .flat_map(|(_, definition)| {
            let within = definition.within.as_ref().map(ToString::to_string);
            definition.classes.keys().filter_map(move |class_name| {
                let qualified = within
                    .as_deref()
                    .map(|prefix| format!("{prefix}.{class_name}"))
                    .unwrap_or_else(|| class_name.clone());
                qualified.split('.').next().map(str::to_owned)
            })
        })
        .collect()
}

/// Compile Modelica models through one session-owned source-root admission
/// boundary and one strict reachable-DAE call. Source roots are admitted from
/// the parsed source before compilation; an unresolved dependency is a terminal
/// compile error rather than a signal to run the DAE pipeline again.
pub struct ModelicaCompiler {
    session: Session,
    /// Top-level package names whose source root is seated in this session —
    /// `"Modelica"`, any structured package under the application search path,
    /// and any other shipped library.
    ///
    /// This is the session's view of MODELICAPATH: Modelica looks up only the
    /// ROOT segment of a qualified name against the search path (`LunCo` is the
    /// root for `LunCo.Propulsion.BellNozzle`), loads that library once, and resolves
    /// everything below it inside the loaded tree. Keyed by that root segment for
    /// the same reason.
    ///
    /// It replaces separate per-library booleans, which recorded the same fact
    /// under hardcoded names and left every additional package without a place
    /// in the search path.
    ///
    /// Source-root installation records the top-level names it actually
    /// parsed here as well. The source-root command may be keyed by a mount
    /// identifier such as `twin:school`, while Modelica resolves a member by
    /// its authored `within` root such as `School`; recording the authored
    /// namespace is what lets later package-member compiles use the already
    /// seated root instead of registering a second URI.
    installed_roots: std::collections::HashSet<String>,
    /// Root segments referenced by the active source document. This is the
    /// compiler's parsed view of the current Modelica search path and keeps
    /// an unresolved reference from loading unrelated external packages.
    requested_source_roots: std::collections::HashSet<String>,
    /// URIs of the user documents currently seated as overlays in this
    /// reused session (NOT the resident source roots). Every compile is
    /// HERMETIC with respect to prior compiles: before seating its own
    /// document set, a compile evicts any previously-seated user doc that is
    /// not part of *this* compile. Without this, a prior compile's primary
    /// overlay stays resident, and compiling a SECOND document that defines
    /// the same package (e.g. the same model opened in two tabs / restored +
    /// shared) leaves the package registered under two URIs → rumoca's merge
    /// pass fails with `Duplicate class '…' found in '…' with non-identical
    /// definition`. The per-doc `session_uri` keying alone can't prevent this
    /// because the two docs legitimately have different URIs; the session
    /// itself must hold only the active compile's user docs.
    seated_user_uris: std::collections::HashSet<String>,
    /// Numeric `input` defaults captured from every library member seated
    /// through [`Self::load_source_root_in_memory`], keyed by the LEAF
    /// component name.
    ///
    /// The strip that makes a bound `input Real x = 3.0` a real runtime slot
    /// runs on every one of those members — but until this map existed the
    /// captured defaults were dropped on the floor (`let (stripped, _defaults)`),
    /// so a library class's input reached the stepper as a slot sitting at 0.0
    /// instead of at its authored default. That is precisely the silent fold the
    /// strip exists to prevent, just moved one seam along. The worker folds this
    /// map into each `CompileUnit` and re-seeds it through the same
    /// `apply_input_defaults_validated` the primary document uses.
    ///
    /// Leaf-keyed because that is what `SimulationSession::set_input` addresses;
    /// a library class is only ever reached by INSTANTIATION, so the worker
    /// resolves these against the flattened `<instance>.<leaf>` slots. Root
    /// source documents are not compiled as user targets, so they are not
    /// represented in this map.
    library_input_defaults: std::collections::HashMap<String, f64>,
    /// Content revisions of the source roots admitted into this session.
    /// These are computed from bytes already read by the admission boundary;
    /// the prepared solve cache uses the aggregate without rescanning disk.
    library_revisions: std::collections::HashMap<String, u64>,
}

impl Default for ModelicaCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelicaCompiler {
    /// Construct an empty compiler session.
    ///
    /// Source roots are admitted explicitly through
    /// [`Self::ensure_source_bundle_installed`],
    /// [`Self::ensure_source_root_installed`], or one of the source-root load
    /// methods. This keeps source selection at the caller that owns the
    /// authored source-root declaration instead of making construction scan
    /// or select a particular library.
    pub fn new() -> Self {
        Self {
            session: Session::new(SessionConfig::default()),
            installed_roots: std::collections::HashSet::new(),
            requested_source_roots: std::collections::HashSet::new(),
            seated_user_uris: std::collections::HashSet::new(),
            library_input_defaults: std::collections::HashMap::new(),
            library_revisions: std::collections::HashMap::new(),
        }
    }

    /// Install the resident parsed source bundle into this session. The bundle
    /// may contain any number of authored libraries; root identities are
    /// derived from their `within` declarations rather than selected by name.
    pub fn ensure_source_bundle_installed(&mut self) -> bool {
        let Some(parsed) = lunco_modelica_library::source_library::parsed_source_bundle() else {
            return false;
        };
        let docs = (**parsed).clone();
        let roots = source_roots_from_parsed_docs(&docs);
        if roots.is_empty() {
            return false;
        }
        let inserted = self.session.replace_parsed_source_set(
            "source-bundle",
            rumoca_compile::compile::SourceRootKind::DurableExternal,
            docs,
            None,
        );
        self.installed_roots.extend(roots);
        self.library_revisions.insert(
            "source-bundle".to_string(),
            source_set_revision("source-bundle", &[]),
        );
        inserted > 0
    }

    /// Seat a shipped Modelica library into this session by its TOP-LEVEL name,
    /// once. A root such as `LunCo` maps to the engine Modelica library's
    /// standard structured package (`package.mo` + `package.order` + members).
    ///
    /// This is MODELICAPATH lookup: the root segment of a qualified name names a
    /// library, the library is loaded whole, and everything below resolves inside
    /// the loaded tree from the members' own `within` declarations. Idempotent and
    /// cheap on repeat.
    ///
    /// The runtime asset tree is the source on native; browser consumers use the
    /// Bevy `ModelicaSource` loader. There is one source of truth, so an edited `.mo`
    /// cannot be compiled from a stale second copy. The disk tree is
    /// the one Bevy's AssetServer serves, so it is what `info:sourceAsset`
    /// already reads; taking the library from anywhere else would make an edited
    /// member compile as its last-built self while the scene loaded the new text.
    ///
    /// A library member is seated through [`Self::seat_user_source`] like any other
    /// user model, one document per `.mo`, so the bound-`input` strip applies to it.
    /// It did not when the disk copy went through rumoca's source-root loader: that
    /// reads the files itself, the strip never ran, and every `input Real x = <d>` in
    /// a library class was demoted to an algebraic — `input_names()` came back EMPTY,
    /// the cosim wire into it was rejected, and the model held its declared default
    /// for the whole run. `LunCo.Propulsion.PlumePhotometry` took `throttle` that way,
    /// which is why a descent burn lit no plume.
    pub fn ensure_source_root_installed(&mut self, root: &str) -> bool {
        if self.installed_roots.contains(root) {
            return true;
        }
        let live_dir = lunco_assets_core::models_package_root_path(root);
        let files = lunco_assets_runtime::models::package_files_live(root);
        if let Ok(files) = files {
            if files.is_empty() {
                return false;
            }
            let (label, source) = match live_dir {
                Some(dir) => (dir.display().to_string(), "the live asset tree"),
                None => (root.to_owned(), "the configured Modelica asset library"),
            };
            log::info!(
                "[ModelicaCompiler] seated library `{root}` from {source} ({})",
                files.len(),
            );
            let report = self.seat_library_files(root, &label, files);
            if !report.diagnostics.is_empty() || report.inserted_file_count == 0 {
                log::error!(
                    "[ModelicaCompiler] source root `{root}` failed: {}",
                    if report.diagnostics.is_empty() {
                        "no Modelica definitions were inserted".to_string()
                    } else {
                        report.diagnostics.join("; ")
                    },
                );
                return false;
            }
            self.installed_roots.insert(root.to_string());
            return true;
        }

        if self.ensure_source_bundle_installed() && self.installed_roots.contains(root) {
            return true;
        }

        // The same root-segment contract also covers a flat bundled model
        // (`Foo.mo` → root `Foo`). Use the existing bundled source path so
        // source-root admission covers both package and example entries in the
        // asset inventory.
        let filename = format!("{root}.mo");
        let Ok(Some(source)) = lunco_assets_runtime::models::model_source(&filename) else {
            return false;
        };
        log::info!("[ModelicaCompiler] seated bundled root `{root}` ({filename})",);
        let report = self.seat_library_files(
            root,
            &format!("bundled:{filename}"),
            vec![(filename, source)],
        );
        if !report.diagnostics.is_empty() || report.inserted_file_count == 0 {
            log::error!(
                "[ModelicaCompiler] bundled source root `{root}` failed: {}",
                if report.diagnostics.is_empty() {
                    "no Modelica definitions were inserted".to_string()
                } else {
                    report.diagnostics.join("; ")
                },
            );
            return false;
        }
        self.installed_roots.insert(root.to_string());
        true
    }

    /// Seat a whole library's members as documents, each through the
    /// bound-`input` strip — see [`Self::ensure_source_root_installed`].
    /// The strip itself lives in [`Self::load_source_root_in_memory`],
    /// so every in-memory root shares it.
    fn seat_library_files(
        &mut self,
        id: &str,
        label: &str,
        files: Vec<(String, String)>,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        self.load_source_root_in_memory(id, label, files)
    }

    /// Compile Modelica source string and return DAE result.
    ///
    /// Source-root dependencies are admitted before the user document enters
    /// Rumoca. Its strict-reachable DAE walker then sees one settled session,
    /// so short-form references resolve through normal Modelica scope lookup.
    ///
    /// `filename` is used as the document URI for error reporting.
    pub fn compile_str(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.requested_source_roots =
            lunco_modelica_index::source_deps::scan_source_root_deps_from_source(source, filename);
        // Root installation is idempotent and remains owned by this compiler
        // session. It must complete before the first DAE call so USD projection
        // cannot turn dependency discovery into a hidden second compile.
        self.prepare_requested_source_roots();
        // A `.mo` declaring `within P;` is a MEMBER of package `P`, not a document
        // that stands on its own. Its class is `P.Name`, and `P`'s source root
        // already owns it, so seating the file as a user overlay registers that
        // qualified class under a SECOND URI and rumoca's merge pass rejects the
        // pair — the same `Duplicate class '…' with non-identical definition` that
        // `seated_user_uris` exists to prevent between two user docs, arrived at
        // from the library side instead.
        //
        // That silently killed every USD prim pointing `info:sourceAsset`
        // at a package member (the lander's `BellNozzle`, both scenes'
        // `SunTracker`). The geometry still drew — the lathe is Rust-side — but the
        // model never solved. It could not have been hit before those existed: every
        // earlier target was a flat, `within`-less file.
        //
        // So do what Modelica does. `loadFile` is for documents; a library class is
        // reached with `loadModel` — resolve the ROOT segment against the search
        // path, seat that library, compile the qualified name. The file itself is
        // never handed to the compiler, which is why no duplicate is possible rather
        // than merely unlikely. `source` is still the authority on WHICH class this
        // is: its `within` clause and class name are read straight from the text the
        // caller loaded, not guessed from the file path.
        if let Some(within) = lunco_modelica_ast::ast_extract::within_package_of_source(source) {
            let root = within.split('.').next().unwrap_or(&within).to_string();
            let qualified = lunco_modelica_ast::ast_extract::qualify(
                &within,
                lunco_modelica_ast::ast_extract::short_name(model_name),
            );
            if !self.ensure_source_root_installed(&root) {
                return Err(format!(
                    "`{qualified}` declares `within {within};`, but no library `{root}` \
                     could be seated from the configured Modelica asset library"
                ));
            }
            return self.compile_loaded(&qualified);
        }
        if self.class_is_owned_by_installed_root(model_name) {
            return Err(format!(
                "`{filename}` declares `{model_name}`, but that class is already owned by a \
                 loaded Modelica source root; edit the source-root document or use a new \
                 namespace instead of registering a second definition"
            ));
        }
        let mut keep = std::collections::HashSet::new();
        keep.insert(filename.to_string());
        self.evict_user_docs_except(&keep);
        self.seat_user_source(filename, source);
        self.seated_user_uris = keep;
        self.compile_loaded(model_name)
    }

    /// Seat one user source into the session — **the single chokepoint where
    /// user model text enters rumoca**, and therefore the one place the
    /// bound-`input` workaround is applied.
    ///
    /// rumoca demotes a bound `input Real g = 9.81` to an algebraic, so it never
    /// reaches `input_names()` and `set_input("g", …)` fails
    /// (`docs/architecture/29-rumoca-workarounds.md` §2). Stripping here rather
    /// than at each caller means no compile path can forget it — `modelica_tester`
    /// silently did, and every future entry point would have been one `git grep`
    /// away from the same bug.
    ///
    /// Safe to apply to already-stripped source: [`strip_input_defaults`] blanks
    /// the binding bytes in place, so a second pass finds nothing to strip and is
    /// a no-op. Callers that need the defaults back (the worker, to re-seed them
    /// via `set_input`) still call it themselves — they get the same answer.
    ///
    /// The blanking is LENGTH-PRESERVING, so byte offsets into the raw source the
    /// caller still holds (diagnostic spans → editor click-to-source) keep
    /// pointing at the same characters.
    fn seat_user_source(&mut self, filename: &str, source: &str) {
        let (stripped, _defaults) = lunco_modelica_ast::ast_extract::strip_input_defaults(source);
        self.session.update_document(filename, &stripped);
    }

    /// Remove every previously-seated user-document overlay whose URI is not
    /// in `keep`, so the reused session holds ONLY the active compile's user
    /// docs (plus the immutable source roots). This is what makes
    /// each compile hermetic against prior compiles — see
    /// [`Self::seated_user_uris`]. source library roots are installed via
    /// `replace_parsed_source_set` (not tracked here), so they're untouched.
    fn evict_user_docs_except(&mut self, keep: &std::collections::HashSet<String>) {
        let stale: Vec<String> = self
            .seated_user_uris
            .iter()
            .filter(|uri| !keep.contains(*uri))
            .cloned()
            .collect();
        for uri in stale {
            self.session.remove_document(&uri);
            self.seated_user_uris.remove(&uri);
        }
    }

    /// Recompute **structured, located** diagnostics for a model that
    /// just failed [`Self::compile_str`].

    ///
    /// Where `compile_str`'s `Err(String)` is a flat human summary,
    /// this returns one [`Diagnostic`](lunco_doc::Diagnostic) per
    /// rumoca failure — each prefixed with the diagnostic code
    /// (e.g. `[ED008]`) and, when the failure's primary span points
    /// into the user's `model.mo`, carrying a 1-based (line, column)
    /// so the Diagnostics panel can make the row click-to-source
    /// (the same treatment parse + lint findings already get).
    ///
    /// Runs rumoca's report-returning strict compile
    /// (`compile_model_strict_reachable_uncached_with_recovery`),
    /// which reuses the session's already-resolved AST/source — so
    /// this is a cheap second pass, taken only on the error path.
    /// `user_uri` is the document URI the caller passed to
    /// `compile_str` (`"model.mo"` for the workbench); only spans in
    /// that file are made clickable.
    pub fn compile_diagnostics(
        &mut self,
        model_name: &str,
        user_uri: &str,
    ) -> Vec<lunco_doc::Diagnostic> {
        let report = self
            .session
            .compile_model_strict_reachable_uncached_with_recovery(model_name);
        diagnostics_from_strict_report(&report, user_uri)
    }

    /// Like `compile_str`, but seats additional `(filename, source)`
    /// pairs into the rumoca session before compiling so the resolver
    /// can satisfy cross-doc class references (e.g. a fresh untitled
    /// `RocketStage` referencing `AnnotatedRocketStage.Tank` from a
    /// sibling untitled doc that holds the package). Each extra is
    /// loaded via the same `update_document` path; rumoca dedups by
    /// filename so re-loading the same file is harmless.
    ///
    /// The active compile's user docs (primary + extras) become the session's
    /// ENTIRE user-overlay set: any previously-seated user doc that isn't part
    /// of this compile is evicted first (see [`Self::seated_user_uris`] /
    /// [`Self::evict_user_docs_except`]). This keeps the reused session
    /// hermetic — a prior compile of the same package under a different URI
    /// (e.g. the same model open in two tabs) can't linger and trip rumoca's
    /// `Duplicate class '…' found in '…' with non-identical definition`. The
    /// seated docs are left resident on return (the error-path
    /// `compile_diagnostics` re-reads them); they're pruned by the NEXT
    /// compile, not removed here.
    pub fn compile_str_multi(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.requested_source_roots =
            lunco_modelica_index::source_deps::scan_source_root_deps_from_source(source, filename);
        for (extra_filename, extra_source) in extras {
            self.requested_source_roots.extend(
                lunco_modelica_index::source_deps::scan_source_root_deps_from_source(
                    extra_source,
                    extra_filename,
                ),
            );
        }
        self.prepare_requested_source_roots();
        let primary_owned_class = lunco_modelica_ast::ast_extract::within_package_of_source(source)
            .and_then(|within| {
                let root = within.split('.').next().unwrap_or(&within);
                if !self.installed_roots.contains(root) {
                    let _ = self.ensure_source_root_installed(root);
                }
                let qualified = lunco_modelica_ast::ast_extract::qualify(
                    &within,
                    lunco_modelica_ast::ast_extract::short_name(model_name),
                );
                self.class_is_owned_by_installed_root(&qualified)
                    .then_some(qualified)
            });
        let mut keep = std::collections::HashSet::new();
        if primary_owned_class.is_none() {
            keep.insert(filename.to_string());
        }
        let mut filtered_extras = Vec::with_capacity(extras.len());
        for (extra_filename, extra_source) in extras {
            if extra_filename != filename {
                let declared = lunco_modelica_ast::ast_extract::declared_class_names(
                    extra_source,
                    extra_filename,
                );
                let owned = declared
                    .iter()
                    .filter(|qualified| self.class_is_owned_by_installed_root(qualified))
                    .count();
                if owned == declared.len() && !declared.is_empty() {
                    continue;
                }
                if owned != 0 {
                    return Err(format!(
                        "extra source `{extra_filename}` mixes classes already owned by a \
                         loaded source root with new definitions; update the source root or \
                         place the new classes under a distinct namespace"
                    ));
                }
                keep.insert(extra_filename.clone());
                filtered_extras.push((extra_filename, extra_source));
            }
        }
        self.evict_user_docs_except(&keep);
        for (extra_filename, extra_source) in filtered_extras {
            if extra_filename == filename {
                continue;
            }
            self.seat_user_source(extra_filename, extra_source);
        }
        if primary_owned_class.is_none() {
            self.seat_user_source(filename, source);
        }
        self.seated_user_uris = keep;
        match primary_owned_class {
            Some(qualified) => self.compile_loaded(&qualified),
            None => self.compile_loaded(model_name),
        }
    }

    /// Whether a fully-qualified class name belongs to a durable source root
    /// already seated in this compiler session. User overlays are deliberately
    /// not treated as durable ownership; they remain governed by
    /// `seated_user_uris` and are replaced by the next compile.
    fn class_is_owned_by_installed_root(&mut self, qualified: &str) -> bool {
        let Some(root) = qualified.split('.').next() else {
            return false;
        };
        self.installed_roots.contains(root) && self.session.class_lookup_query(qualified).is_some()
    }

    /// Compile a source-library class that is already loaded into the session
    /// (no `update_document` call). Used by the
    /// `lunco-modelica-assets` indexer's `--warm` pass to populate rumoca's
    /// semantic-summary cache for common
    /// examples — the workbench's first compile of those classes is
    /// then a cache hit instead of paying the full multi-minute walk.
    pub fn compile_library_class(
        &mut self,
        qualified: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.requested_source_roots.clear();
        self.compile_loaded(qualified)
    }

    /// Inner helper: heartbeat + session.compile + final timing log.
    /// Both [`Self::compile_str`] (user-edited source) and
    /// [`Self::compile_library_class`] (already-loaded source library class) flow
    /// through here so the heartbeat behaviour is identical.
    fn compile_loaded(
        &mut self,
        model_name: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        let t_total = web_time::Instant::now();

        // Heartbeat: rumoca's compile pipeline is opaque from outside
        // and can take minutes on cold caches with source library-heavy models
        // (parol Debug::fmt overhead — see ../rumoca/docs/design-notes/
        // perf-parol-trace-overhead.md). Without a periodic log line,
        // the user sees nothing for the entire duration and reasonably
        // assumes the worker hung. Spawn a tiny thread that emits an
        // INFO log every 5s while the synchronous compile is in
        // flight; signal it to stop on return.
        //
        // Wasm note: `std::thread::spawn` panics on wasm32-unknown-unknown
        // (single-threaded target). The compile already runs on the main
        // task there via the chunked browser-load path, so a heartbeat thread
        // would be unavailable and purely cosmetic. Skip it.
        use std::sync::atomic::{AtomicBool, Ordering};
        let still_compiling = std::sync::Arc::new(AtomicBool::new(true));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let stopper = std::sync::Arc::clone(&still_compiling);
            let model_for_thread = model_name.to_string();
            // Spawn detached — we deliberately do NOT join after compile
            // returns. The heartbeat sleeps in 5-second chunks; joining
            // would block the worker for up to a full tick (5 s) on EVERY
            // compile, even fast cache-hit ones. The workbench's
            // is_compiling flag would then stay set for that whole window,
            // and the Step dispatcher would idle visibly. Letting the
            // JoinHandle drop detaches the thread; it self-exits within
            // 5 s of `stopper=false` with at most one stray "still
            // compiling +N s" log line if the timing aligns badly.
            let _ = std::thread::spawn(move || {
                let started = web_time::Instant::now();
                let tick = std::time::Duration::from_secs(5);
                loop {
                    std::thread::sleep(tick);
                    if !stopper.load(Ordering::Relaxed) {
                        return;
                    }
                    log::info!(
                        "[ModelicaCompiler] still compiling `{}` (+{:.0}s)",
                        model_for_thread,
                        started.elapsed().as_secs_f64()
                    );
                }
            });
        }

        let result = self
            .session
            .compile_model_dae_strict_reachable_uncached_with_recovery(model_name);

        still_compiling.store(false, Ordering::Relaxed);
        // No `join` — see spawn comment above. The thread is detached
        // and will exit on its own within one tick.

        log::info!(
            "[ModelicaCompiler] compile `{}` finished in {:.2}s ({})",
            model_name,
            t_total.elapsed().as_secs_f64(),
            if result.is_ok() { "OK" } else { "ERR" },
        );
        result
    }

    /// Seat the source roots discoverable from the source text before the first
    /// compile attempt.  The compiler session is the sole owner of this
    /// operation; callers only provide source text and never duplicate the
    /// dependency inventory or root installation logic.
    fn prepare_requested_source_roots(&mut self) {
        let mut roots = self
            .requested_source_roots
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        roots.sort_unstable();
        for root in roots {
            if !self.installed_roots.contains(&root) {
                let _ = self.ensure_source_root_installed(&root);
            }
        }
    }

    /// Merge a Modelica source root into the live session so
    /// subsequent compiles can resolve its types. Used by the
    /// `LoadSourceRoot` worker command (`source_roots` lazy-load
    /// pipeline) — main thread sends this command before a Compile
    /// that depends on the library. Idempotent: rumoca dedups by
    /// `id`, so re-issuing for an already-loaded root is cheap.
    ///
    /// Blocks the worker thread for the duration of the parse. Other queued
    /// commands wait behind it.
    pub fn load_source_root(
        &mut self,
        id: &str,
        root_dir: &std::path::Path,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        // Every disk root holds classes that can be compile targets, so each
        // file must pass the bound-`input` strip. `load_source_root_tolerant`
        // parses off disk directly and would skip it (the `within P;`
        // member trap: rumoca demotes a bound input to an algebraic, the
        // model loses its runtime input slots and every wire is dropped),
        // so read the tree here and seat it through the in-memory path.
        let (files, read_diagnostics) =
            lunco_assets_runtime::discovery::read_files_with_extension(root_dir, "mo");
        for diagnostic in &read_diagnostics {
            log::warn!("[ModelicaCompiler] source root `{id}`: {diagnostic}");
        }
        let mut report =
            self.load_source_root_in_memory(id, &root_dir.display().to_string(), files);
        report.diagnostics.extend(
            read_diagnostics
                .into_iter()
                .map(|diagnostic| format!("source root `{id}`: {diagnostic}")),
        );
        report
    }

    /// Merge an in-memory source root (e.g. a bundled `.mo` file or
    /// a single workspace file) into the live session. Same
    /// idempotency + blocking semantics as
    /// [`Self::load_source_root`], but bytes are passed inline so
    /// callers without a real on-disk path can still install
    /// sources. Members are parsed first and installed as one source
    /// set so a large package invalidates Rumoca once, and a malformed
    /// member cannot leave a partially-installed root behind.
    ///
    /// `label` shows up in diagnostics as the "source root path"
    /// (rumoca convention: `"in-memory:<id>"`). `files` is a list
    /// of `(uri, source)` pairs; each `uri` is the filename rumoca
    /// will report errors against.
    ///
    /// Every file passes the bound-`input` strip here — this is the
    /// chokepoint for all in-memory roots (bundled deps, workspace
    /// files, twin libraries, disk roots read by
    /// [`Self::load_source_root`]), so no root member can reach the
    /// compiler with a demotable `input x = default` binding.
    ///
    /// Stripping alone is only half the contract: the defaults it removes are
    /// accumulated into [`Self::library_input_defaults`] so the worker can
    /// re-seed them onto the fresh stepper. These defaults keep each bound
    /// library input at its authored initial value when it is not wired.
    pub fn load_source_root_in_memory(
        &mut self,
        id: &str,
        label: &str,
        files: Vec<(String, String)>,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        let file_count = files.len();
        let mut diagnostics = Vec::new();
        let mut parsed = Vec::with_capacity(file_count);
        let mut uris = Vec::with_capacity(file_count);
        for (uri, text) in &files {
            let (stripped, defaults, issues) =
                lunco_modelica_ast::ast_extract::strip_input_defaults_with_report(text);
            uris.push(uri.clone());
            // A library member has no editor buffer to point diagnostics at, so
            // the report goes to the log — but it is never dropped: a parse
            // failure here means NOTHING in this file was stripped and every
            // bound input in it will be folded to a constant.
            for issue in &issues {
                match issue {
                    lunco_modelica_ast::ast_extract::InputDefaultIssue::ParseFailed => {
                        let message = format!(
                            "source root `{id}`: the bound-`input` strip could not parse {uri} — \
                             the file is seated unstripped, so bound inputs would be demoted and \
                             their wires discarded"
                        );
                        log::warn!("[ModelicaCompiler] {message}");
                        diagnostics.push(message);
                    }
                    lunco_modelica_ast::ast_extract::InputDefaultIssue::Unresolvable {
                        name,
                        binding,
                        ..
                    } => {
                        log::warn!(
                            "[ModelicaCompiler] source root `{id}`: {uri} declares `input {name} = \
                             {binding}` — an expression, not a literal, so the slot stays runtime \
                             but starts at 0.0 unless wired"
                        )
                    }
                    lunco_modelica_ast::ast_extract::InputDefaultIssue::Collision {
                        name,
                        kept_scope,
                        kept,
                        dropped_scope,
                        dropped,
                    } => log::warn!(
                        "[ModelicaCompiler] source root `{id}`: {uri} declares `{name}` in two \
                         scopes with different defaults — keeping {kept} from `{kept_scope}`, \
                         dropping {dropped} from `{dropped_scope}`"
                    ),
                }
            }
            for (name, value) in defaults {
                match self.library_input_defaults.entry(name) {
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(value);
                    }
                    std::collections::hash_map::Entry::Occupied(slot) if *slot.get() != value => {
                        // Two library members author the same leaf name with
                        // different defaults. Leaf keying can carry one; first
                        // seated wins (seating order is sorted, so this is
                        // deterministic) and the other is named rather than lost.
                        log::warn!(
                            "[ModelicaCompiler] source root `{id}`: input default `{}` = {value} \
                             in {uri} conflicts with {} already captured from another member — \
                             keeping the first. Rename one if they are different signals.",
                            slot.key(),
                            slot.get(),
                        );
                    }
                    std::collections::hash_map::Entry::Occupied(_) => {}
                }
            }
            match lunco_modelica_ast::parse_to_ast(&stripped, uri) {
                Ok(ast) => parsed.push((uri.clone(), ast)),
                Err(error) => {
                    let message = format!("source root `{id}`: could not parse {uri}: {error:?}");
                    log::warn!("[ModelicaCompiler] {message}");
                    diagnostics.push(message);
                }
            }
        }
        // A source root is one semantic unit. Do not publish a partial package:
        // the compile owner must either see every member or a terminal load
        // diagnostic. Bulk installation also keeps Rumoca's source-set index
        // and invalidation work to one pass instead of one pass per file.
        let inserted = if diagnostics.is_empty() && !parsed.is_empty() {
            let inserted = self.session.replace_parsed_source_set(
                id,
                rumoca_compile::compile::SourceRootKind::DurableExternal,
                parsed,
                None,
            );
            for uri in &uris {
                self.remember_source_roots_from_document(uri);
            }
            inserted
        } else {
            0
        };
        let report = rumoca_compile::compile::SourceRootLoadReport {
            source_set_id: id.to_string(),
            source_root_path: label.to_string(),
            parsed_file_count: file_count,
            inserted_file_count: inserted,
            cache_status: None,
            cache_key: None,
            cache_file: None,
            diagnostics,
        };
        if report.diagnostics.is_empty() && report.inserted_file_count > 0 {
            self.library_revisions
                .insert(id.to_string(), source_set_revision(id, &files));
        }
        report
    }

    /// Record the authored top-level namespace(s) supplied by one source-root
    /// document. A source-root identifier is transport metadata and may not be
    /// the Modelica namespace (`twin:demo` can contain `Demo.*`), so the
    /// compiler must derive this from the parsed document rather than guess
    /// from the URI or loader id.
    fn remember_source_roots_from_document(&mut self, uri: &str) {
        let roots: Vec<String> = self
            .session
            .parsed_file_query(uri)
            .map(|ast| {
                let within = ast.within.as_ref().map(ToString::to_string);
                ast.classes
                    .keys()
                    .filter_map(|class_name| {
                        let qualified = within
                            .as_deref()
                            .map(|prefix| format!("{prefix}.{class_name}"))
                            .unwrap_or_else(|| class_name.clone());
                        qualified.split('.').next().map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.installed_roots.extend(roots);
    }

    /// The `input` defaults captured from every seated library member — the
    /// other half of the strip that happens in
    /// [`Self::load_source_root_in_memory`]. The worker folds these into each
    /// `CompileUnit` so they are re-seeded through the SAME
    /// `apply_input_defaults_validated` path as the primary document's.
    pub fn library_input_defaults(&self) -> &std::collections::HashMap<String, f64> {
        &self.library_input_defaults
    }

    /// Return one deterministic revision for the complete set of admitted
    /// source roots. The ordering of the source-root map is not semantic, so
    /// root identifiers are sorted before hashing.
    pub fn library_revision(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut roots = self
            .library_revisions
            .iter()
            .map(|(id, revision)| (id.as_str(), *revision))
            .collect::<Vec<_>>();
        roots.sort_unstable_by(|left, right| left.0.cmp(right.0));

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        SOURCE_SET_REVISION_VERSION.hash(&mut hasher);
        for (id, revision) in roots {
            id.hash(&mut hasher);
            revision.hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// Convert a rumoca [`StrictCompileReport`] into the Diagnostics
/// panel's [`Diagnostic`](lunco_doc::Diagnostic) form.
///
/// A failure becomes *located* (click-to-source) only when its primary
/// label points at `user_uri` — diagnostics rooted in source library / sibling
/// library files keep their message (suffixed with the originating
/// file name) but no line/column, so clicking never jumps the editor
/// to a file it isn't showing. Each message is prefixed with rumoca's
/// diagnostic code when present (`ED008`, `EI001`, …) so the panel
/// reads like a real compiler's output.
///
/// [`StrictCompileReport`]: rumoca_compile::compile::StrictCompileReport
fn diagnostics_from_strict_report(
    report: &rumoca_compile::compile::StrictCompileReport,
    user_uri: &str,
) -> Vec<lunco_doc::Diagnostic> {
    use lunco_doc::Diagnostic;
    report
        .failures
        .iter()
        .map(|f| {
            let message = match &f.error_code {
                Some(code) if !code.is_empty() => format!("[{code}] {}", f.error),
                _ => f.error.clone(),
            };
            // Resolve the primary span against the report's source map.
            // `source_map` is `None` only on malformed reports; spans in
            // dummy/compiler-generated sources resolve to `None` here.
            let resolved = f.primary_label.as_ref().and_then(|label| {
                let sm = report.source_map.as_ref()?;
                let (name, content) = sm.get_source(label.span.source)?;
                Some((name.to_string(), content.to_string(), label.span.start.0))
            });
            match resolved {
                Some((name, content, start)) if name == user_uri => {
                    let (line, column) =
                        lunco_modelica_document::document::core::byte_offset_to_line_col(
                            &content, start,
                        );
                    Diagnostic::error(message, Some(line), Some(column))
                }
                // Diagnostic in another file — name it so the user knows
                // where it came from, but leave it unlocated.
                Some((name, _, _)) => Diagnostic::message_only(format!("{message}  (in {name})")),
                None => Diagnostic::message_only(message),
            }
        })
        .collect()
}

#[cfg(test)]
mod source_root_smoke {
    use super::*;

    #[test]
    fn source_root_reports_unstrippable_bound_input_files() {
        let mut compiler = ModelicaCompiler::new();
        let report = compiler.load_source_root_in_memory(
            "Broken",
            "test-root",
            vec![(
                "Broken.mo".into(),
                "model Broken\n  input Real x = 1.0;\n".into(),
            )],
        );
        assert_eq!(report.parsed_file_count, 1);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("could not parse Broken.mo")),
            "a source root must not report Ready when its bound-input strip failed: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn source_root_failure_does_not_publish_partial_members() {
        let mut compiler = ModelicaCompiler::new();
        let report = compiler.load_source_root_in_memory(
            "Demo",
            "test-root",
            vec![
                (
                    "Demo/Healthy.mo".into(),
                    "model Healthy end Healthy;".into(),
                ),
                ("Demo/Broken.mo".into(), "model Broken".into()),
            ],
        );
        assert_eq!(report.parsed_file_count, 2);
        assert_eq!(report.inserted_file_count, 0);
        assert!(!report.diagnostics.is_empty());
        let result = compiler.compile_str(
            "Consumer",
            "model Consumer\n  Demo.Healthy healthy;\nend Consumer;",
            "Consumer.mo",
        );
        assert!(
            result.is_err(),
            "a failed source-root admission must not expose only its parseable members"
        );
    }

    #[test]
    fn source_root_namespace_owns_later_package_member_compiles() {
        let mut compiler = ModelicaCompiler::new();
        let member = "within Demo;\nmodel Part\n  Real x;\nequation\n  x = 1;\nend Part;\n";
        let report = compiler.load_source_root_in_memory(
            "twin:demo",
            "in-memory:demo",
            vec![("demo/Part.mo".to_string(), member.to_string())],
        );
        assert_eq!(report.inserted_file_count, 1);
        assert!(report.diagnostics.is_empty(), "{report:?}");

        // The source root is keyed by a transport id (`twin:demo`), but the
        // package-member route must resolve the authored `within Demo;` root
        // and compile the already-seated class rather than adding a second
        // URI for the same qualified name.
        let result = compiler.compile_str("Part", member, "workspace/Part.mo");
        assert!(
            result.is_ok(),
            "a loaded source root must own its package members: {:?}",
            result.err()
        );

        let multi_result = compiler.compile_str_multi("Part", member, "workspace/Part.mo", &[]);
        assert!(
            multi_result.is_ok(),
            "the multi-document path must share source-root ownership: {:?}",
            multi_result.err()
        );
    }

    #[test]
    fn admitted_library_revision_is_order_independent_and_content_sensitive() {
        let first = vec![
            ("b.mo".to_string(), "model B end B;".to_string()),
            ("a.mo".to_string(), "model A end A;".to_string()),
        ];
        let mut reordered = first.clone();
        reordered.reverse();

        assert_eq!(
            source_set_revision("demo", &first),
            source_set_revision("demo", &reordered),
            "source admission revision must not depend on filesystem enumeration order"
        );
        assert_ne!(
            source_set_revision("demo", &first),
            source_set_revision(
                "demo",
                &[("a.mo".to_string(), "model A end A; changed".to_string())]
            ),
            "a source edit must invalidate prepared solve IR"
        );

        let mut compiler = ModelicaCompiler::new();
        let empty_revision = compiler.library_revision();
        let report = compiler.load_source_root_in_memory("demo", "test-root", first);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_ne!(
            empty_revision,
            compiler.library_revision(),
            "successful source-root admission must update the compiler revision"
        );
    }

    /// Trivial smoke test — compile a self-contained model with no
    /// source library references, verifying the source-root-independent path.
    #[test]
    fn bare_model_compiles_without_library() {
        let src = r#"
            model Bare
              Real x(start=1);
            equation
              der(x) = -x;
            end Bare;
        "#;
        let mut c = ModelicaCompiler::new();
        let r = c
            .compile_str("Bare", src, "Bare.mo")
            .expect("bare model must compile without source library");
        // Just assert we got a DAE at all — shape details vary
        // by rumoca version.
        let _ = r.dae;
    }
}

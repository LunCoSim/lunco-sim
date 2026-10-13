// Indexing admits paired source text and syntax through Rumoca's checked parser.
use lunco_modelica_ast::ClassType;
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
// `web_time::Instant` keeps the elapsed-time type portable if this module is
// inspected from a cross-target build; the module itself is native-only.
use web_time::Instant;

/// Native source-indexing options shared by command-line and library callers.
#[derive(Default, Clone, Debug)]
pub struct Options {
    /// Print per-file scan progress.
    pub verbose: bool,
    /// Run the warm-compile pass after indexing finishes. Heavy; off
    /// by default. Targets must be supplied explicitly with
    /// `--warm-only` or `LUNCOSIM_WARM_DIRS`; the indexer never embeds
    /// a product-specific model list.
    pub warm: bool,
    /// When `Some`, only warm-compile the listed fully-qualified
    /// class names. Implies `warm = true`.
    pub warm_only: Option<Vec<String>>,
    /// Native source root to index. `None` selects the canonical source library cache;
    /// the workbench sets this when a user configured a local source library root.
    pub(crate) source_root: Option<std::path::PathBuf>,
}

impl Options {
    /// Index the supplied native source library root while writing generated artifacts
    /// beside that root. The CLI keeps using the canonical cache through
    /// [`Default`].
    pub fn for_source_root(source_root: std::path::PathBuf) -> Self {
        Self {
            source_root: Some(source_root),
            ..Self::default()
        }
    }

    /// Parse from CLI args. Calls `std::process::exit` on unknown
    /// arguments or `--help` — only suitable from the binary entry
    /// point, not from inside the running app.
    pub fn parse() -> Self {
        let mut opts = Self::default();
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                "-v" | "--verbose" => opts.verbose = true,
                "--warm" => opts.warm = true,
                "--warm-only" => {
                    let list = iter.next().unwrap_or_else(|| {
                        eprintln!(
                            "error: --warm-only requires a comma-separated list of qualified names"
                        );
                        std::process::exit(2);
                    });
                    opts.warm_only = Some(
                        list.split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                    );
                    // An explicit target list also enables the warm pass.
                    opts.warm = true;
                }
                other => {
                    eprintln!("error: unknown argument `{other}` (use --help for usage)");
                    std::process::exit(2);
                }
            }
        }
        opts
    }
}

fn print_help() {
    println!(
        "modelica_library_indexer — index Modelica library components and optionally warm rumoca caches"
    );
    println!();
    println!("USAGE:");
    println!("  modelica_library_indexer [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("  -v, --verbose         Per-file logging during the scan pass");
    println!("      --warm            Warm explicitly supplied targets after indexing.");
    println!("      --warm-only LIST  Comma-separated qualified names or .mo paths to warm.");
    println!("  -h, --help            Show this help");
    println!();
    println!("OUTPUT:");
    println!("  library_index.json (next to the source root) — read by the workbench at startup");
    println!("  ~/Documents/luncosim-workspace/.cache/rumoca/parsed-files/ — populated as a side");
    println!(
        "    effect of the scan pass; --warm populates semantic-summaries for explicit targets."
    );
}

const PARSED_BUNDLE_ZSTD_LEVEL: i32 = 9;

/// Write the runtime's native parsed-source bundle beside the indexed source.
/// The compiler only reads this artifact; the host asset boundary is its sole
/// producer.
fn write_parsed_bundle(
    path: &std::path::Path,
    docs: &[rumoca_compile::compile::ParsedSourceDocument],
) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    encode_parsed_bundle(std::io::BufWriter::new(file), docs)
}

fn encode_parsed_bundle<W: std::io::Write>(
    writer: W,
    docs: &[rumoca_compile::compile::ParsedSourceDocument],
) -> std::io::Result<()> {
    let mut encoder = zstd::stream::write::Encoder::new(writer, PARSED_BUNDLE_ZSTD_LEVEL)?;
    bincode::serde::encode_into_std_write(docs, &mut encoder, bincode::config::standard())
        .map_err(std::io::Error::other)?;
    encoder.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Fallback strategy for ports without a Placement annotation
// ---------------------------------------------------------------------------

/// True when the top-level class `name` is actually the package
/// declared by the containing folder — i.e. the `package.mo` file
/// declares `package <FolderName> … end <FolderName>` per MLS.
///
/// Without this check, a naïve `"{current_path}.{name}"` join for
/// `Modelica/Blocks/package.mo` produces `Modelica.Blocks.Blocks`
/// instead of `Modelica.Blocks`. Nested classes then compound:
/// `Modelica.Blocks.Blocks.Examples.BooleanNetwork1`.
///
/// Two cases qualify:
///  1. `name == "package"` — files that
///     literally named the class `package`.
///  2. `is_package_file` AND the leaf segment of `current_path`
///     matches `name` — the source library-typical case.
fn is_top_level_self_ref(name: &str, current_path: &str, is_package_file: bool) -> bool {
    if name == "package" {
        return true;
    }
    if is_package_file {
        if let Some(leaf) = current_path.rsplit('.').next() {
            return leaf == name;
        }
    }
    false
}

struct SourceLibraryIndexer {
    /// Shared source-aware engine. Indexed ports, parameters and icons consume
    /// the same Rumoca-resolved snapshots as the workbench.
    ///
    /// Populated bulk after `scan_dir` finishes via
    /// `engine.session_mut().replace_in_memory_source_set("source-bundle", …)`
    /// — same code path the web bootstrap uses to install the
    /// prebuilt source library bundle. Indexer and runtime then have the
    /// SAME session shape; any `extends` chain that resolves at
    /// runtime resolves here too.
    engine: lunco_modelica_core::engine::ModelicaEngine,
    /// Flat map of fully-qualified-name → own ClassDef. Used for
    /// fields that don't need inheritance (description,
    /// class_kind and own diagram_graphics). Resolved members come from
    /// `engine.icon_for` and friends — single source of truth.
    classes: HashMap<String, ClassDef>,
    /// Per-file logging during scan_dir when true; otherwise a tick
    /// every couple seconds with running counters.
    verbose: bool,
    files_scanned: usize,
    bytes_scanned: usize,
    scan_started: Option<Instant>,
    last_progress_print: Option<Instant>,
    /// Bundle of every parsed `.mo` collected during the scan. Written
    /// at the end of `main()` to `.cache/library/parsed-library.bin` so the
    /// workbench can install paired source text and syntax
    /// via `Session::replace_in_memory_source_set` — mirrors the wasm
    /// runtime's `parsed-*.bin.zst` strategy on native.
    parsed_bundle: Vec<rumoca_compile::compile::ParsedSourceDocument>,
    source_errors: Vec<String>,
}

/// HTML-tag and whitespace-collapse patterns for [`clean_info_text`].
/// Compiled once (not per class) — `clean_info_text` runs once for each
/// of the source library declarations during an index build, and recompiling these
/// every call dominated that pass.
static TAG_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]*>").expect("tag regex"));
static WS_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\s+").expect("ws regex"));

/// Turn Rumoca's decoded documentation into UI-ready plain text.
/// Strips HTML tags and common entities, collapses whitespace, and keeps the first
/// paragraph (so a multi-screen source library doc fits in a card tagline).
fn clean_info_text(raw: &str) -> String {
    let mut s = raw.to_owned();

    // First-paragraph boundary: `</p>` is the source library convention; fall
    // back to a blank line so prose-only info strings still split.
    let lower = s.to_ascii_lowercase();
    if let Some(idx) = lower.find("</p>") {
        s.truncate(idx);
    } else if let Some(idx) = s.find("\n\n") {
        s.truncate(idx);
    }

    // Strip tags + entities using the module-level compiled patterns.
    let no_tags = TAG_RE.replace_all(&s, " ");
    let decoded = no_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    WS_RE.replace_all(&decoded, " ").trim().to_string()
}

impl SourceLibraryIndexer {
    fn new() -> Self {
        Self {
            engine: lunco_modelica_core::engine::ModelicaEngine::new(),
            classes: HashMap::new(),
            verbose: false,
            files_scanned: 0,
            bytes_scanned: 0,
            scan_started: None,
            last_progress_print: None,
            parsed_bundle: Vec::with_capacity(2700),
            source_errors: Vec::new(),
        }
    }

    /// Validate the complete parsed source set before publishing artifacts.
    fn validate_source_admission(&self) -> Result<(), String> {
        if !self.source_errors.is_empty() {
            return Err(format!(
                "source admission failed; no artifacts published: {}",
                self.source_errors.join("; ")
            ));
        }
        if self.parsed_bundle.is_empty() {
            return Err(
                "source admission failed; no Modelica sources found; no artifacts published".into(),
            );
        }
        Ok(())
    }

    /// Install the admitted source text in the engine's within-aware lookup index.
    fn install_into_engine(&mut self) {
        let count = self.parsed_bundle.len();
        let started = Instant::now();
        self.engine.replace_in_memory_source_set(
            "source-bundle",
            rumoca_compile::compile::SourceRootKind::DurableExternal,
            self.parsed_bundle.clone(),
        );
        println!(
            "[indexer] installed {count} docs into engine session in {:.1}s",
            started.elapsed().as_secs_f64()
        );
    }

    fn scan_dir(&mut self, dir: &Path, package_prefix: &str) {
        if self.scan_started.is_none() {
            self.scan_started = Some(Instant::now());
            self.last_progress_print = Some(Instant::now());
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        self.source_errors
                            .push(format!("{}: {error}", dir.display()));
                        continue;
                    }
                };
                let path = entry.path();
                if path.is_dir() {
                    let folder_name = path.file_name().unwrap().to_str().unwrap();
                    let new_prefix = if package_prefix.is_empty() {
                        folder_name.to_string()
                    } else {
                        format!("{}.{}", package_prefix, folder_name)
                    };
                    self.scan_dir(&path, &new_prefix);
                } else if path.extension().is_some_and(|ext| ext == "mo") {
                    if let Ok(source) = fs::read_to_string(&path) {
                        self.files_scanned += 1;
                        self.bytes_scanned += source.len();
                        // Verbose: one line per file as it's parsed.
                        // Quiet: a tick every 2s with running counters
                        // so the user sees liveness without 2.5k log
                        // lines.
                        if self.verbose {
                            let kb = source.len() as f64 / 1024.0;
                            println!(
                                "[scan] {} ({:.1} KB)",
                                path.strip_prefix(dir.ancestors().last().unwrap_or(dir))
                                    .unwrap_or(&path)
                                    .display(),
                                kb,
                            );
                        } else if let Some(last) = self.last_progress_print {
                            if last.elapsed() >= std::time::Duration::from_secs(2) {
                                let elapsed = self
                                    .scan_started
                                    .map(|t| t.elapsed().as_secs_f64())
                                    .unwrap_or(0.0);
                                let mb = self.bytes_scanned as f64 / (1024.0 * 1024.0);
                                println!(
                                    "[scan] {} files, {:.1} MB, {:.1}s elapsed (current: {})",
                                    self.files_scanned, mb, elapsed, package_prefix,
                                );
                                self.last_progress_print = Some(Instant::now());
                            }
                        }
                        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
                        self.ingest_file(&path, &source, &file_name, package_prefix);
                    } else {
                        self.source_errors
                            .push(format!("cannot read {}", path.display()));
                    }
                }
            }
        } else {
            self.source_errors
                .push(format!("cannot list {}", dir.display()));
        }
    }

    /// Parse and index a single `.mo` file. Extracted from the file
    /// branch of `scan_dir` so we can also ingest top-level companion
    /// files (e.g. `Complex.mo`) that live next to `Modelica/` rather
    /// than inside it.
    fn ingest_file(&mut self, path: &Path, source: &str, file_name: &str, package_prefix: &str) {
        // `package.mo` declares `package <FolderName> …
        // end <FolderName>` per MLS — the class inside IS the package,
        // so we must collapse rather than prefix. Track the file role so
        // both the placement mapping below and `add_stored_definition`
        // treat the class name correctly.
        let is_package_file = file_name == "package.mo";
        let parsed = match rumoca_compile::compile::ParsedSourceDocument::parse(
            path.to_string_lossy().to_string(),
            source.to_owned(),
        ) {
            Ok(document) => document,
            Err(error) => {
                self.source_errors
                    .push(format!("{}: {error}", path.display()));
                return;
            }
        };
        let ast = parsed.definition().clone();
        self.parsed_bundle.push(parsed);
        self.add_stored_definition(ast, package_prefix, is_package_file);
    }

    /// Top-level companion-file shorthand: load a flat `.mo` at the
    /// source library cache root with no package prefix. Used for `Complex.mo`
    /// and similar siblings of the main `Modelica/` tree.
    fn ingest_root_file(&mut self, path: &Path, source: &str) {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        self.ingest_file(path, source, &file_name, "");
    }

    fn add_stored_definition(
        &mut self,
        ast: StoredDefinition,
        current_path: &str,
        is_package_file: bool,
    ) {
        for (name, class) in ast.classes {
            let full_name = if is_top_level_self_ref(&name, current_path, is_package_file) {
                current_path.to_string()
            } else if current_path.is_empty() {
                name.to_string()
            } else {
                format!("{}.{}", current_path, name)
            };
            self.add_class(class, &full_name);
        }
    }

    fn add_class(&mut self, mut class: ClassDef, full_name: &str) {
        // The flat index stores each declaration once, without cloning its
        // descendants again at every level of the package hierarchy.
        for (nested_name, nested_class) in std::mem::take(&mut class.classes) {
            self.add_class(nested_class, &format!("{}.{}", full_name, nested_name));
        }
        self.classes.insert(full_name.to_string(), class);
    }

    fn index_all(&mut self) -> Vec<lunco_modelica_index::index::ClassEntry> {
        let mut all_comps = Vec::new();

        // Snapshot keys so we can iterate while mutably borrowing
        // `self.engine` for icon queries. The classes themselves
        // stay borrowed read-only via per-iteration `self.classes.get`.
        let mut names: Vec<String> = self.classes.keys().cloned().collect();
        names.sort();
        for (index, full_name) in names.iter().enumerate() {
            if self.verbose {
                println!(
                    "[indexer] interface {}/{}: {}",
                    index + 1,
                    names.len(),
                    full_name
                );
            }
            let full_name = full_name.as_str();
            let Some(class) = self.classes.get(full_name) else {
                continue;
            };
            if matches!(
                class.class_type,
                ClassType::Model | ClassType::Block | ClassType::Connector
            ) {
                let snapshot = self.engine.diagram_snapshot(
                    full_name,
                    lunco_modelica_core::diagram::DiagramScope::ClassInterface,
                );
                let (ports, parameters, icon_graphics, resolution, resolution_message) =
                    match snapshot {
                        Ok(snapshot) => {
                            let resolved =
                                snapshot.class(full_name).expect("captured target class");
                            (
                                snapshot.class_ports(resolved),
                                resolved.parameters(),
                                resolved.icon.clone(),
                                lunco_modelica_index::index::ClassResolutionState::Resolved,
                                None,
                            )
                        }
                        Err(error) => (
                            Vec::new(),
                            Vec::new(),
                            None,
                            lunco_modelica_index::index::ClassResolutionState::Missing,
                            Some(error),
                        ),
                    };

                let category = rumoca_core::parent_scope(full_name)
                    .unwrap_or("")
                    .replace('.', "/");

                // Diagram annotation — used when a connector instance
                // is rendered on a parent's diagram (carries the
                // `%name` Text label and the larger filled triangle
                // graphic that source library signal connectors use only in the
                // diagram view, not as port markers).
                let diagram_graphics =
                    lunco_modelica_ast::annotations::extract_diagram(&class.annotation);

                // Pull the first authored Text graphic's string for the
                // palette text fallback (used when a class has no
                // structural icon primitives but still labels itself).
                // Walks the typed Icon via `extract_icon` — same path
                // the workbench uses, no regex over Debug output.
                let icon_text = lunco_modelica_ast::annotations::extract_icon(&class.annotation)
                    .and_then(|icon| {
                        icon.graphics.iter().find_map(|g| match g {
                            lunco_modelica_ast::annotations::GraphicItem::Text(t) => {
                                Some(t.text_string.clone())
                            }
                            _ => None,
                        })
                    });

                let short_description =
                    lunco_modelica_ast::ast_extract::description_from_tokens(&class.description);
                let (documentation_info, _) =
                    lunco_modelica_index::doc_extract::extract_documentation(&class.annotation);
                let documentation_info = documentation_info.as_deref().map(clean_info_text);
                // `expandable connector` (MLS §9.1.3) is a connector
                // with the `expandable` keyword — folded into the
                // typed enum so consumers don't need a separate flag.
                let class_kind = match (&class.class_type, class.expandable) {
                    (lunco_modelica_ast::ClassType::Connector, true) => {
                        lunco_modelica_index::index::ClassKind::ExpandableConnector
                    }
                    (t, _) => lunco_modelica_index::index::map_class_type(t),
                };

                // Emit the canonical `ClassEntry` directly. Per-doc
                // runtime fields (source_range, extends, children,
                // equation_count, experiment) stay at their defaults —
                // the live AST producer fills them when a user opens
                // the file.
                all_comps.push(lunco_modelica_index::index::ClassEntry {
                    name: full_name.to_string(),
                    kind: class_kind,
                    description: short_description.unwrap_or_default(),
                    documentation: (documentation_info, None),
                    icon: icon_graphics,
                    diagram_graphics,
                    icon_text,
                    category,
                    partial: class.partial,
                    ports,
                    parameters,
                    source_range: None,
                    extends: Vec::new(),
                    children: Vec::new(),
                    equation_count: 0,
                    experiment: None,
                    resolution,
                    resolution_message,
                });
            }
        }
        all_comps
    }
}

/// Library entry point. Same workflow the CLI binary uses; safe to
/// invoke from inside the workbench (e.g. on a startup task that
/// follows a successful source library download). Prints progress to stdout —
/// callers that want structured progress should redirect stdout.
///
/// Non-cancellable; the workbench should call [`run_with_cancel`]
/// when it wants to be able to interrupt a long indexing pass.
/// The explicitly selected native source set as `(directory, package_prefix)`
/// package roots plus standalone `.mo` files.
///
/// This is the source-root discovery used by the native indexer.
/// It derives the package inventory from the
/// selected directory itself; no package name or cache sibling is special.
pub(crate) fn native_library_roots(
    library_root: &std::path::Path,
) -> (Vec<(std::path::PathBuf, String)>, Vec<std::path::PathBuf>) {
    let mut dirs: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut files: Vec<std::path::PathBuf> = Vec::new();

    if library_root.join("package.mo").is_file() {
        let prefix = library_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        if !prefix.is_empty() {
            dirs.push((library_root.to_path_buf(), prefix));
        }
        return (dirs, files);
    }

    let Ok(entries) = fs::read_dir(library_root) else {
        return (dirs, files);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if path.is_dir() && path.join("package.mo").is_file() {
            dirs.push((path, name));
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "mo") {
            files.push(path);
        }
    }
    dirs.sort_by(|left, right| left.1.cmp(&right.1));
    files.sort();
    (dirs, files)
}

pub fn run(opts: Options) -> Result<(), String> {
    run_with_cancel(opts, None)
}

/// Like [`run`] but checks `cancel` at phase boundaries and returns
/// early when it observes `true`. The granularity is per-phase
/// (`scan Modelica`, `scan companions`, `index_all`, `bundle write`),
/// so a cancel during the long initial scan still waits for the
/// directory walk to finish. Real per-file cancel would need
/// instrumenting `LIBRARYIndexer::scan_dir`; phase-level is enough for
/// the Settings → Assets → Cancel button.
pub fn run_with_cancel(
    opts: Options,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(), String> {
    let cancelled = || {
        cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    macro_rules! bail_if_cancelled {
        () => {
            if cancelled() {
                return Err("source library indexing cancelled".into());
            }
        };
    }
    bail_if_cancelled!();
    let library_root = opts
        .source_root
        .clone()
        .unwrap_or_else(lunco_modelica_library::source_library::source_library_cache_dir);
    if !library_root.is_dir() {
        return Err(format!(
            "source root not found at {}",
            library_root.display()
        ));
    }

    let t_total = Instant::now();
    println!(
        "[indexer] scanning source roots at {:?} (verbose={})",
        library_root, opts.verbose
    );

    let mut indexer = SourceLibraryIndexer::new();
    indexer.verbose = opts.verbose;

    // Single source of truth for the native source set (source tree +
    // companions + discovered third-party libs), shared with the
    // source-root discovery. `native_library_roots` returns folder
    // packages as `(dir, prefix)` and standalone companion files (e.g.
    // `Complex.mo`, referenced by Modelica.Fluid / ComplexBlocks)
    // separately, since the indexer keys flat files by declared class name
    // and folder packages by their `package.mo` `within` shape.
    let (root_dirs, companion_files) = native_library_roots(&library_root);
    for (dir, prefix) in &root_dirs {
        bail_if_cancelled!();
        println!("[indexer] scanning `{}` at {:?}", prefix, dir);
        indexer.scan_dir(dir, prefix);
    }
    for file in &companion_files {
        bail_if_cancelled!();
        if let Ok(source) = fs::read_to_string(file) {
            indexer.files_scanned += 1;
            indexer.bytes_scanned += source.len();
            indexer.ingest_root_file(file, &source);
        } else {
            indexer
                .source_errors
                .push(format!("cannot read {}", file.display()));
        }
    }

    indexer.validate_source_admission()?;

    let scan_secs = indexer
        .scan_started
        .map(|t| t.elapsed().as_secs_f64())
        .unwrap_or(0.0);
    let scan_mb = indexer.bytes_scanned as f64 / (1024.0 * 1024.0);
    println!(
        "[indexer] scan done: {} files, {:.1} MB in {:.1}s",
        indexer.files_scanned, scan_mb, scan_secs,
    );

    bail_if_cancelled!();
    println!("[indexer] indexing components (resolving inheritance)...");
    let t_index = Instant::now();
    // Bulk-install all parsed defs into the engine session BEFORE
    // index_all runs. After this, `engine.icon_for(name)` resolves
    // any class via rumoca's MLS § 5 lookup — same path the
    // workbench uses. No indexer-side resolver heuristic.
    indexer.install_into_engine();
    let components = indexer.index_all();
    println!(
        "[indexer] index done: {} components in {:.1}s",
        components.len(),
        t_index.elapsed().as_secs_f64()
    );

    // Bundled examples — small `.mo` files compiled into the workbench
    // binary at runtime from the external asset tree. Pre-parse their class
    // hierarchy here so the Package Browser can render them with
    // proper kind badges and expandable inner classes (matches source library /
    // workspace docs) without paying any parse cost at startup.
    let bundled_nodes = scan_bundled_examples();
    println!(
        "[indexer] bundled examples indexed: {} top-level nodes",
        bundled_nodes.len()
    );

    // Borrowing mirror of `lunco_modelica_index::visual_diagram::LibraryIndex` — same
    // field shape, but holds slices so we serialise without cloning
    // `components`/`bundled` into an owned `LibraryIndex`. Both fields are
    // the canonical types the runtime deserialises into directly.
    #[derive(Serialize)]
    struct LocalLibraryIndex<'a> {
        components: &'a [lunco_modelica_index::index::ClassEntry],
        bundled: &'a [lunco_modelica_index::package_tree::types::PackageNode],
    }
    let output_path =
        library_root.join(lunco_modelica_index::visual_diagram::LIBRARY_INDEX_FILE_NAME);
    let index = LocalLibraryIndex {
        components: &components,
        bundled: &bundled_nodes,
    };
    let json = serde_json::to_string_pretty(&index).unwrap();
    fs::write(&output_path, json).unwrap();
    println!(
        "[indexer] wrote {} components + {} bundled nodes → {}",
        components.len(),
        bundled_nodes.len(),
        output_path.display()
    );

    // Pre-parsed bundle for the workbench's fast path. Native mirror
    // of the wasm `parsed-*.bin.zst` artifact: bincode-serialised
    // `Vec<ParsedSourceDocument>` that the workbench installs
    // directly via `Session::replace_in_memory_source_set`, bypassing
    // every per-file cache key concern.
    bail_if_cancelled!();
    // The package is native-only for this module, while the surrounding asset
    // package keeps a wasm-safe empty library surface.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bundle_path = library_root
            .join(lunco_modelica_library::source_library::PARSED_LIBRARY_BUNDLE_FILE_NAME);
        let t_bundle = Instant::now();
        match write_parsed_bundle(&bundle_path, &indexer.parsed_bundle) {
            Ok(()) => {
                let mb = fs::metadata(&bundle_path)
                    .map(|m| m.len() as f64 / (1024.0 * 1024.0))
                    .unwrap_or(0.0);
                println!(
                    "[indexer] wrote parsed bundle (zstd): {} docs, {:.1} MB in {:.1}s → {}",
                    indexer.parsed_bundle.len(),
                    mb,
                    t_bundle.elapsed().as_secs_f64(),
                    bundle_path.display()
                );
            }
            Err(e) => {
                return Err(format!(
                    "cannot publish parsed bundle {}: {e}",
                    bundle_path.display()
                ));
            }
        }
    }

    if opts.warm {
        println!();
        warm_compile_pass(&opts);
    }

    println!();
    println!(
        "[indexer] all done in {:.1}s",
        t_total.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Parse every bundled `.mo` (compiled into `lunco_modelica` via
/// the external asset inventory and produce `PackageNode`s ready for the runtime
/// Package Browser to clone directly. No intermediate shape: the
/// indexer emits the exact tree the browser consumes, so the
/// runtime side is a trivial deserialise.
///
/// Pure function over the external example inventory. The asset boundary has
/// already loaded the source text; this function only parses each document.
fn scan_bundled_examples() -> Vec<lunco_modelica_index::package_tree::types::PackageNode> {
    use lunco_modelica_core::models::bundled_models;

    // Use the document's recovering syntax projection so the catalog retains
    // complete valid declarations and recoverable siblings with diagnostics.
    let Ok(models) = bundled_models() else {
        return Vec::new();
    };
    models
        .into_iter()
        .filter_map(|m| {
            let syntax = rumoca_phase_parse::parse_to_syntax(&m.source, &m.filename);
            let ast = syntax.best_effort();
            let (top_short, top_class) = ast.classes.iter().next()?;
            Some(bundled_class_node(&m.filename, top_short, top_class, ""))
        })
        .collect()
}

fn bundled_class_node(
    filename: &str,
    short_name: &str,
    class_def: &ClassDef,
    parent_path: &str,
) -> lunco_modelica_index::package_tree::types::PackageNode {
    use lunco_modelica_index::index::ClassKind;
    use lunco_modelica_index::package_tree::types::ModelSource;
    use lunco_modelica_index::package_tree::types::PackageNode;

    let qualified = lunco_modelica_ast::ast_extract::qualify(parent_path, short_name);
    let kind = lunco_modelica_index::index::map_class_type(&class_def.class_type);
    let id = format!("bundled://{filename}#{qualified}");
    let is_package = matches!(kind, ClassKind::Package);
    let children: Vec<PackageNode> = class_def
        .classes
        .iter()
        .map(|(child_short, child_def)| {
            bundled_class_node(filename, child_short, child_def, &qualified)
        })
        .collect();
    if is_package && !children.is_empty() {
        PackageNode::Category {
            id,
            name: short_name.to_string(),
            package_path: qualified,
            fs_path: std::path::PathBuf::new(),
            children: Some(children),
            is_loading: false,
        }
    } else {
        PackageNode::Model {
            id,
            name: short_name.to_string(),
            library: ModelSource::Bundled,
            class_kind: Some(kind),
        }
    }
}

/// Drive a full rumoca compile of every requested model so that
/// rumoca's semantic-summary cache (under `<cache>/rumoca/source-roots/
/// semantic-summaries/`) is populated. The workbench's first compile
/// of the same model is then a cache hit (ms instead of minutes).
///
/// Sources to compile come from three places, in priority order:
///   1. `--warm-only NAME[,NAME...]` — explicit qualified names or .mo
///      file paths. Anything containing `/`, `\`, or ending in `.mo`
///      is treated as a path; everything else as a source-library qualified name.
///   2. `LUNCOSIM_WARM_DIRS` env var — platform path-list of directories (`;` on Windows, `:` on Unix)
///      to scan for `*.mo` files. Every top-level model in each file
///      is warmed under its `<file_stem_or_package>.<model_name>`
///      qualified path.
///   3. If neither (1) nor (2) yielded anything, no warm work is performed.
///
/// Each compile is gated by the compiler host's strict reachable-DAE path;
/// existing 5-second heartbeat (see lib.rs), so even a multi-minute
/// source library-heavy compile prints proof-of-life every 5s.
fn warm_compile_pass(opts: &Options) {
    println!("[warm] starting compile pass — populating rumoca semantic-summary cache");
    let t_total = Instant::now();

    let mut compiler = lunco_modelica_compiler::ModelicaCompiler::new();
    // The warm pass compiles source library classes by name, so it needs the full
    // library resident up front. `new()` no longer preloads source library (Layer A:
    // source compilation admits roots from source text), so install it
    // explicitly here.
    compiler.ensure_source_bundle_installed();

    // Resolve work units. Each entry: (display_label, kind). The
    // `WarmKind` enum is declared at module scope so `push_file_units`
    // can refer to it.
    let mut units: Vec<(String, WarmKind)> = Vec::new();

    // (1) --warm-only — mixed paths and qualified names.
    if let Some(list) = &opts.warm_only {
        for item in list {
            if item.contains('/') || item.contains('\\') || item.ends_with(".mo") {
                push_file_units(&std::path::PathBuf::from(item), &mut units);
            } else {
                units.push((item.clone(), WarmKind::QualifiedClass(item.clone())));
            }
        }
    }

    // (2) LUNCOSIM_WARM_DIRS — scan dirs for .mo files.
    if let Some(dirs) = std::env::var_os("LUNCOSIM_WARM_DIRS") {
        for path in std::env::split_paths(&dirs).filter(|path| !path.as_os_str().is_empty()) {
            if !path.exists() {
                eprintln!(
                    "[warm] LUNCOSIM_WARM_DIRS entry does not exist: {}",
                    path.display()
                );
                continue;
            }
            if path.is_file() {
                push_file_units(&path, &mut units);
            } else if path.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().is_some_and(|e| e == "mo") {
                            push_file_units(&p, &mut units);
                        }
                    }
                }
            }
        }
    }

    if units.is_empty() {
        println!("[warm] no explicit targets; skipping compile pass");
        return;
    }

    println!("[warm] {} units to compile", units.len());
    for (i, (label, _)) in units.iter().enumerate() {
        println!("[warm]   {}/{}: {}", i + 1, units.len(), label);
    }
    println!();

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut total_compile_secs = 0.0f64;

    for (i, (label, kind)) in units.iter().enumerate() {
        println!("[warm] [{}/{}] compiling {} ...", i + 1, units.len(), label);
        let t = Instant::now();
        let result = match kind {
            WarmKind::QualifiedClass(qn) => compiler.compile_library_class(qn),
            WarmKind::FileWithSource {
                qualified,
                source,
                filename,
            } => compiler.compile_str(qualified, source, filename),
        };
        let secs = t.elapsed().as_secs_f64();
        total_compile_secs += secs;
        match result {
            Ok(_) => {
                println!(
                    "[warm] [{}/{}] ✓ {} compiled in {:.1}s",
                    i + 1,
                    units.len(),
                    label,
                    secs
                );
                succeeded += 1;
            }
            Err(e) => {
                let msg: String = e.chars().take(200).collect();
                println!(
                    "[warm] [{}/{}] ✗ {} FAILED in {:.1}s: {}",
                    i + 1,
                    units.len(),
                    label,
                    secs,
                    msg
                );
                failed += 1;
            }
        }
    }

    println!();
    println!(
        "[warm] done: {} succeeded, {} failed, total compile {:.1}s, wall {:.1}s",
        succeeded,
        failed,
        total_compile_secs,
        t_total.elapsed().as_secs_f64(),
    );
}

/// Read a `.mo` file and emit one warm unit per top-level class found
/// in it (model / block / package contents). Uses the lenient parser
/// so syntactically-broken files still surface what they can.
///
/// `qualified` follows MLS scoping: a top-level `model Foo` produces
/// `Foo`; a `package Foo { model Bar }` produces `Foo.Bar`.
fn push_file_units(path: &std::path::Path, units: &mut Vec<(String, WarmKind)>) {
    let Ok(source) = std::fs::read_to_string(path) else {
        eprintln!("[warm] read failed: {}", path.display());
        return;
    };
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("model.mo")
        .to_string();
    // Lenient parse to discover top-level classes. Errors don't kill
    // the warm — we still emit any classes the parser could salvage.
    let syntax = rumoca_phase_parse::parse_to_syntax(&source, &filename);
    let ast = syntax.best_effort();
    let mut emitted = 0;
    for (top_name, class_def) in &ast.classes {
        // If the top class is a package, descend one level so we warm
        // the actual models the user runs (the package itself isn't
        // simulable). One level is enough for the bundled assets;
        // deeper nesting would need recursion.
        if matches!(class_def.class_type, ClassType::Package) {
            for (inner_name, _) in &class_def.classes {
                let qualified = format!("{}.{}", top_name, inner_name);
                let label = format!("{} ({})", qualified, path.display());
                units.push((
                    label,
                    WarmKind::FileWithSource {
                        qualified,
                        source: source.clone(),
                        // Match what the workbench passes to
                        // compile_str so cache keys align: the
                        // workbench uses the literal "model.mo".
                        filename: "model.mo".to_string(),
                    },
                ));
                emitted += 1;
            }
        } else {
            let qualified = top_name.clone();
            let label = format!("{} ({})", qualified, path.display());
            units.push((
                label,
                WarmKind::FileWithSource {
                    qualified,
                    source: source.clone(),
                    filename: "model.mo".to_string(),
                },
            ));
            emitted += 1;
        }
    }
    if emitted == 0 {
        eprintln!(
            "[warm] no compilable classes found in {} (parse errors?)",
            path.display()
        );
    }
}

enum WarmKind {
    QualifiedClass(String),
    FileWithSource {
        qualified: String,
        source: String,
        filename: String,
    },
}

#[cfg(test)]
mod parsed_bundle_tests {
    use super::{SourceLibraryIndexer, encode_parsed_bundle};
    use std::io::Read;

    #[test]
    fn indexed_documentation_keeps_qualified_ownership_and_decoded_strings() {
        let mut indexer = SourceLibraryIndexer::new();
        indexer.ingest_file(std::path::Path::new("Docs.mo"), r#"package Docs package A model Part output Real y=1; annotation(Documentation(info="<p>first C:\\new</p>")); end Part; end A; package B model Part output Real y=2; annotation(Documentation(info="<p>second</p>")); end Part; end B; end Docs;"#, "Docs.mo", "");
        indexer.install_into_engine();
        let entries = indexer.index_all();
        let a = entries
            .iter()
            .find(|entry| entry.name == "Docs.A.Part")
            .unwrap();
        let b = entries
            .iter()
            .find(|entry| entry.name == "Docs.B.Part")
            .unwrap();
        assert_eq!(a.documentation.0.as_deref(), Some(r"first C:\new"));
        assert_eq!(b.documentation.0.as_deref(), Some("second"));
    }

    #[test]
    fn class_index_reads_interfaces_without_instantiating_internal_models() {
        let mut indexer = SourceLibraryIndexer::new();
        indexer.ingest_file(std::path::Path::new("Bounded.mo"), "package Bounded model Device Missing internal; end Device; model Root input Real demand; Device device; output Real delivered; end Root; end Bounded;", "Bounded.mo", "");
        indexer.install_into_engine();
        let entries = indexer.index_all();
        let root = entries
            .iter()
            .find(|entry| entry.name == "Bounded.Root")
            .unwrap();
        assert_eq!(
            root.resolution,
            lunco_modelica_index::index::ClassResolutionState::Resolved
        );
        assert_eq!(
            root.ports
                .iter()
                .map(|port| port.name.as_str())
                .collect::<Vec<_>>(),
            vec!["demand", "delivered"]
        );
        let device = entries
            .iter()
            .find(|entry| entry.name == "Bounded.Device")
            .unwrap();
        assert_eq!(
            device.resolution,
            lunco_modelica_index::index::ClassResolutionState::Missing
        );
    }

    #[test]
    fn empty_source_set_is_rejected_before_publication() {
        let indexer = SourceLibraryIndexer::new();
        assert!(indexer.validate_source_admission().is_err());
    }

    #[test]
    fn index_uses_resolved_ports_and_reports_invalid_declarations() {
        let mut indexer = SourceLibraryIndexer::new();
        indexer.ingest_file(
            std::path::Path::new("Facts.mo"),
            r#"
package Facts
  connector Data = input Real;
  model PinInput Real value; end PinInput;
  model Base Data demand; end Base;
  model Derived
    extends Base;
    PinInput ordinary;
  end Derived;
  model Invalid extends Missing; end Invalid;
  package A
    model Shared annotation(Documentation(info="<p>First class.</p>")); end Shared;
  end A;
  package B
    model Shared annotation(Documentation(info="<p>Second class.</p>")); end Shared;
  end B;
end Facts;
"#,
            "Facts.mo",
            "",
        );
        assert!(indexer.source_errors.is_empty());
        assert!(
            indexer
                .classes
                .values()
                .all(|class| class.classes.is_empty())
        );
        indexer.install_into_engine();
        let entries = indexer.index_all();
        let derived = entries
            .iter()
            .find(|entry| entry.name == "Facts.Derived")
            .unwrap();
        assert_eq!(derived.ports.len(), 1);
        assert_eq!(derived.ports[0].name, "demand");
        assert_eq!(derived.ports[0].library_path, "Facts.Data");
        assert_eq!(
            derived.ports[0].kind,
            lunco_modelica_index::visual_diagram::PortKind::Input
        );
        let invalid = entries
            .iter()
            .find(|entry| entry.name == "Facts.Invalid")
            .unwrap();
        assert_eq!(
            invalid.resolution,
            lunco_modelica_index::index::ClassResolutionState::Missing
        );
        assert!(invalid.resolution_message.is_some());
        for (name, documentation) in [
            ("Facts.A.Shared", "First class."),
            ("Facts.B.Shared", "Second class."),
        ] {
            assert_eq!(
                entries
                    .iter()
                    .find(|entry| entry.name == name)
                    .unwrap()
                    .documentation
                    .0
                    .as_deref(),
                Some(documentation),
            );
        }
        assert!(entries.windows(2).all(|pair| pair[0].name < pair[1].name));
    }

    /// The generated parsed-source artifact is a zstd-compressed bincode
    /// stream. Keep this codec contract next to its sole producer without
    /// pulling the asset writer into the compiler-host test target.
    #[test]
    fn parsed_bundle_rejects_invalid_source() {
        let mut indexer = SourceLibraryIndexer::new();
        indexer.ingest_file(
            std::path::Path::new("Bad.mo"),
            "model Bad Real x = ; end Bad;",
            "Bad.mo",
            "",
        );
        assert!(!indexer.source_errors.is_empty());
        assert!(indexer.parsed_bundle.is_empty());
    }

    #[test]
    fn parsed_bundle_roundtrips_in_memory() {
        let source = "function External input Real x; output Real y; external \"C\" y = external_call(x); end External; model M Real x; equation der(x) = -x; end M;";
        let docs = vec![
            rumoca_compile::compile::ParsedSourceDocument::parse("M.mo", source)
                .expect("parse sample model"),
        ];

        let mut encoded = Vec::new();
        {
            encode_parsed_bundle(&mut encoded, &docs).expect("encode parsed bundle");
        }
        assert_eq!(&encoded[..4], &[0x28, 0xB5, 0x2F, 0xFD]);

        let mut decoder =
            ruzstd::StreamingDecoder::new(encoded.as_slice()).expect("create zstd decoder");
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .expect("decode zstd stream");
        let (back, consumed): (Vec<rumoca_compile::compile::ParsedSourceDocument>, usize) =
            bincode::serde::decode_from_slice(&decoded, bincode::config::standard())
                .expect("decode parsed bundle");

        assert_eq!(back.len(), docs.len());
        assert_eq!(consumed, decoded.len());
        assert_eq!(back[0].uri(), docs[0].uri());
        assert_eq!(back[0].source(), source);
        assert!(back[0].definition().classes.contains_key("M"));
    }
}

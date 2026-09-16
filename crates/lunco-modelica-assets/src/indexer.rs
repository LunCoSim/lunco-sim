// Bulk indexing goes through `rumoca_compile::parsing::parse_files_parallel`,
// which routes every parse through rumoca's content-hash keyed artifact cache
// (`<workspace>/.cache/rumoca/parsed-files/`). Second indexer runs and
// the workbench's runtime drill-ins share the same cache entries, so
// a file parsed here is instant at runtime and vice versa.
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
use rumoca_compile::parsing::{Causality, ClassType, Variability};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
// `web_time::Instant` keeps the elapsed-time type portable if this module is
// inspected from a cross-target build; the module itself is native-only.
use web_time::Instant;

/// Indexer options. Used by both the CLI binary and the in-process
/// startup task in `NativeLibraryIndexerPlugin`. Kept tiny on purpose — adding
/// `clap` would pull megabytes of build into a tool whose whole point
/// is to make the workbench start faster.
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
    println!("modelica_library_indexer — index Modelica library components and optionally warm rumoca caches");
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
    docs: &[(String, rumoca_compile::parsing::StoredDefinition)],
) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    encode_parsed_bundle(std::io::BufWriter::new(file), docs)
}

fn encode_parsed_bundle<W: std::io::Write>(
    writer: W,
    docs: &[(String, rumoca_compile::parsing::StoredDefinition)],
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

/// How to assign a diagram position to a connector that carries no
/// `annotation(Placement(...))` declaration.
///
/// # Why this exists
/// The Modelica Language Specification (§18.6) defines the *format* of the
/// Placement annotation but **does not specify any default layout** when it is
/// absent. Quote: "The Placement annotation ... is used to define the placement
/// of the component in the diagram layer."  No default is stated — tools are free
/// to do whatever they want.
///
/// In practice, every source library connector declares an explicit Placement, so this
/// fallback only fires for:
///   - User-defined components that have no graphical layer at all
///   - Third-party libraries with incomplete annotations
///   - Components whose Placement the rumoca parser cannot yet extract
///
/// # Rationale for `SideByCausality` as the active default
/// Scanning the source library reveals an informal but consistent convention:
///   - causal `input`  connectors sit at (-100..110, ~0)  → left side
///   - causal `output` connectors sit at (+100..110, ~0)  → right side
///   - acausal connectors in `extends OnePort` / `TwoPort` follow the same
///     left/right pattern: `p` left, `n` right
///
/// This is **not a standard** — it is an observed pattern that produces
/// sensible schematics for the vast majority of library components.
///
fn fallback_port_position(causality: &Causality, port_index: usize) -> (f32, f32) {
    match causality {
        Causality::Input(_) => (-100.0, 0.0),
        Causality::Output(_) => (100.0, 0.0),
        _ => match port_index % 4 {
            0 => (-100.0, 0.0),
            1 => (100.0, 0.0),
            2 => (0.0, 100.0),
            _ => (0.0, -100.0),
        },
    }
}

// The indexer emits the canonical
// [`lunco_modelica_index::index::ClassEntry`] and
// [`lunco_modelica_index::visual_diagram::{PortDef, ParamDef}`] directly — `library_index.json`
// deserialises straight back into those types at runtime, so there is no
// indexer-local mirror to keep field-aligned by hand.

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
    /// Workspace + library engine. Receives every parsed `.mo`
    /// the indexer scans, then `engine.icon_for(name)` resolves
    /// inheritance the same way the runtime workbench does —
    /// rumoca's `class_lookup_query` does proper MLS § 5
    /// scope-walks, no indexer-side resolver heuristic.
    ///
    /// Populated bulk after `scan_dir` finishes via
    /// `engine.session_mut().replace_parsed_source_set("source-bundle", …)`
    /// — same code path the web bootstrap uses to install the
    /// prebuilt source library bundle. Indexer and runtime then have the
    /// SAME session shape; any `extends` chain that resolves at
    /// runtime resolves here too.
    engine: lunco_modelica_core::engine::ModelicaEngine,
    /// Flat map of fully-qualified-name → own ClassDef. Used for
    /// fields that don't need inheritance (description,
    /// class_kind, own diagram_graphics, port/param walks until
    /// they migrate too). Inheritance-merged data comes from
    /// `engine.icon_for` and friends — single source of truth.
    classes: HashMap<String, ClassDef>,
    /// Per-class first-paragraph plain-text from
    /// `annotation(Documentation(info="…"))`. Keyed by the simple
    /// class name (not fully-qualified) — good enough at source library scale
    /// because `Examples.*` class names are unique within a file
    /// and the browser looks it up from the `short_name`. Populated
    /// by `extract_documentation_infos` during `scan_dir` while the
    /// `.mo` source is still in memory.
    doc_infos: HashMap<String, String>,
    /// Per-file logging during scan_dir when true; otherwise a tick
    /// every couple seconds with running counters.
    verbose: bool,
    files_scanned: usize,
    bytes_scanned: usize,
    scan_started: Option<Instant>,
    last_progress_print: Option<Instant>,
    /// Bundle of every parsed `.mo` collected during the scan. Written
    /// at the end of `main()` to `.cache/library/parsed-library.bin` so the
    /// workbench can install pre-parsed `StoredDefinition`s in ~1s
    /// via `Session::replace_parsed_source_set` — mirrors the wasm
    /// runtime's `parsed-*.bin.zst` strategy on native.
    parsed_bundle: Vec<(String, StoredDefinition)>,
    /// `(path, source)` pairs captured during scan. Used by
    /// `install_into_engine` to feed the session via the full
    /// `add_document` pipeline so rumoca's `within`-aware lookup
    /// indexes are populated correctly. `replace_parsed_source_set`
    /// alone leaves scope-aware lookup blind to within-prefixed nested classes.
    sources: Vec<(std::path::PathBuf, String)>,
}

/// Scan a Modelica source buffer and map each class's simple name to
/// the **plain-text first paragraph** of its
/// `annotation(Documentation(info="…"))`, if any.
///
/// Walks the rumoca AST: parses the source, recursively visits every
/// nested class (`iter_classes`), and pulls the class-level
/// Documentation via the same `extract_documentation` helper that
/// the workbench's model_view panel uses. Consistent path for
/// boundary detection + doc extraction; no regex.
///
/// After matching, strip HTML tags and common entities, collapse
/// whitespace, and keep only the first paragraph (`</p>` boundary,
/// falling back to a double-newline). Dropping the rest means the
/// index stays small (~200 examples × < 200 chars each).
///
/// Last-write wins on duplicate short names (different source library files can
/// define classes with the same simple name; the indexer keys by
/// short name for the palette tagline lookup, full qualified names
/// are matched elsewhere).
fn extract_documentation_infos(source: &str) -> HashMap<String, String> {
    let Ok(ast) = lunco_modelica_ast::parse_to_ast(source, "library.mo") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, String> = HashMap::new();
    for (name, class_def) in &ast.classes {
        collect_documentation(name, class_def, &mut out);
    }
    out
}

/// Recursively visit a class and its nested classes, recording each
/// one's `Documentation(info=…)` keyed by short name.
fn collect_documentation(
    short_name: &str,
    class_def: &rumoca_compile::parsing::ast::ClassDef,
    out: &mut HashMap<String, String>,
) {
    let (info, _revisions) =
        lunco_modelica_index::doc_extract::extract_documentation(&class_def.annotation);
    if let Some(info) = info {
        out.insert(short_name.to_string(), clean_info_text(&info));
    }
    for (nested_name, nested_def) in class_def.iter_classes() {
        collect_documentation(nested_name, nested_def, out);
    }
}

/// HTML-tag and whitespace-collapse patterns for [`clean_info_text`].
/// Compiled once (not per class) — `clean_info_text` runs once for each
/// of the ~2700 source library classes during an index build, and recompiling these
/// every call dominated that pass.
static TAG_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]*>").expect("tag regex"));
static WS_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\s+").expect("ws regex"));

/// Turn a raw Modelica `info="…"` string into UI-ready plain text.
/// Unescapes Modelica string escapes, strips HTML tags and common
/// entities, collapses whitespace, and keeps only the first
/// paragraph (so a multi-screen source library doc fits in a card tagline).
fn clean_info_text(raw: &str) -> String {
    // Modelica string escapes we actually see in source library.
    let mut s = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => s.push('\n'),
                Some('t') => s.push('\t'),
                Some('"') => s.push('"'),
                Some('\\') => s.push('\\'),
                Some(other) => {
                    s.push('\\');
                    s.push(other);
                }
                None => s.push('\\'),
            }
        } else {
            s.push(c);
        }
    }

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
            doc_infos: HashMap::new(),
            verbose: false,
            files_scanned: 0,
            bytes_scanned: 0,
            scan_started: None,
            last_progress_print: None,
            parsed_bundle: Vec::with_capacity(2700),
            sources: Vec::with_capacity(2700),
        }
    }

    /// Install each file into the engine's session via the full
    /// `add_document` pipeline. `replace_parsed_source_set` looked
    /// like the right call (it's what web bootstrap uses) but
    /// the old annotation walker called
    /// `find_class_def_in_file` which expects `qualified_name` to
    /// walk through `parsed.classes` directly — and source library's flat
    /// per-file `within X; model Y end Y;` shape doesn't have
    /// that structure (the file's `parsed.classes` is just `{Y}`,
    /// not nested under X). `add_document` goes through rumoca's
    /// full pipeline which handles `within` correctly when
    /// indexing scope. rumoca's content-hash artifact cache makes
    /// the parse near-free on the second pass since the indexer
    /// already populated it via `parse_files_parallel`.
    fn install_into_engine(&mut self) {
        let count = self.sources.len();
        let started = Instant::now();
        let sources = std::mem::take(&mut self.sources);
        let session = self.engine.session_mut();
        for (path, source) in &sources {
            let uri = path.to_string_lossy().to_string();
            let _ = session.add_document(&uri, source);
        }
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
            for entry in entries.flatten() {
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
                    }
                }
            }
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
        // Parse through rumoca-compile's cache. A content-hash-matching
        // entry at `.cache/rumoca/parsed-files/` deserialises from
        // bincode in ~ms; a miss pays the full rumoca parse once and
        // writes the bincode so the NEXT indexer run and the workbench's
        // first drill-in are both instant. `parse_files_parallel` with
        // one path is the public entry point that exercises the cache;
        // rayon overhead is negligible for length-1.
        let ast_opt = rumoca_compile::parsing::parse_files_parallel(&[path.to_path_buf()])
            .ok()
            .and_then(|mut pairs| pairs.pop().map(|(_, ast)| ast));
        if let Some(ast) = ast_opt {
            for (k, v) in extract_documentation_infos(source) {
                self.doc_infos.entry(k).or_insert(v);
            }
            self.parsed_bundle
                .push((path.to_string_lossy().to_string(), ast.clone()));
            // Capture source for engine population. add_document
            // needs raw text so rumoca's full pipeline runs (which
            // populates the within-aware lookup tables that
            // scope-aware lookup tables read).
            self.sources.push((path.to_path_buf(), source.to_string()));
            self.add_stored_definition(ast, package_prefix, is_package_file);
        }
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

    fn add_class(&mut self, class: ClassDef, full_name: &str) {
        for (nested_name, nested_class) in class.classes.clone() {
            self.add_class(nested_class, &format!("{}.{}", full_name, nested_name));
        }
        self.classes.insert(full_name.to_string(), class);
    }

    /// Resolve a (possibly relative) `name` referenced from within
    /// `context_class` by peeling the context's enclosing scope one
    /// segment at a time and checking `self.classes` for a match.
    /// Returns the first fully-qualified key that exists, or `None`.
    ///
    /// Reduced-form MLS §5.3 scope walk shared by `extends`-base and
    /// component-type resolution below. NOTE: this is an indexer-side
    /// heuristic — it doesn't consult imports and falls back on a
    /// `Modelica.`-prefixed guess (at the `extends` call site). The
    /// runtime resolves via rumoca's `class_lookup_query`; see the
    /// icon resolver in `index_all`.
    ///
    /// CQ-109: the scope-chain candidate order is generated by the crate's
    /// **single canonical** resolver, [`lunco_modelica_ast::scope_chain_candidates`],
    /// rather than a hand-rolled parent walk — so index-time `extends`/type
    /// resolution can't drift from the runtime diagram projector that uses
    /// the same generator. This probes the indexer's `self.classes`; the
    /// projector probes the lazily-loaded source library/engine index instead.
    fn resolve_in_scope(&self, context_class: &str, name: &str) -> Option<String> {
        lunco_modelica_ast::scope_chain_candidates(name, Some(context_class))
            .into_iter()
            .find(|cand| self.classes.contains_key(cand))
    }

    fn resolve_inheritance(
        &self,
        class_name: &str,
        ports: &mut Vec<lunco_modelica_index::visual_diagram::PortDef>,
        params: &mut Vec<lunco_modelica_index::visual_diagram::ParamDef>,
        visited: &mut HashSet<String>,
    ) {
        if visited.contains(class_name) {
            return;
        }
        visited.insert(class_name.to_string());

        if let Some(class) = self.classes.get(class_name) {
            // 1. Resolve base classes first (extends)
            for ext in &class.extends {
                let base_short_name = ext
                    .base_name
                    .name
                    .iter()
                    .map(|s| s.text.to_string())
                    .collect::<Vec<String>>()
                    .join(".");

                // Scope-chain resolution; an unresolved reference remains
                // unresolved and is reported by the normal compiler path.
                let mut resolved_base = self.resolve_in_scope(class_name, &base_short_name);
                if resolved_base.is_none() {
                    if self.classes.contains_key(&base_short_name) {
                        resolved_base = Some(base_short_name);
                    }
                }

                if let Some(base) = resolved_base {
                    self.resolve_inheritance(&base, ports, params, visited);
                }
            }

            // 2. Add local components
            for comp in class.components.values() {
                if matches!(comp.variability, Variability::Parameter(_))
                    && !params.iter().any(|p| p.name == comp.name)
                {
                    // Format the default value for `%paramName`
                    // text substitution at render time. Prefer
                    // the explicit binding (`= expr`); fall back
                    // to `start=` modification (`parameter Real
                    // R(start=1)`) when no binding is present.
                    // Numeric and string literals show as-written;
                    // enum refs collapse to the leaf name (matches
                    // OMEdit); arithmetic and array literals use the shared
                    // AST display projection.
                    let default = comp
                        .binding
                        .as_ref()
                        .map(lunco_modelica_ast::ast_extract::format_expression_for_display)
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| {
                            // `comp.start: Expression` — Empty when no
                            // explicit start was given. The shared display
                            // projection returns "" for `Empty` so this is safe.
                            lunco_modelica_ast::ast_extract::format_expression_for_display(
                                &comp.start,
                            )
                        });
                    // TODO: resolve `unit` from the type definition.
                    // For `parameter SI.Torque tau_constant` the
                    // authoritative unit lives on `Modelica.Units.SI.Torque`
                    // as `type Torque = Real(unit="N.m")`. Resolve
                    // `comp.type_name` through the scope chain +
                    // imports, walk the `extends Real(unit=...)`
                    // modification, and store the result here so
                    // the canvas substitution (currently using a
                    // hand-maintained table in
                    // `canvas_diagram::si_unit_suffix`) can read
                    // `p.unit` directly. Until then `unit` is None
                    // and user-defined SI types lose their suffix.
                    params.push(lunco_modelica_index::visual_diagram::ParamDef {
                        name: comp.name.clone(),
                        param_type: comp.type_name.to_string(),
                        default,
                        unit: None,
                    });
                }

                let type_str = comp.type_name.to_string();
                let lower = type_str.to_lowercase();

                let is_port = lower.contains("pin")
                    || lower.contains("flange")
                    || lower.contains("port")
                    || lower.contains("input")
                    || lower.contains("output");

                let has_causality = matches!(comp.causality, Causality::Input(_))
                    || matches!(comp.causality, Causality::Output(_));

                if is_port || has_causality {
                    // Skip conditional connectors (e.g. `BooleanInput
                    // reset if use_reset` on Continuous.Integrator).
                    // They're declared in the type's interface but
                    // *not instantiated* unless the condition is true.
                    // Including them in the index made every Integrator
                    // instance render extra port dots for ports that
                    // aren't actually present in this instance.
                    //
                    // We're conservative — `condition.is_some()` is
                    // enough; we don't try to evaluate the condition.
                    // Worst case: a connector that's always-on via
                    // `if true` gets dropped, which is fine for the
                    // index (the user can still wire it; the dot just
                    // won't pre-render).
                    //
                    // TODO: per-instance conditional resolution.
                    // -----------------------------------------------
                    // The current uniform skip is correct for the
                    // common "default-off source library conditional" case but
                    // creates a UX gap when a user *enables* the
                    // conditional on a specific instance (e.g.
                    // `Integrator integrator(use_reset=true)`):
                    // simulation works, but the canvas never renders
                    // the `reset` dot, so the user can't drag a wire
                    // to it in the diagram editor.
                    //
                    // The fix is a 3-step upgrade:
                    //   1. Index the conditional ports too — add
                    //      `PortDef.conditional: Option<String>`
                    //      storing the condition expression source
                    //      (e.g. `"use_reset"`).
                    //   2. In the canvas projector, for each
                    //      conditional port consult the *instance's*
                    //      modifications (Integrator(use_reset=true))
                    //      with the class's parameter default as
                    //      fallback. Decide render-vs-skip per
                    //      instance.
                    //   3. Render conditionally-on ports in a slightly
                    //      different style (dashed outline) so users
                    //      see "this port only exists because the
                    //      parameter is on."
                    //
                    // Most source library conditions are plain boolean parameter
                    // refs (`use_reset`, `useSupport`, `useHeatPort`),
                    // so a 90%-coverage implementation is small.
                    if comp.condition.is_some() {
                        continue;
                    }
                    // Skip protected components — they're internal to
                    // the model (e.g. Integrator's `local_reset` /
                    // `local_set`) and shouldn't render as external
                    // ports. OMEdit / Dymola don't draw them either.
                    if comp.is_protected {
                        continue;
                    }

                    if !ports.iter().any(|p| p.name == comp.name) {
                        // Read the placement straight from rumoca's
                        // typed annotation tree — same code path the
                        // workbench uses at runtime
                        // (`lunco_modelica_ast::annotations::extract_placement`).
                        // Replaces the prior text-regex scan that
                        // (1) couldn't pull `origin=` when authored
                        // before `extent=`, (2) silently dropped
                        // placements declared in nested-class scopes,
                        // and (3) was source library-specific by virtue of being
                        // unable to handle parser variations from
                        // other Modelica libraries. Going through
                        // rumoca means any library rumoca can parse
                        // also gets correctly-positioned ports.
                        let placement =
                            lunco_modelica_ast::annotations::extract_placement(&comp.annotation);
                        // Shared extent→centre/size math (see
                        // `Transformation::centre_size`); position and size
                        // fall back independently when no placement is given.
                        let centre_size =
                            placement.as_ref().map(|p| p.transformation.centre_size());
                        let (x, y) = centre_size
                            .map(|(cx, cy, _, _)| (cx as f32, cy as f32))
                            .unwrap_or_else(|| {
                                fallback_port_position(&comp.causality, ports.len())
                            });
                        let (size_x, size_y) = centre_size
                            .map(|(_, _, w, h)| (w as f32, h as f32))
                            .unwrap_or((20.0, 20.0));
                        let rotation_deg = placement
                            .as_ref()
                            .map(|p| p.transformation.rotation as f32)
                            .unwrap_or(0.0);

                        // Resolve `type_str` to a fully-qualified path so
                        // runtime callers (canvas port-icon renderer,
                        // wire-color resolver) can look the connector
                        // class up directly via `class_cache`. Without
                        // this, `parameter RealInput u` writes
                        // `library_path = "RealInput"` and downstream
                        // resolution fails.
                        //
                        // Mirrors the scope-chain walk used above for
                        // `extends` resolution (no `Modelica.` fallback —
                        // an unresolved type keeps its as-written name).
                        let resolved_path = if self.classes.contains_key(&type_str) {
                            type_str.clone()
                        } else {
                            self.resolve_in_scope(class_name, &type_str)
                                .unwrap_or_else(|| type_str.clone())
                        };
                        ports.push(lunco_modelica_index::visual_diagram::PortDef {
                            name: comp.name.clone(),
                            connector_type: type_str.clone(),
                            library_path: resolved_path,
                            is_flow: is_port,
                            x,
                            y,
                            size_x,
                            size_y,
                            rotation_deg,
                            // Indexer doesn't compute these; the live
                            // projector/painter fills wire color, port
                            // kind, and flow-var metadata at runtime.
                            color: None,
                            kind: lunco_modelica_index::visual_diagram::PortKind::default(),
                            flow_vars: Vec::new(),
                        });
                    }
                }
            }
        }
    }

    fn index_all(&mut self) -> Vec<lunco_modelica_index::index::ClassEntry> {
        use std::sync::Arc;
        let mut all_comps = Vec::new();

        // Snapshot keys so we can iterate while mutably borrowing
        // `self.engine` for icon queries. The classes themselves
        // stay borrowed read-only via per-iteration `self.classes.get`.
        let names: Vec<String> = self.classes.keys().cloned().collect();
        for full_name in &names {
            let full_name = full_name.as_str();
            let class = match self.classes.get(full_name) {
                Some(c) => c.clone(),
                None => continue,
            };
            let class = &class;
            // Original loop body resumes; `class` is &ClassDef, an
            // owned clone here so we can freely &mut self for engine
            // queries below. ClassDef cloning is cheap relative to
            // an inheritance walk.
            if matches!(
                class.class_type,
                ClassType::Model | ClassType::Block | ClassType::Connector
            ) {
                let mut ports = Vec::new();
                let mut parameters = Vec::new();
                let mut visited = HashSet::new();

                self.resolve_inheritance(full_name, &mut ports, &mut parameters, &mut visited);

                let short_name = lunco_modelica_ast::ast_extract::short_name(full_name).to_string();
                let category =
                    lunco_modelica_ast::ast_extract::parent_qualified(full_name).replace('.', "/");

                // Inheritance-merged icon. The merge logic lives in
                // `extract_icon_inherited`; the resolver does
                // class-name → ClassDef lookup. Use rumoca's
                // `class_lookup_query` for the **lookup** (full MLS § 5
                // scope-chain, no indexer-side heuristic), then fetch
                // the ClassDef from `self.classes` (which is keyed by
                // qualified name and populated during scan).
                //
                // Why two layers: rumoca's
                // Rumoca's query would do this end-to-end, but its internal
                // file lookup
                // expects `parsed.classes` to contain the full-path
                // nesting — and source library's `within X; model Y end Y;`
                // shape only puts `Y` directly under `parsed.classes`.
                // Our `self.classes` map sidesteps that — we pre-built
                // the nested-name keying in `add_stored_definition`.
                let resolver_classes = &self.classes;
                let session = self.engine.session_mut();
                let mut resolver = |name: &str| -> Option<Arc<ClassDef>> {
                    // rumoca's MLS § 5 lookup. Returns the full
                    // qualified name we keyed on.
                    let qualified = session.class_lookup_query(name)?;
                    resolver_classes.get(&qualified).cloned().map(Arc::new)
                };
                let mut icon_visited = HashSet::new();
                let icon_graphics = lunco_modelica_ast::annotations::extract_icon_inherited(
                    full_name,
                    class,
                    &mut resolver,
                    &mut icon_visited,
                );
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
                let documentation_info = self.doc_infos.get(&short_name).cloned();
                // `expandable connector` (MLS §9.1.3) is a connector
                // with the `expandable` keyword — folded into the
                // typed enum so consumers don't need a separate flag.
                let class_kind = match (&class.class_type, class.expandable) {
                    (rumoca_compile::parsing::ClassType::Connector, true) => {
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
                    resolution: lunco_modelica_index::index::ClassResolutionState::Resolved,
                    resolution_message: None,
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
/// This is the single source of truth for the CLI indexer and the workbench's
/// cold-cache bundle builder. It derives the package inventory from the
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

/// Recursively collect every `.mo` file under `dir` into `out`.
#[cfg(not(target_arch = "wasm32"))]
fn collect_mo_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_mo_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "mo") {
                out.push(path);
            }
        }
    }
}

/// Parse every native source library `.mo` file into `(uri, StoredDefinition)` pairs,
/// **one file at a time**. The workbench's cold-cache bundle builder and
/// the in-app source library bundle builder.
///
/// **Why not `rumoca_compile::parsing::parse_files_parallel`?** That routes
/// every file through rumoca_compile's *global* in-memory artifact-cache
/// mutex (+ per-file disk-cache writes) on the *shared global* rayon pool.
/// Run on the workbench worker thread *alongside the Bevy main thread*
/// (which drives that same pool + locks every frame: drill-in, engine
/// async parse, icon/§5 lookups), it self-destructs: per-file dispatch →
/// futex convoy (~282% CPU, 99 threads in `futex_do_wait`); one batch
/// dispatch → mutex spin-storm (~2600% CPU, no progress). Either way an
/// 8 s standalone parse becomes *minutes* in-app. The cost is contention,
/// not parse work.
///
/// So we bypass the cache layer entirely: parse each file with the **raw,
/// lock-free** `lunco_modelica_ast::parse_to_ast` (exactly what
/// `parse_files_parallel` calls internally — identical AST) on a
/// **dedicated, bounded** rayon pool. No shared global pool, no global
/// mutex → no contention with the render loop; capped threads leave cores
/// for rendering and bound the memory peak; and we skip rumoca's in-memory
/// cache duplicate (we persist our own `parsed-library.bin`, the real cache).
/// Uses [`native_library_roots`] (same root set as the CLI) → identical bundle.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_native_library_bundle() -> Vec<(String, StoredDefinition)> {
    use rayon::prelude::*;
    let library_root = lunco_assets_core::source_library_dir("library");
    let (root_dirs, companion_files) = native_library_roots(&library_root);
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for (dir, _prefix) in &root_dirs {
        collect_mo_files(dir, &mut paths);
    }
    paths.extend(companion_files);

    // Leave a couple of cores for the render loop; 16 MB stacks for deep
    // nested-class recursion (matches rumoca's own pool sizing).
    // `LUNCO_LIBRARY_PARSE_THREADS` overrides the count (tuning / weak machines).
    let threads = std::env::var("LUNCO_LIBRARY_PARSE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().saturating_sub(2).max(1))
                .unwrap_or(2)
        });
    let started = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .stack_size(16 * 1024 * 1024)
        .build();
    let bundle: Vec<(String, StoredDefinition)> = match pool {
        Ok(pool) => pool.install(|| paths.par_iter().filter_map(|p| parse_one_mo(p)).collect()),
        Err(e) => {
            log::warn!("[library-bundle] dedicated pool build failed ({e}); parsing sequentially");
            paths.iter().filter_map(|p| parse_one_mo(p)).collect()
        }
    };
    log::info!(
        "[library-bundle] parsed {} source library files (raw parser, {} threads) in {:.1}s",
        bundle.len(),
        threads,
        started.elapsed().as_secs_f64()
    );
    bundle
}

/// Read + parse one `.mo` with the raw, lock-free parser. `None` on read or
/// parse error (logged). The bundle uri is the file path (matches the CLI).
#[cfg(not(target_arch = "wasm32"))]
fn parse_one_mo(path: &std::path::Path) -> Option<(String, StoredDefinition)> {
    let src = std::fs::read_to_string(path).ok()?;
    match lunco_modelica_ast::parse_to_ast(&src, &path.to_string_lossy()) {
        Ok(ast) => Some((path.to_string_lossy().to_string(), ast)),
        Err(e) => {
            log::warn!(
                "[library-bundle] parse failed for `{}`: {e}",
                path.display()
            );
            None
        }
    }
}

pub fn run(opts: Options) {
    run_with_cancel(opts, None);
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
) {
    let cancelled = || {
        cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };
    macro_rules! bail_if_cancelled {
        () => {
            if cancelled() {
                println!("[indexer] cancelled");
                return;
            }
        };
    }
    bail_if_cancelled!();
    // Point rumoca at the same on-disk parse cache the workbench
    // uses (`<workspace>/.cache/rumoca`), so a run here warms the
    // cache for the app and vice versa. Same one-liner as
    // `ClassCachePlugin::build` — keeps all tooling cache under
    // one roof. Honors an explicit `RUMOCA_CACHE_DIR` the user set.
    if std::env::var_os("RUMOCA_CACHE_DIR").is_none() {
        let target = lunco_assets_core::cache_dir().join("rumoca");
        std::env::set_var("RUMOCA_CACHE_DIR", &target);
        println!("[indexer] using rumoca parse cache at {}", target.display());
    }

    let library_root = opts
        .source_root
        .clone()
        .unwrap_or_else(|| lunco_assets_core::source_library_dir("library"));
    if !library_root.is_dir() {
        println!("[indexer] source root not found at {:?}", library_root);
        return;
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
    // workbench's cold-cache bundle builder (`parse_native_library_bundle`) so
    // both scan exactly the same files. `native_library_roots` returns folder
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
        }
    }

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
    // binary at runtime via `include_dir!()`. Pre-parse their class
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
    let output_path = library_root.join("library_index.json");
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
    // `Vec<(uri, StoredDefinition)>` that the workbench installs
    // directly via `Session::replace_parsed_source_set`, bypassing
    // every per-file cache key concern.
    bail_if_cancelled!();
    // The package is native-only for this module, while the surrounding asset
    // package keeps a wasm-safe empty library surface.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bundle_path = library_root.join("parsed-library.bin");
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
            Err(e) => eprintln!(
                "[indexer] WARN: failed to write parsed bundle to {}: {e}",
                bundle_path.display()
            ),
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
}

/// Parse every bundled `.mo` (compiled into `lunco_modelica` via
/// `include_dir!`) and produce `PackageNode`s ready for the runtime
/// Package Browser to clone directly. No intermediate shape: the
/// indexer emits the exact tree the browser consumes, so the
/// runtime side is a trivial deserialise.
///
/// Pure function over the in-memory `bundled_models()` list — no
/// disk I/O beyond what `include_dir!` already inlined at compile
/// time, so the cost is `n * parse(file)`, ≤ ~10 small files.
fn scan_bundled_examples() -> Vec<lunco_modelica_index::package_tree::types::PackageNode> {
    use lunco_modelica_core::models::bundled_models;

    // `parse_to_syntax(...).best_effort()` is the same path
    // `SyntaxCache::from_source` uses and is what the workspace
    // browser already renders cleanly — it preserves the full
    // nested class list, including `partial connector` siblings
    // that the bare `parse_to_recovered_ast` recovery parser
    // truncates after the first error-ish token.
    bundled_models()
        .into_iter()
        .filter_map(|m| {
            let syntax = lunco_modelica_ast::parse_to_syntax(m.source, m.filename);
            let ast = syntax.best_effort();
            let (top_short, top_class) = ast.classes.iter().next()?;
            Some(bundled_class_node(m.filename, top_short, top_class, ""))
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
///   2. `LUNCOSIM_WARM_DIRS` env var — `:`-separated list of directories
///      to scan for `*.mo` files. Every top-level model in each file
///      is warmed under its `<file_stem_or_package>.<model_name>`
///      qualified path.
///   3. If neither (1) nor (2) yielded anything, no warm work is performed.
///
/// Each compile is gated by [`lunco_modelica_core::ModelicaCompiler::compile_loaded`]'s
/// existing 5-second heartbeat (see lib.rs), so even a multi-minute
/// source library-heavy compile prints proof-of-life every 5s.
fn warm_compile_pass(opts: &Options) {
    println!("[warm] starting compile pass — populating rumoca semantic-summary cache");
    let t_total = Instant::now();

    let mut compiler = lunco_modelica_core::ModelicaCompiler::new();
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
        let dirs = dirs.to_string_lossy().to_string();
        for dir in dirs.split(':').filter(|s| !s.is_empty()) {
            let path = std::path::PathBuf::from(dir);
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
    let syntax = lunco_modelica_ast::parse_to_syntax(&source, &filename);
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
    use super::encode_parsed_bundle;
    use std::io::Read;

    /// The generated parsed-source artifact is a zstd-compressed bincode
    /// stream. Keep this codec contract next to its sole producer without
    /// pulling the asset writer into the compiler-core test target.
    #[test]
    fn parsed_bundle_roundtrips_in_memory() {
        let source = "model M Real x; equation der(x) = -x; end M;";
        let definition =
            lunco_modelica_ast::parse_to_ast(source, "M.mo").expect("parse sample model");
        let docs = vec![("M.mo".to_string(), definition)];

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
        let back: Vec<(String, rumoca_compile::parsing::StoredDefinition)> =
            bincode::serde::decode_from_slice(&decoded, bincode::config::standard())
                .expect("decode parsed bundle")
                .0;

        assert_eq!(back.len(), docs.len());
        assert_eq!(back[0].0, docs[0].0);
    }
}

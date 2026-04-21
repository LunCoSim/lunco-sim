//! Two-tier class cache: `FileCache` (PathBuf → parsed file) +
//! `ClassCache` (qualified name → class entry sharing a FileCache
//! parse).
//!
//! # Why two tiers
//!
//! MSL files come in two shapes: **own-file classes**
//! (`Capacitor.mo` → one class) and **package-aggregated files**
//! (`Continuous.mo` → holds `Der`, `Integrator`, `PID`, `FirstOrder`,
//! …). A qualified-name-only cache wastes work on the second shape:
//! drilling into `Der` parses `Continuous.mo`; drilling into
//! `Integrator` tomorrow re-parses it. A file-path cache keyed by
//! the `.mo` file turns sibling drill-ins into free lookups because
//! the one `Arc<StoredDefinition>` is shared by every class inside.
//!
//! The generic `ResourceCache` engine from [`lunco_cache`] drives
//! both tiers — only the loader differs. `ClassCache` doesn't
//! spawn tasks itself; it chases pending qualified names by
//! watching `FileCache` each frame and promoting `pending → ready`
//! when the file lands.
//!
//! # Layering
//!
//! ```text
//!     cache.request("Modelica.Blocks.Continuous.Der")
//!       │
//!       ├─ resolve qualified → "Continuous.mo" (static MSL index)
//!       │
//!       └─ file_cache.request(Continuous.mo)   [one parse per file]
//!            │
//!            └─ when FileEntry ready, ClassCache builds a
//!               CachedClass referencing its Arc<AstCache>.
//! ```
//!
//! ASTs and sources live once in [`FileEntry`]; every
//! [`CachedClass`] points at the same `Arc<str>` + `Arc<AstCache>`.
//! Memory cost of N classes sharing M files is O(M) parses, not
//! O(N) parses.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use std::path::PathBuf;
use std::sync::Arc;

use crate::document::AstCache;
use lunco_cache::{ResourceCache, ResourceLoader};

// ═══════════════════════════════════════════════════════════════════
// File tier: one parse per .mo file, shared by all classes inside.
// ═══════════════════════════════════════════════════════════════════

/// One parsed `.mo` file. `source` and `ast` are `Arc` so every
/// class referencing this file shares them — many `CachedClass`
/// entries point at the same two `Arc`s.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub source: Arc<str>,
    pub ast: Arc<AstCache>,
}

#[derive(Debug)]
pub enum FileLoadError {
    Io(std::io::Error),
}

impl std::fmt::Display for FileLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

pub struct ModelicaFileLoader;

impl ResourceLoader for ModelicaFileLoader {
    type Key = PathBuf;
    type Value = FileEntry;
    type Error = FileLoadError;

    fn load(&self, key: &PathBuf) -> Task<Result<FileEntry, FileLoadError>> {
        let path = key.clone();
        info!("[FileCache] scheduling load for `{}`", path.display());
        AsyncComputeTaskPool::get().spawn(async move {
            let t0 = std::time::Instant::now();
            // Read source once so we can hand it to both the
            // rumoca-session parse path (which carries its own
            // content-hash keyed artifact cache — in-memory + on-disk
            // bincode across app restarts) AND to our FileEntry for
            // UI-side text/projection consumers.
            let source = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        "[FileCache] bg task: read failed for `{}`: {}",
                        path.display(),
                        e
                    );
                    return Err(FileLoadError::Io(e));
                }
            };
            let read_done = t0.elapsed();

            // Delegate the parse to rumoca-session. Compared to
            // calling `rumoca_phase_parse::parse_to_ast` directly
            // (what `AstCache::from_source` does), this path:
            //   - Hashes the source and checks rumoca's in-memory
            //     LRU (256 entries) + on-disk bincode cache; a hit
            //     returns a ready `StoredDefinition` with ~zero
            //     parse cost.
            //   - Populates the same cache, so when rumoca-session
            //     runs a compile pipeline later over the same file,
            //     its query layer hits this cached entry instead of
            //     re-parsing.
            //   - Emits rumoca's per-phase instrumentation counters
            //     (parse calls, nanos, hits/misses) which we can
            //     surface via `rumoca_session::runtime_api` for
            //     diagnostics.
            //
            // `parse_files_parallel` is the public API that goes
            // through the artifact cache. We call it with a single
            // path; the rayon parallelism is overhead-free for a
            // 1-element input.
            let parse_result =
                rumoca_session::parsing::parse_files_parallel(&[path.clone()]);
            let ast = match parse_result {
                Ok(mut pairs) if !pairs.is_empty() => {
                    let (_, stored) = pairs.remove(0);
                    Arc::new(AstCache {
                        generation: 0,
                        result: Ok(Arc::new(stored)),
                    })
                }
                Ok(_) => Arc::new(AstCache {
                    generation: 0,
                    result: Err("rumoca returned no parse result".to_string()),
                }),
                Err(e) => Arc::new(AstCache {
                    generation: 0,
                    result: Err(e.to_string()),
                }),
            };
            let parse_done = t0.elapsed();
            info!(
                "[FileCache] bg task done `{}`: read {:.1}ms parse {:.1}ms ({} bytes) {}",
                path.display(),
                read_done.as_secs_f64() * 1000.0,
                (parse_done - read_done).as_secs_f64() * 1000.0,
                source.len(),
                if ast.result.is_ok() { "ok" } else { "ERR" },
            );
            Ok(FileEntry {
                path,
                source: source.into(),
                ast,
            })
        })
    }
}

#[derive(Resource)]
pub struct FileCache(pub ResourceCache<ModelicaFileLoader>);

impl Default for FileCache {
    fn default() -> Self {
        Self(ResourceCache::new(ModelicaFileLoader))
    }
}

impl FileCache {
    pub fn peek(&self, path: &std::path::Path) -> Option<Arc<FileEntry>> {
        self.0.peek(&path.to_path_buf())
    }
    pub fn is_loading(&self, path: &std::path::Path) -> bool {
        self.0.is_loading(&path.to_path_buf())
    }
    pub fn request(&mut self, path: PathBuf) -> bool {
        self.0.request(path)
    }
    pub fn evict(&mut self, path: &std::path::Path) -> bool {
        self.0.evict(&path.to_path_buf())
    }
}

pub fn drive_file_cache(cache: Option<ResMut<FileCache>>) {
    let Some(mut cache) = cache else { return };
    for key in cache.0.drive() {
        match cache.0.state(&key) {
            Some(lunco_cache::ResourceState::Ready(entry)) => {
                info!(
                    "[FileCache] loaded `{}` ({} bytes)",
                    entry.path.display(),
                    entry.source.len()
                );
            }
            Some(lunco_cache::ResourceState::Failed(msg)) => {
                warn!("[FileCache] load failed for `{}`: {}", key.display(), msg);
            }
            None => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Class tier: qualified name → class entry, composed on FileCache.
// ═══════════════════════════════════════════════════════════════════

/// One cached class. Points at the `source` + `ast` of the file it
/// lives in (shared `Arc`s). Downstream code walks `ast` to find
/// the specific sub-class by qualified name.
#[derive(Debug, Clone)]
pub struct CachedClass {
    pub qualified: String,
    pub source: Arc<str>,
    pub ast: Arc<AstCache>,
    pub file_path: PathBuf,
}

/// Terminal state for a class request.
enum ClassStatus {
    Ready(Arc<CachedClass>),
    /// We've resolved the file path and asked `FileCache` — waiting
    /// for it to land.
    PendingFile(PathBuf),
    Failed(Arc<str>),
}

/// Qualified-name → class cache. Unlike `FileCache`, this doesn't
/// own a `ResourceCache<Loader>` — there's no async work specific
/// to the class tier. It's pure bookkeeping: path resolution,
/// pending bindings, and promotion when `FileCache` resolves.
#[derive(Resource, Default)]
pub struct ClassCache {
    entries: std::collections::HashMap<String, ClassStatus>,
}

impl ClassCache {
    pub fn peek(&self, qualified: &str) -> Option<Arc<CachedClass>> {
        match self.entries.get(qualified) {
            Some(ClassStatus::Ready(c)) => Some(Arc::clone(c)),
            _ => None,
        }
    }

    pub fn is_loading(&self, qualified: &str) -> bool {
        matches!(self.entries.get(qualified), Some(ClassStatus::PendingFile(_)))
    }

    /// Tri-state state accessor for UI diagnostics: `Some(Ready(...))`,
    /// `Some(Pending)`, `Some(Failed(msg))`, or `None` for never-requested.
    pub fn state_display(&self, qualified: &str) -> Option<&'static str> {
        self.entries.get(qualified).map(|s| match s {
            ClassStatus::Ready(_) => "ready",
            ClassStatus::PendingFile(_) => "loading",
            ClassStatus::Failed(_) => "failed",
        })
    }

    pub fn failure_message(&self, qualified: &str) -> Option<Arc<str>> {
        match self.entries.get(qualified) {
            Some(ClassStatus::Failed(s)) => Some(Arc::clone(s)),
            _ => None,
        }
    }

    pub fn evict(&mut self, qualified: &str) -> bool {
        self.entries.remove(qualified).is_some()
    }

    /// Kick off a load (via `FileCache`) for this class if it
    /// isn't already cached or in-flight. Returns whether a NEW
    /// resolution happened (cache-miss path). Cheap on repeats —
    /// just a HashMap lookup.
    ///
    /// `file_cache` is passed in because two separate resources
    /// can't be fetched in one call from `World`; the caller (a
    /// Bevy system or a helper like [`request_class`]) holds both
    /// borrows.
    pub fn request(
        &mut self,
        qualified: impl Into<String>,
        file_cache: &mut FileCache,
    ) -> bool {
        let qualified = qualified.into();
        if self.entries.contains_key(&qualified) {
            // Already Ready / PendingFile / Failed — nothing to do.
            return false;
        }
        let Some(path) = resolve_msl_class_path(&qualified) else {
            self.entries.insert(
                qualified.clone(),
                ClassStatus::Failed(format!("no file for `{qualified}`").into()),
            );
            return true;
        };
        // If the file is ALREADY loaded, promote synchronously this
        // frame — no need to wait for the next drive tick.
        if let Some(entry) = file_cache.peek(&path) {
            self.entries.insert(
                qualified.clone(),
                ClassStatus::Ready(Arc::new(CachedClass {
                    qualified: qualified.clone(),
                    source: Arc::clone(&entry.source),
                    ast: Arc::clone(&entry.ast),
                    file_path: entry.path.clone(),
                })),
            );
            return true;
        }
        // Otherwise ask `FileCache` and remember our binding.
        file_cache.request(path.clone());
        self.entries.insert(qualified, ClassStatus::PendingFile(path));
        true
    }
}

/// Bevy system: for each class entry waiting on its file, check
/// whether `FileCache` has it now. If yes, promote to Ready. If
/// the file load failed, propagate the failure.
pub fn drive_class_cache(
    mut classes: Option<ResMut<ClassCache>>,
    files: Option<Res<FileCache>>,
) {
    let (Some(classes), Some(files)) = (classes.as_mut(), files.as_ref()) else {
        return;
    };
    // Snapshot pending keys → paths so we can mutate `entries` below.
    let pending: Vec<(String, PathBuf)> = classes
        .entries
        .iter()
        .filter_map(|(q, s)| match s {
            ClassStatus::PendingFile(p) => Some((q.clone(), p.clone())),
            _ => None,
        })
        .collect();
    for (qualified, path) in pending {
        if let Some(entry) = files.peek(&path) {
            classes.entries.insert(
                qualified.clone(),
                ClassStatus::Ready(Arc::new(CachedClass {
                    qualified: qualified.clone(),
                    source: Arc::clone(&entry.source),
                    ast: Arc::clone(&entry.ast),
                    file_path: entry.path.clone(),
                })),
            );
            info!("[ClassCache] promoted `{}` (file hit)", qualified);
            continue;
        }
        // File failed? Propagate.
        if let Some(lunco_cache::ResourceState::Failed(msg)) =
            files.0.state(&path)
        {
            classes
                .entries
                .insert(qualified.clone(), ClassStatus::Failed(Arc::clone(msg)));
            warn!(
                "[ClassCache] `{}` failed because file `{}` failed: {}",
                qualified,
                path.display(),
                msg
            );
        }
    }
}

/// Helper for non-system callers (Bevy commands, render functions)
/// to kick a class load without plumbing both `ResMut`s at every
/// call site. Takes `&mut World` so it can fetch both resources.
///
/// Returns whether a new load was started.
pub fn request_class(world: &mut World, qualified: impl AsRef<str>) -> bool {
    let qualified = qualified.as_ref().to_string();
    // Two-step borrow: get path + file state first (immutable/scoped),
    // then mutate class + file caches together. We can't hold two
    // mutable resource borrows simultaneously via `world.resource_mut`,
    // so funnel through `ResourceScope`.
    world.resource_scope::<ClassCache, bool>(|world, mut classes| {
        let Some(mut files) = world.get_resource_mut::<FileCache>() else {
            return false;
        };
        classes.request(qualified, &mut files)
    })
}

// ═══════════════════════════════════════════════════════════════════
// Qualified name → file path resolution (static index)
// ═══════════════════════════════════════════════════════════════════

pub fn msl_class_to_file_index(
) -> &'static std::collections::HashMap<String, std::path::PathBuf> {
    use std::sync::OnceLock;
    static INDEX: OnceLock<std::collections::HashMap<String, std::path::PathBuf>> =
        OnceLock::new();
    INDEX.get_or_init(build_msl_class_to_file_index)
}

fn build_msl_class_to_file_index(
) -> std::collections::HashMap<String, std::path::PathBuf> {
    let start = std::time::Instant::now();
    let lib = crate::visual_diagram::msl_component_library();
    let mut map = std::collections::HashMap::with_capacity(lib.len());
    for comp in lib {
        if let Some(path) = locate_msl_file(&comp.msl_path) {
            map.insert(comp.msl_path.clone(), path);
        }
    }
    info!(
        "[ClassCache] MSL class index built: {} classes in {:?}",
        map.len(),
        start.elapsed()
    );
    map
}

pub fn locate_msl_file(qualified: &str) -> Option<std::path::PathBuf> {
    let msl_root = lunco_assets::msl_dir();
    let segments: Vec<&str> = qualified.split('.').collect();
    if segments.is_empty() {
        return None;
    }
    for i in (1..=segments.len()).rev() {
        let mut dir = msl_root.clone();
        for seg in &segments[..i] {
            dir.push(seg);
        }
        // At any depth: prefer a directory holding `package.mo`
        // over a flat `.mo`, so `Modelica.Blocks` resolves to
        // `Modelica/Blocks/package.mo` (the package) rather than
        // falling up to `Modelica/package.mo` (the grandparent).
        let pkg = dir.join("package.mo");
        if pkg.exists() {
            return Some(pkg);
        }
        let flat = dir.with_extension("mo");
        if flat.exists() {
            return Some(flat);
        }
    }
    None
}

pub fn resolve_msl_class_path(qualified: &str) -> Option<std::path::PathBuf> {
    msl_class_to_file_index().get(qualified).cloned()
}

// ═══════════════════════════════════════════════════════════════════
// Filesystem-derived MSL resolver
// ═══════════════════════════════════════════════════════════════════
//
// The hardcoded `("Rotational", "Modelica.Mechanics.Rotational")`
// style alias table gets stale the moment MSL reorganizes. Walk the
// filesystem once and build the head-index from what's actually there:
//
//   by_head["Rotational"] = ["Modelica.Mechanics.Rotational"]
//   by_head["Blocks"]     = ["Modelica.Blocks", "Modelica.ComplexBlocks"]
//
// When a short-form ref `Rotational.Interfaces.Flange_a` comes in
// and `locate_msl_file` can't find `Rotational/` at MSL root, we
// look `Rotational` up in `by_head`, prefix-rewrite, retry.
//
// Each entry here is a *package container* — a directory with
// `package.mo` or a flat `.mo` file immediately under some parent.
// Classes nested *inside* `.mo` files (e.g. `Modelica.Units.SI`
// lives inside `Modelica/Units.mo`) don't appear as filesystem
// entries; those still need either explicit user imports or a
// loaded-file import-scope scan.

#[derive(Debug, Default)]
pub struct MslFsIndex {
    /// Last-segment → full qualified names. `"Rotational"` may map
    /// to multiple fully-qualified packages; resolver tries each.
    pub by_head: std::collections::HashMap<String, Vec<String>>,
    /// Full qualified name → on-disk file.
    pub qualified_to_path: std::collections::HashMap<String, std::path::PathBuf>,
}

pub fn msl_fs_index() -> &'static MslFsIndex {
    use std::sync::OnceLock;
    static INDEX: OnceLock<MslFsIndex> = OnceLock::new();
    INDEX.get_or_init(build_msl_fs_index)
}

fn build_msl_fs_index() -> MslFsIndex {
    let start = std::time::Instant::now();
    let Some(root) = lunco_assets::msl_source_root_path() else {
        return MslFsIndex::default();
    };
    let mut index = MslFsIndex::default();
    walk_msl_fs(&root, &root, &[], &mut index);
    info!(
        "[ClassCache] MSL fs index built: {} qualified paths, {} distinct heads in {:?}",
        index.qualified_to_path.len(),
        index.by_head.len(),
        start.elapsed()
    );
    index
}

fn walk_msl_fs(
    root: &std::path::Path,
    dir: &std::path::Path,
    prefix: &[String],
    index: &mut MslFsIndex,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Record the package itself if dir has a package.mo (and it's
    // not the MSL root).
    if !prefix.is_empty() && dir.join("package.mo").exists() {
        let qualified = prefix.join(".");
        index
            .qualified_to_path
            .insert(qualified.clone(), dir.join("package.mo"));
        if let Some(head) = prefix.last() {
            index
                .by_head
                .entry(head.clone())
                .or_default()
                .push(qualified);
        }
    }
    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = match file_name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if file_type.is_dir() {
            // Skip MSL's Resources / test dirs.
            if matches!(name_str, "Resources" | "Images" | "test") {
                continue;
            }
            let mut next = prefix.to_vec();
            next.push(name_str.to_string());
            walk_msl_fs(root, &path, &next, index);
        } else if file_type.is_file()
            && name_str.ends_with(".mo")
            && name_str != "package.mo"
        {
            let stem = name_str.trim_end_matches(".mo").to_string();
            let mut full = prefix.to_vec();
            full.push(stem.clone());
            let qualified = full.join(".");
            index.qualified_to_path.insert(qualified.clone(), path);
            index
                .by_head
                .entry(stem)
                .or_default()
                .push(qualified);
        }
    }
    let _ = root;
}

/// Try to resolve a short-form dotted ref by prefix-rewriting its
/// head against the filesystem head index. Returns the rewritten
/// full qualified name (e.g. `Rotational.Interfaces.Flange_a` →
/// `Modelica.Mechanics.Rotational.Interfaces.Flange_a`) if exactly
/// one head match exists that also resolves to an actual file when
/// combined with the remaining segments. `None` if ambiguous, no
/// head match, or already starts with a qualified path that resolves.
pub fn resolve_msl_head_prefix(qualified: &str) -> Option<String> {
    // Direct hit first — head is already at MSL root.
    if locate_msl_file(qualified).is_some() {
        return Some(qualified.to_string());
    }
    let (head, rest) = qualified.split_once('.').unwrap_or((qualified, ""));
    let index = msl_fs_index();
    let candidates = index.by_head.get(head)?;
    // Refuse to guess when the head is ambiguous. `Logical` appears
    // in `Modelica.Blocks.Logical`, `Modelica.Clocked...Logical`,
    // etc. — picking the first match is wrong for 90% of callers.
    // Only rewrite when the filesystem gives us a unique answer.
    // For the right ambiguous case, rumoca's §5 scope walk should
    // find the actual target via imports in enclosing packages
    // (which the caller has ensured are loaded).
    if candidates.len() > 1 {
        return None;
    }
    for full_head in candidates {
        let candidate = if rest.is_empty() {
            full_head.clone()
        } else {
            format!("{full_head}.{rest}")
        };
        if locate_msl_file(&candidate).is_some() {
            return Some(candidate);
        }
    }
    None
}

/// Heuristic: "is this class a pure icon / visual symbol, not a
/// functional component?"
///
/// **Current rule**: the qualified name contains a `.Icons.` segment.
/// That matches the conventional Modelica Standard Library subtree
/// (`Modelica.Icons.*`, `Modelica.Electrical.Analog.Icons.*`,
/// `Modelica.Mechanics.Rotational.Icons.*`, …) where every class is
/// a partial shell with `annotation(Icon(...))` and no connectors.
///
/// **Why not just count connectors.** A zero-connector class could
/// legitimately be a self-contained simulation model (a sandbox
/// with only equations, no external I/O). Path-based matches MSL's
/// own naming intent, so the false-positive rate is tiny.
///
/// **TODO**: upgrade to AST-based detection. The accurate signal is
/// "has `Icon` annotation + no `Diagram` annotation + no
/// placed components + class is `partial`". That catches custom
/// user icon libraries outside the MSL naming convention. Needs an
/// AST walk at projection time, not just a string check, so deferred
/// until we're sure the cheaper rule doesn't suffice.
///
/// Reference: [Modelica.Icons](https://doc.modelica.org/Modelica%204.0.0/Resources/helpOM/Modelica.Icons.html)
/// is how the MSL itself organises its graphical-only classes.
pub fn is_icon_only_class(qualified: &str) -> bool {
    qualified.contains(".Icons.")
}

// ═══════════════════════════════════════════════════════════════════
// Sync MSL class loader — for callers that need a class *right now*
// ═══════════════════════════════════════════════════════════════════

/// Synchronously resolve an MSL class by qualified name and return
/// its [`ClassDef`]. Lazily reads + parses the containing `.mo` file
/// and memoises the result by qualified name.
///
/// # Why sync (vs. the main `ClassCache.request` async flow)
///
/// The async [`ClassCache`] is the right tier for foreground UI
/// loads — user clicks Drill-in, a background task parses, the
/// canvas re-projects next frame. But *icon extraction* runs inside
/// the projector pipeline where we need the parent-class AST NOW to
/// resolve `extends`-graphics inheritance. Deferring by a frame
/// means rendering every MSL sensor / partial without its inherited
/// body on first open, then popping in — bad UX.
///
/// This helper does the blocking I/O + parse once per qualified name
/// and caches the `Arc<ClassDef>` for instant subsequent calls. MSL
/// files are small (most ≤ a few KB; the package-aggregate files are
/// at worst a few hundred KB) so a one-shot read is acceptable. Any
/// resolution failure is memoised as `None` so repeated hits don't
/// re-hammer the filesystem.
///
/// Used by the icon-inheritance resolver so `SpeedSensor extends
/// Modelica.Mechanics.Rotational.Icons.RelativeSensor` pulls in the
/// parent's rectangle/text primitives the first time it renders.
pub fn peek_or_load_msl_class(
    qualified: &str,
) -> Option<Arc<rumoca_session::parsing::ast::ClassDef>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    // Per-qualified-name cache. `None` is remembered too — missing
    // classes don't retry on every icon render.
    static CACHE: OnceLock<
        Mutex<HashMap<String, Option<Arc<rumoca_session::parsing::ast::ClassDef>>>>,
    > = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(map) = cache.lock() {
        if let Some(slot) = map.get(qualified) {
            return slot.clone();
        }
    }

    let resolved = load_msl_class_uncached(qualified);
    if let Ok(mut map) = cache.lock() {
        map.insert(qualified.to_string(), resolved.clone());
    }
    resolved
}

/// File-level parse cache for `peek_or_load_msl_class`. Without this,
/// drilling into a single class triggers a chain of MSL file loads
/// (one per `extends` target), and a package-aggregated source file
/// like `Continuous.mo` (184 KB, 20+ classes) gets re-parsed once per
/// qualified-name request — pushing icon-extends inheritance from
/// ~ms to seconds per drill-in. Keying by absolute path ensures
/// every class inside the same file shares one parse.
fn parse_msl_file_cached(
    path: &std::path::Path,
) -> Option<Arc<rumoca_session::parsing::ast::StoredDefinition>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<
        Mutex<HashMap<std::path::PathBuf, Option<Arc<rumoca_session::parsing::ast::StoredDefinition>>>>,
    > = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(slot) = map.get(path) {
            return slot.clone();
        }
    }
    let parsed = (|| {
        let source = std::fs::read_to_string(path).ok()?;
        let syntax = rumoca_phase_parse::parse_to_syntax(&source, &path.to_string_lossy());
        Some(Arc::new(syntax.best_effort().clone()))
    })();
    if let Ok(mut map) = cache.lock() {
        map.insert(path.to_path_buf(), parsed.clone());
    }
    parsed
}

fn load_msl_class_uncached(
    qualified: &str,
) -> Option<Arc<rumoca_session::parsing::ast::ClassDef>> {
    // Palette index first; fall through to filesystem walk so classes
    // nested inside `.mo` files (e.g. a partial base not surfaced in
    // the palette) still resolve.
    let path = resolve_msl_class_path(qualified)
        .or_else(|| locate_msl_file(qualified))?;
    let stored = parse_msl_file_cached(&path)?;
    find_class_in_stored_def(&stored, qualified)
        .map(|c| Arc::new(c.clone()))
}

/// Walk a parsed `StoredDefinition` down the qualified dotted path
/// to locate a specific class. Handles the common MSL-file shape:
/// a top-level `package.mo` containing nested classes N layers deep,
/// and flat `.mo` files where the leaf class sits at the top level.
fn find_class_in_stored_def<'a>(
    ast: &'a rumoca_session::parsing::ast::StoredDefinition,
    qualified: &str,
) -> Option<&'a rumoca_session::parsing::ast::ClassDef> {
    let parts: Vec<&str> = qualified.split('.').collect();

    // Strategy: walk every top-level class and treat its name as a
    // prefix; if the qualified path starts with it, recurse into
    // its nested classes. When the last segment matches the current
    // level, return. Tries flat (leaf-at-top-level) too.
    for (top_name, top_class) in ast.classes.iter() {
        // Flat: the top-level class name IS the qualified tail.
        let tail = parts.last().copied().unwrap_or("");
        if top_name.as_str() == qualified || top_name.as_str() == tail {
            return Some(top_class);
        }
        // Prefix: top-level name matches a middle segment of the
        // qualified path, walk nested classes for the rest.
        if let Some(pos) = parts.iter().position(|p| *p == top_name.as_str()) {
            let remaining = &parts[pos + 1..];
            if let Some(c) = walk_nested_classes(top_class, remaining) {
                return Some(c);
            }
        }
    }
    None
}

fn walk_nested_classes<'a>(
    class: &'a rumoca_session::parsing::ast::ClassDef,
    path: &[&str],
) -> Option<&'a rumoca_session::parsing::ast::ClassDef> {
    if path.is_empty() {
        return Some(class);
    }
    let (head, rest) = (path[0], &path[1..]);
    let child = class.classes.get(head)?;
    walk_nested_classes(child, rest)
}

// ═══════════════════════════════════════════════════════════════════
// Plugin
// ═══════════════════════════════════════════════════════════════════

pub struct ClassCachePlugin;

impl Plugin for ClassCachePlugin {
    fn build(&self, app: &mut App) {
        // Colocate rumoca's parsed-artifact cache under our
        // workspace `.cache/` (next to the MSL files we already
        // manage there) instead of letting rumoca default to
        // `~/.cache/rumoca/`. Keeps all our tooling's cache in
        // one discoverable, clearable place. Honors any explicit
        // `RUMOCA_CACHE_DIR` the user already set.
        //
        // Runs once in plugin build(), before any bg task spawns
        // a rumoca parse, so the first call sees the redirected
        // path.
        if std::env::var_os("RUMOCA_CACHE_DIR").is_none() {
            let target = lunco_assets::cache_dir().join("rumoca");
            std::env::set_var("RUMOCA_CACHE_DIR", &target);
            info!(
                "[ClassCache] redirected rumoca parse cache to `{}`",
                target.display()
            );
        }

        app.init_resource::<FileCache>()
            .init_resource::<ClassCache>()
            // FileCache drives FIRST so newly-finished files are
            // visible to ClassCache's promoter on the same frame.
            .add_systems(Update, (drive_file_cache, drive_class_cache).chain());
    }
}

//! MSL filesystem layout — qualified-name → on-disk (or in-memory)
//! `.mo` file resolution.
//!
//! Pure path / index logic, no parsing. The resolver tier consumed
//! by `class_cache` (engine-backed loader) and the drill-in / open
//! flows that need a file path before reading source.
//!
//! One lazy index:
//!
//! - [`class_to_file_index`] — `qualified → PathBuf` for every
//!   class the visual palette knows about. Built from
//!   [`crate::visual_diagram::msl_class_library`].
//!
//! [`locate_library_file`] is the single source-of-truth resolver: it
//! walks the in-memory bundle (web) or filesystem roots (native,
//! including extra libraries like ThermofluidStream) to map any
//! qualified (fully-qualified only) name to its containing file.
//!
//! Short-form / scoped / import-based name lookup (MLS §5) is NOT done
//! here — that is rumoca's job, via the engine `Session`. This module
//! is purely §13 storage resolution (qualified name → `.mo` file) for
//! the *pre-load* palette/drill-in path, where no resolved `Session`
//! exists yet.

use bevy::log::info;

pub fn class_to_file_index(
) -> &'static std::collections::HashMap<String, std::path::PathBuf> {
    use std::sync::OnceLock;
    static INDEX: OnceLock<std::collections::HashMap<String, std::path::PathBuf>> =
        OnceLock::new();
    static EMPTY: OnceLock<std::collections::HashMap<String, std::path::PathBuf>> =
        OnceLock::new();

    if let Some(idx) = INDEX.get() {
        return idx;
    }
    // On web, the palette library is empty until the MSL bundle has
    // been fetched + decompressed (see `msl_class_library` for the
    // same trick). If we'd `OnceLock::set` an empty map here, the
    // index would stay empty for the lifetime of the page even after
    // MSL lands. So: return an empty placeholder *without* memoising,
    // so the next caller retries the build.
    let lib = crate::visual_diagram::msl_class_library();
    if lib.is_empty() {
        return EMPTY.get_or_init(std::collections::HashMap::new);
    }
    INDEX.get_or_init(build_class_to_file_index)
}

fn build_class_to_file_index(
) -> std::collections::HashMap<String, std::path::PathBuf> {
    let start = web_time::Instant::now();
    let lib = crate::visual_diagram::msl_class_library();
    let mut map = std::collections::HashMap::with_capacity(lib.len());
    for comp in lib {
        if let Some(path) = locate_library_file(&comp.name) {
            map.insert(comp.name.clone(), path);
        }
    }
    info!(
        "[MslFs] MSL class index built: {} classes in {:?}",
        map.len(),
        start.elapsed()
    );
    map
}

pub fn locate_library_file(qualified: &str) -> Option<std::path::PathBuf> {
    let segments: Vec<&str> = qualified.split('.').collect();
    if segments.is_empty() {
        return None;
    }

    // Search every installed library root in priority order (MSL
    // first, then any extra libraries). The walk (`resolve_in_root`:
    // longest-prefix-first, package.mo over flat `.mo`) is pure §13
    // path logic; each root's backend membership (`contains`) and its
    // join base (`base`) are owned by `lunco_assets` — no filesystem
    // access here. So both targets run identical logic and the §13
    // semantics can't drift.
    for source in lunco_assets::msl::global_msl_sources() {
        if let Some(hit) = resolve_in_root(&segments, source.base(), |c| source.contains(c)) {
            return Some(hit);
        }
    }
    None
}

/// The §13 storage resolution walk over a single root, backend-agnostic.
///
/// Tries the qualified name's prefixes longest-first; at each depth
/// prefers a `package.mo` directory over a flat `.mo` (so
/// `Modelica.Blocks` → `Modelica/Blocks/package.mo`, not the
/// grandparent `Modelica/package.mo`). `base` is the root the
/// candidate is joined onto (a disk dir, or `""` for the in-memory
/// bundle's relative keys); `contains` tests membership against that
/// root's backend. Returns the first hit's (base-joined) path.
fn resolve_in_root<F>(
    segments: &[&str],
    base: &std::path::Path,
    contains: F,
) -> Option<std::path::PathBuf>
where
    F: Fn(&std::path::Path) -> bool,
{
    for i in (1..=segments.len()).rev() {
        let mut dir = base.to_path_buf();
        for seg in &segments[..i] {
            dir.push(seg);
        }
        let pkg = dir.join("package.mo");
        if contains(&pkg) {
            return Some(pkg);
        }
        let flat = dir.with_extension("mo");
        if contains(&flat) {
            return Some(flat);
        }
    }
    None
}

pub fn resolve_class_path_indexed(qualified: &str) -> Option<std::path::PathBuf> {
    class_to_file_index().get(qualified).cloned()
}

//! Host-side USD reference-closure walk for scenario manifests.
//!
//! A sandbox scene pulls its rovers/components from **outside its own folder**:
//!
//! ```text
//! assets/scenes/sandbox/sandbox_scene.usda
//!   prepend references = @../../vessels/rovers/ackermann_rover.usda@   // → assets/vessels/…
//! ```
//!
//! The manifest builder's folder walk ([`collect_scenario_input`]) only sees
//! files under the Twin root, so those out-of-tree layers were never shipped and
//! a client's `scenario://` load 404'd on the sublayer (the resolver also rejects
//! the `..` needed to reach them). This module parses the scene's transitive
//! `subLayers` / `references` / `payload` graph and returns every file it reaches,
//! so the builder can add the external ones and re-root all paths at their common
//! ancestor (making every synced path `..`-free).
//!
//! The path logic mirrors `lunco_usd_bevy`'s loader-side `discover_arcs` /
//! `normalize` (kept byte-identical so the host closure agrees with the client
//! resolver's `canonicalize` — "R-canon"). It uses only the `openusd` text
//! parser, so the crate stays free of Bevy / `lunco-usd`.
//!
//! [`collect_scenario_input`]: crate::server

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use openusd::sdf::{self, Path as SdfPath, Value};
use openusd::usda;

/// Lexically resolve `..` / `.` without touching the filesystem. Mirrors
/// `lunco_usd_bevy::resolver::normalize`; a leading `..` with nothing to pop is
/// preserved (a relative anchor would otherwise resolve to the wrong place).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True if `path` is a USD layer we should parse-and-recurse into (vs. a leaf
/// asset like a `.glb` that is shipped but not followed).
pub(crate) fn is_usd_layer(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("usd" | "usda" | "usdc")
    )
}

/// Collect the `subLayers` + `references` + `payload` asset-path arcs authored in
/// a parsed layer. Mirrors `lunco_usd_bevy::compose::discover_arcs` but **keeps**
/// binary arcs (`.glb`/…) — the manifest ships referenced binaries as leaf files
/// even though they are not recursed into. Iterating ALL specs (not just the live
/// prim tree) catches references authored inside variant blocks.
fn discover_arcs(data: &sdf::Data) -> Vec<String> {
    let mut arcs = Vec::new();
    if let Some(root) = data.spec(&SdfPath::abs_root()) {
        if let Some(Value::StringVec(subs)) = root.get("subLayers") {
            arcs.extend(subs.iter().filter(|s| !s.is_empty()).cloned());
        }
    }
    for (_path, spec) in data.iter() {
        if let Some(Value::ReferenceListOp(op)) = spec.get("references") {
            arcs.extend(
                op.iter()
                    .filter(|r| !r.asset_path.is_empty())
                    .map(|r| r.asset_path.clone()),
            );
        }
        match spec.get("payload") {
            Some(Value::Payload(p)) if !p.asset_path.is_empty() => arcs.push(p.asset_path.clone()),
            Some(Value::PayloadListOp(op)) => arcs.extend(
                op.iter()
                    .filter(|p| !p.asset_path.is_empty())
                    .map(|p| p.asset_path.clone()),
            ),
            _ => {}
        }
    }
    arcs
}

/// BFS the transitive file closure of `roots` (absolute scene paths). Returns
/// every reachable file — USD layers plus referenced leaf assets — including the
/// roots themselves.
///
/// - Arcs carrying a `scheme://` (`lunco-lib://`, `twin://`, `scenario://`, …)
///   are skipped: they resolve through their own asset source, not the scenario
///   file tree.
/// - Leading-`/` (assets-root-relative) arcs are skipped — the assets root is
///   unknown here, and the scenes in scope use only layer-relative refs.
/// - Unreadable / unparseable layers drop out of recursion (best-effort). A file
///   that is genuinely required but missing surfaces later as a manifest read
///   error — [`build_manifest_from_input`] is fail-closed on unreadable assets.
///
/// [`build_manifest_from_input`]: crate::server
pub(crate) fn reference_closure(roots: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = roots.iter().map(|p| normalize(p)).collect();
    while let Some(path) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        if !is_usd_layer(&path) {
            continue; // binary leaf — shipped, not recursed
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = usda::parse(&text) else {
            continue;
        };
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for arc in discover_arcs(&data) {
            if arc.contains("://") || arc.starts_with('/') {
                continue;
            }
            queue.push(normalize(&base.join(&arc)));
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_and_preserves_leading_parent() {
        assert_eq!(normalize(Path::new("a/b/../c")), PathBuf::from("a/c"));
        assert_eq!(normalize(Path::new("a/./b")), PathBuf::from("a/b"));
        // A leading `..` with nothing to pop is preserved.
        assert_eq!(
            normalize(Path::new("x/../../y")),
            PathBuf::from("../y")
        );
    }

    #[test]
    fn closure_follows_out_of_tree_reference() {
        // scene at <tmp>/scenes/sandbox/scene.usda references a rover two levels
        // up at <tmp>/vessels/rover.usda, which in turn references a co-located
        // wheel — the multi-level, out-of-tree shape of the real sandbox scene.
        let tmp = std::env::temp_dir().join(format!(
            "lunco_closure_test_{}",
            std::process::id()
        ));
        let scene_dir = tmp.join("scenes").join("sandbox");
        let vessels_dir = tmp.join("vessels");
        std::fs::create_dir_all(&scene_dir).unwrap();
        std::fs::create_dir_all(&vessels_dir).unwrap();

        let scene = scene_dir.join("scene.usda");
        let rover = vessels_dir.join("rover.usda");
        let wheel = vessels_dir.join("wheel.usda");
        std::fs::write(
            &scene,
            "#usda 1.0\ndef Xform \"R\" (prepend references = @../../vessels/rover.usda@) {}\n",
        )
        .unwrap();
        std::fs::write(
            &rover,
            "#usda 1.0\ndef Xform \"W\" (prepend references = @wheel.usda@) {}\n",
        )
        .unwrap();
        std::fs::write(&wheel, "#usda 1.0\n").unwrap();

        let closure = reference_closure(&[scene.clone()]);
        assert!(closure.contains(&normalize(&scene)), "root scene included");
        assert!(closure.contains(&normalize(&rover)), "out-of-tree rover followed");
        assert!(closure.contains(&normalize(&wheel)), "transitive wheel followed");

        std::fs::remove_dir_all(&tmp).ok();
    }
}

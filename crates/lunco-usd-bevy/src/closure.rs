//! The canonical USD reference-closure walk — **one** implementation, shared by
//! every consumer that needs to know what files a layer transitively depends on.
//!
//! A scene pulls its rovers/components from outside its own folder:
//!
//! ```text
//! assets/scenes/sandbox/sandbox_scene.usda
//!   prepend references = @../../vessels/rovers/ackermann_rover.usda@   // → assets/vessels/…
//! ```
//!
//! so "the files this scene needs" is a graph walk, not a folder listing.
//!
//! Three consumers wanted that walk and it existed twice, because there was no
//! shared home: the composition pre-fetch ([`crate::compose`]) had one, and the
//! scenario-manifest builder in `lunco-networking` had a near-identical copy
//! whose own comment admitted it "mirrors" the first — which is also why that
//! crate talked to `openusd` directly. They differed on exactly **one** axis, so
//! that axis is now a parameter ([`ArcFilter`]) rather than a fork:
//!
//! - **Composition pre-fetch** wants layers only — a `.glb` is not a layer to
//!   fetch, the resolver stubs it.
//! - **Manifests and staleness** want everything — a client must receive the
//!   `.glb`, and swapping a DEM must invalidate the scene that points at it.
//!
//! The BFS *drivers* legitimately differ (async-`AssetServer` vs synchronous
//! filesystem) and stay separate; only the per-layer arc extraction is shared.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openusd::sdf::{self, Path as SdfPath, Value};
use openusd::usda;

use lunco_assets::asset_path::normalize;

use crate::resolver::is_binary_asset;

/// Which arcs [`discover_arcs`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcFilter {
    /// USD layers only — binary assets (glTF/OBJ/STL) are dropped. For callers
    /// that will *parse* what they get back.
    LayersOnly,
    /// Every arc, binary leaves included. For callers that must *ship* or
    /// *watch* the file rather than parse it.
    All,
}

/// Collect the `subLayers` + `references` + `payload` asset-path arcs authored
/// in a parsed layer.
///
/// Iterating ALL specs (not just the live prim tree) is deliberate: it catches
/// references authored inside variant blocks, which live at decorated paths.
///
/// `subLayers` are layers by definition and are never filtered; [`ArcFilter`]
/// applies to references and payloads, the arcs that can point at a binary.
pub fn discover_arcs(data: &sdf::Data, filter: ArcFilter) -> Vec<String> {
    let mut out = Vec::new();

    if let Some(root) = data.spec(&SdfPath::abs_root()) {
        if let Some(Value::StringVec(subs)) = root.get("subLayers") {
            out.extend(subs.iter().filter(|s| !s.is_empty()).cloned());
        }
    }

    for (_path, spec) in data.iter() {
        let mut arcs: Vec<String> = Vec::new();
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
        match filter {
            ArcFilter::All => out.extend(arcs),
            ArcFilter::LayersOnly => out.extend(arcs.into_iter().filter(|a| !is_binary_asset(a))),
        }
    }

    out
}

/// True if `path` is a USD layer to parse-and-recurse into, as opposed to a leaf
/// asset (a `.glb`) that is shipped or watched but never followed.
pub fn is_usd_layer(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("usd" | "usda" | "usdc")
    )
}

/// BFS the transitive file closure of `roots` (absolute paths) against the
/// **local filesystem**. Returns every reachable file — USD layers plus binary
/// leaves — including the roots themselves.
///
/// Used by the scenario-manifest builder (what a client must be sent) and by
/// document staleness (what, if changed on disk, makes an open document stale).
/// Both want the same answer, which is why they now ask the same function.
///
/// - Arcs carrying a `scheme://` (`lunco://`, `twin://`, …) are skipped:
///   they resolve through their own asset source, not this file tree.
/// - Leading-`/` (assets-root-relative) arcs are skipped — the assets root is
///   not known here.
/// - Unreadable / unparseable layers drop out of the recursion. This is
///   best-effort by design: a caller that must be fail-closed about a missing
///   file checks for it separately (the manifest builder does).
pub fn reference_closure(roots: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = roots.iter().map(|p| normalize(p)).collect();
    while let Some(path) = queue.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        if !is_usd_layer(&path) {
            continue; // binary leaf — carried, not recursed
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = usda::parse(&text) else {
            continue;
        };
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        // TODO(multiplayer): deferred — singleplayer focus for now, RBAC disabled
        // for ease of debugging. Relative arcs are followed out of the twin root
        // with no confinement. Revisit before multiplayer hardening
        // (REVIEW-2026-07-19.md finding #5).
        for arc in discover_arcs(&data, ArcFilter::All) {
            if lunco_assets::asset_path::is_anchored(&arc) {
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

    /// The walk must leave the scene's own folder — the bug that motivated it: a
    /// client's `twin://` load 404'd on a rover referenced two levels up.
    #[test]
    fn closure_follows_out_of_tree_reference() {
        let dir = tempfile::tempdir().unwrap();
        let scenes = dir.path().join("scenes/sandbox");
        let vessels = dir.path().join("vessels");
        std::fs::create_dir_all(&scenes).unwrap();
        std::fs::create_dir_all(&vessels).unwrap();

        let scene = scenes.join("scene.usda");
        let rover = vessels.join("rover.usda");
        let wheel = vessels.join("wheel.usda");
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
        assert!(closure.contains(&normalize(&scene)));
        assert!(
            closure.contains(&normalize(&rover)),
            "out-of-tree reference"
        );
        assert!(closure.contains(&normalize(&wheel)), "transitive reference");
    }

    /// The one axis the two former copies disagreed on. A `.glb` must survive
    /// `All` (a client needs the bytes; swapping it must invalidate the scene)
    /// and be dropped by `LayersOnly` (it is not a layer to parse).
    #[test]
    fn arc_filter_decides_whether_binaries_survive() {
        let data =
            usda::parse("#usda 1.0\ndef Xform \"M\" (prepend references = @rover.glb@) {}\n")
                .unwrap();

        assert_eq!(
            discover_arcs(&data, ArcFilter::All),
            vec!["rover.glb".to_string()]
        );
        assert!(discover_arcs(&data, ArcFilter::LayersOnly).is_empty());
    }
}

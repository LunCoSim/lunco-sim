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
//! **What a layer depends on is openusd's answer, not ours.**
//! `Data::composition_asset_dependencies` (`SdfLayer::GetCompositionAssetDependencies`)
//! and `Data::asset_dependencies` report the arcs and the asset-valued
//! attributes; this module only walks the graph they describe. Matching raw spec
//! fields here is what previously left every asset-attribute dependency — a
//! texture, a `.glb`, a program's `info:sourceAsset` — outside the closure.
//!
//! Two consumers, one walk: the scenario-manifest builder (what a client must be
//! sent) and document staleness (what, changed on disk, makes a document stale).
//! The composition PRE-FETCH does not use this walk — it cannot, since on web a
//! stage's layers must be fetched before a stage exists — and asks openusd for
//! one layer's arcs directly (see `compose`, and its `TODO(web)`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openusd::usda;

use lunco_assets::asset_path::normalize;

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
///
/// TODO(openusd): **DELETE this function.** The recursion below — open a layer,
/// ask it for its dependencies, anchor them, repeat — is
/// `UsdUtils.ComputeAllDependencies`, and belongs in the library rather than
/// here. Half of it already moved: the per-layer question is answered by the
/// fork's `Data::composition_asset_dependencies()` +
/// `Data::asset_dependencies()` (openusd `a198b40`), which is what deleted the
/// old hand-rolled `discover_arcs`/`ArcFilter`.
///
/// What is still missing upstream is the *walk*: a recursive,
/// resolver-anchored `compute_all_dependencies` that opens layers through
/// `LayerRegistry` + `ar::Resolver` instead of `usda::parse` on a file path.
/// Doing it there fixes three things this cannot:
///
/// - **Confinement** — a resolver context has a root, so relative arcs cannot
///   walk out of the twin (see the `TODO(multiplayer)` below, which is the same
///   defect seen from the other end).
/// - **Binary layers** — `usda::parse` cannot read `.usdc`/`.usdz`, so a
///   crate-side walk silently treats a binary layer as a leaf and misses
///   everything it references.
/// - **Schemes** — `lunco://`/`twin://` arcs are skipped here because this
///   function has no resolver; a resolver would resolve them.
pub fn reference_closure(roots: &[PathBuf]) -> BTreeSet<PathBuf> {
    // No resolver ⇒ every schemed arc drops out, which is the historical
    // behaviour the manifest builder and staleness both rely on.
    reference_closure_with(roots, |_| None)
}

/// [`reference_closure`], but with a **resolver for schemed arcs**.
///
/// `reference_closure` skips anything anchored (`lunco://`, `twin://`, a leading
/// `/`) because it has no way to turn such a reference into a file. That is a
/// silent hole for any caller that wants to ANSWER "what files does this scene
/// consist of": shipped assets are REQUIRED to be referenced as `@lunco://…@`, so
/// a scene's rovers, components, `.mo` models and textures are exactly the arcs
/// the plain walk throws away — it would report a scene as three files.
///
/// `resolve_anchored` is handed the raw reference (scheme included) and returns
/// the file it names, or `None` for one this caller cannot reach (a `twin://` with
/// no Twin mounted). Returning `None` degrades to the old behaviour for that arc
/// rather than failing the walk: a browser listing files should show what it can
/// find, not nothing.
pub fn reference_closure_with(
    roots: &[PathBuf],
    resolve_anchored: impl Fn(&str) -> Option<PathBuf>,
) -> BTreeSet<PathBuf> {
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
        // BOTH dependency kinds, both from openusd: composition arcs
        // (`SdfLayer::GetCompositionAssetDependencies`) and asset-valued
        // attributes. The second was missing entirely — a texture, a `.glb` a
        // schema binds by attribute, a program's `info:sourceAsset` — so any
        // such file outside the caller's own folder walk was never shipped.
        //
        // TODO(multiplayer): deferred — singleplayer focus for now, RBAC disabled
        // for ease of debugging. Relative arcs are followed out of the twin root
        // with no confinement, because anchoring still happens here rather than
        // through a resolver context that has a root. Moves to openusd's
        // recursive `compute_all_dependencies` (REVIEW-2026-07-19.md finding #5).
        let arcs = data
            .composition_asset_dependencies()
            .into_iter()
            .chain(data.asset_dependencies());
        for arc in arcs {
            if lunco_assets::asset_path::is_anchored(&arc) {
                if let Some(resolved) = resolve_anchored(&arc) {
                    queue.push(normalize(&resolved));
                }
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

    /// A binary leaf must be REACHED (a client needs the bytes, and swapping it
    /// must invalidate the scene) but never recursed into. The pre-fetch drops
    /// binaries itself, at its own call site, because only it will parse what it
    /// gets back — that axis is no longer a parameter of the walk.
    #[test]
    fn closure_carries_binary_leaves_without_parsing_them() {
        let dir = tempfile::tempdir().unwrap();
        let scene = dir.path().join("scene.usda");
        let mesh = dir.path().join("rover.glb");
        std::fs::write(
            &scene,
            "#usda 1.0\ndef Xform \"M\" (prepend references = @rover.glb@) {}\n",
        )
        .unwrap();
        std::fs::write(&mesh, b"\x67\x6c\x54\x46").unwrap();

        let closure = reference_closure(&[scene]);
        assert!(closure.contains(&normalize(&mesh)), "binary leaf ships");
    }

    /// The gap that motivated moving this to openusd: a dependency bound by an
    /// ASSET ATTRIBUTE — a program's `info:sourceAsset`, a texture — is not a
    /// composition arc, so a walk that only followed references/payloads left it
    /// out of the manifest and the client never received it.
    #[test]
    fn closure_follows_asset_valued_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();

        let scene = dir.path().join("scene.usda");
        let script = shared.join("policy.rhai");
        std::fs::write(
            &scene,
            "#usda 1.0\ndef Xform \"R\" { asset info:sourceAsset = @shared/policy.rhai@ }\n",
        )
        .unwrap();
        std::fs::write(&script, "fn on_start(me) {}\n").unwrap();

        let closure = reference_closure(&[scene]);
        assert!(
            closure.contains(&normalize(&script)),
            "an asset-valued attribute is a dependency: {closure:?}"
        );
    }

    /// Shipped assets are REQUIRED to be referenced as `@lunco://…@`, so without a
    /// resolver a scene made of library parts reports as a single file. With one,
    /// the schemed arc is followed like any other — including transitively, and
    /// including the `.mo` a program prim binds by `info:sourceAsset`.
    #[test]
    fn a_resolver_follows_schemed_arcs_the_plain_walk_drops() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(assets.join("vessels")).unwrap();
        std::fs::create_dir_all(assets.join("models")).unwrap();

        let scene = dir.path().join("scene.usda");
        let rover = assets.join("vessels/rover.usda");
        let model = assets.join("models/Drive.mo");
        std::fs::write(
            &scene,
            "#usda 1.0\ndef Xform \"R\" (prepend references = @lunco://vessels/rover.usda@) {}\n",
        )
        .unwrap();
        std::fs::write(
            &rover,
            "#usda 1.0\ndef Xform \"P\" { asset info:sourceAsset = @lunco://models/Drive.mo@ }\n",
        )
        .unwrap();
        std::fs::write(&model, "model Drive end Drive;\n").unwrap();

        let plain = reference_closure(&[scene.clone()]);
        assert!(
            !plain.contains(&normalize(&rover)),
            "without a resolver a schemed arc is unreachable — that is the hole"
        );

        let assets_root = assets.clone();
        let resolved = reference_closure_with(&[scene], |arc| {
            let rel = lunco_assets::parse_lunco_uri(arc)?;
            Some(assets_root.join(rel))
        });
        assert!(resolved.contains(&normalize(&rover)), "{resolved:?}");
        assert!(
            resolved.contains(&normalize(&model)),
            "transitively, and across a `.mo` bound by attribute: {resolved:?}"
        );
    }
}

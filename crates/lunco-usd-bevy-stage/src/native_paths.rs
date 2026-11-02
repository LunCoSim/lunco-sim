//! Native asset preparation over the shared composed USD read surface.

use crate::UsdReadObject;
use lunco_assets_core::asset_path::AssetReference;
use openusd::sdf::{AssetPath, Path as SdfPath, Value};
use std::collections::BTreeSet;

fn native_reference(reference: &str) -> bool {
    reference
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
}

fn collect_asset(
    path: &AssetPath,
    source_layer: Option<&str>,
    references: &mut impl FnMut(String, &str),
) {
    let Some(source_layer) = source_layer else {
        return;
    };
    match lunco_usd_compose::canonicalize_at(
        path.asset_path(),
        Some(&openusd::ar::ResolvedPath::new(source_layer)),
    ) {
        Ok(identifier) => {
            references(identifier, path.asset_path());
        }
        Err(error) => bevy::log::error!(%error, "invalid composed USD asset identity"),
    }
}

fn collect_value(
    value: Option<Value>,
    source_layer: Option<&str>,
    references: &mut impl FnMut(String, &str),
) {
    match value {
        Some(Value::AssetPath(path)) => collect_asset(&path, source_layer, references),
        Some(Value::AssetPathVec(paths)) => {
            for path in &paths {
                collect_asset(path, source_layer, references);
            }
        }
        _ => {}
    }
}

fn collect_property(
    reader: &dyn UsdReadObject,
    prim: &SdfPath,
    name: &str,
    references: &mut impl FnMut(String, &str),
) {
    if !matches!(
        reader.attr_type_name(prim, name).as_deref(),
        Some("asset" | "asset[]")
    ) {
        return;
    }
    collect_value(
        reader.attr_value(prim, name),
        reader.asset_source_layer(prim, name, None).as_deref(),
        references,
    );
    for time in reader.time_sample_times(prim, name) {
        collect_value(
            reader.attr_value_at(prim, name, time),
            reader.asset_source_layer(prim, name, Some(time)).as_deref(),
            references,
        );
    }
}

/// Read asset-valued attributes needing worker preparation — native file
/// identifiers and Twin search paths — and native binary arcs from composed
/// prims. Default scalar/array values carry their contributing-layer identifiers.
/// Time samples are included only when their USD value carries that context;
/// consumers reject unanchored asset values. No domain schemas are enumerated.
pub fn native_references_for_prims(
    reader: &dyn UsdReadObject,
    prims: impl IntoIterator<Item = SdfPath>,
) -> BTreeSet<AssetReference> {
    let mut references = BTreeSet::new();
    for prim in prims {
        for name in reader.attr_names(&prim) {
            collect_property(reader, &prim, &name, &mut |identifier, authored| {
                if let Some(reference) = AssetReference::for_asset_value(&identifier, authored) {
                    references.insert(reference);
                }
            });
        }
        if let Some(reference) = reader.binary_asset_uri(&prim)
            && native_reference(&reference)
        {
            references.insert(AssetReference::Native(reference));
        }
    }
    references
}

/// Canonical asset identities consumed by the active composed stage. Built
/// with the projection snapshot on its worker, including arrays/time samples.
/// Inactive prims and unselected variants do not contribute requirements.
pub fn composed_asset_identifiers_for_prims(
    reader: &dyn UsdReadObject,
    prims: impl IntoIterator<Item = SdfPath>,
) -> std::collections::BTreeMap<String, BTreeSet<String>> {
    let mut by_prim = std::collections::BTreeMap::new();
    for prim in prims {
        if reader.is_active(&prim) != Ok(true) {
            continue;
        }
        let mut identifiers = BTreeSet::new();
        for name in reader.attr_names(&prim) {
            collect_property(reader, &prim, &name, &mut |identifier, _| {
                identifiers.insert(identifier);
            });
        }
        if let Some(identifier) = reader.binary_asset_uri(&prim) {
            identifiers.insert(identifier);
        }
        if !identifiers.is_empty() {
            by_prim.insert(prim.to_string(), identifiers);
        }
    }
    by_prim
}

/// Consumer identities use the same prepared search-path contract as material
/// and DEM readers. Call after native/search preparation has been admitted.
pub fn consumed_asset_identifiers_for_prims(
    reader: &dyn UsdReadObject,
    prims: impl IntoIterator<Item = SdfPath>,
) -> std::collections::BTreeMap<String, BTreeSet<String>> {
    let mut by_prim = std::collections::BTreeMap::new();
    for prim in prims {
        if reader.is_active(&prim) != Ok(true) {
            continue;
        }
        let mut identifiers = BTreeSet::new();
        for name in reader.attr_names(&prim) {
            if !matches!(
                reader.attr_type_name(&prim, &name).as_deref(),
                Some("asset" | "asset[]")
            ) {
                continue;
            }
            let mut values = vec![(reader.attr_value(&prim, &name), None)];
            values.extend(
                reader
                    .time_sample_times(&prim, &name)
                    .into_iter()
                    .map(|time| (reader.attr_value_at(&prim, &name, time), Some(time))),
            );
            for (value, time) in values {
                let paths = match value {
                    Some(Value::AssetPath(path)) => vec![path],
                    Some(Value::AssetPathVec(paths)) => paths,
                    _ => Vec::new(),
                };
                for path in paths {
                    match crate::read::consumed_asset_identifier(
                        Some(Value::AssetPath(path)),
                        reader.native_asset_paths(),
                        reader.asset_source_layer(&prim, &name, time).as_deref(),
                        &prim,
                        &name,
                    ) {
                        Ok(Some(identifier)) if !identifier.is_empty() => {
                            identifiers.insert(identifier);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            bevy::log::error!(%error,"invalid consumed USD asset dependency")
                        }
                    }
                }
            }
        }
        if let Some(identifier) = reader.binary_asset_uri(&prim) {
            identifiers.insert(identifier);
        }
        if !identifiers.is_empty() {
            by_prim.insert(prim.to_string(), identifiers);
        }
    }
    by_prim
}

/// Native composition supplies the contributing spec sites, so only selected
/// reference/payload layers (and unresolved arcs at those sites) participate.
/// This never scans raw authored variant alternatives.
pub fn composed_layer_identifiers_for_prims(
    stage: &openusd::usd::Stage,
    prims: impl IntoIterator<Item = SdfPath>,
) -> std::collections::BTreeMap<String, BTreeSet<String>> {
    let diagnostics = stage.composition_errors();
    let mut references = std::collections::BTreeMap::new();
    for path in prims {
        let Ok(prim) = stage.prim(path.clone()) else {
            continue;
        };
        if !prim.is_active().is_ok_and(|active| active) {
            continue;
        }
        let Ok(sites) = prim.prim_stack() else {
            continue;
        };
        let mut identifiers = BTreeSet::new();
        for site in sites {
            identifiers.insert(site.layer.clone());
            for diagnostic in &diagnostics {
                use openusd::pcp::CompositionDiagnostic;
                let missing = match diagnostic {
                    CompositionDiagnostic::UnresolvedLayer {
                        asset_path,
                        introduced_by,
                        site_path,
                        ..
                    }
                    | CompositionDiagnostic::MalformedLayer {
                        asset_path,
                        introduced_by,
                        site_path,
                        ..
                    } if introduced_by == &site.layer && site_path == &site.path => {
                        Some((asset_path, introduced_by))
                    }
                    CompositionDiagnostic::UnresolvedSublayer {
                        asset_path,
                        introduced_by,
                    }
                    | CompositionDiagnostic::MalformedSublayer {
                        asset_path,
                        introduced_by,
                        ..
                    } if introduced_by == &site.layer => Some((asset_path, introduced_by)),
                    _ => None,
                };
                if let Some((asset, source)) = missing {
                    match lunco_usd_compose::canonicalize_at(
                        asset,
                        Some(&openusd::ar::ResolvedPath::new(source)),
                    ) {
                        Ok(identifier) => {
                            identifiers.insert(identifier);
                        }
                        Err(error) => {
                            bevy::log::error!(%error, "invalid composed layer dependency")
                        }
                    }
                }
            }
        }
        references.insert(path.to_string(), identifiers);
    }
    references
}

/// Discover only the authored changed properties and structurally affected
/// subtrees. Ordinary transform changes do not snapshot or scan a whole stage.
pub fn changed_native_references(
    reader: &dyn UsdReadObject,
    changes: &[crate::canonical::RawStageChange],
) -> BTreeSet<AssetReference> {
    let _span = bevy::log::info_span!("usd_native_reference_discovery").entered();
    let mut references = BTreeSet::new();
    let mut structural = BTreeSet::new();
    for change in changes {
        for property in change
            .info_only
            .iter()
            .chain(change.resynced.iter().filter(|path| path.is_property()))
        {
            let Some((prim, name)) = property.as_str().split_once('.') else {
                continue;
            };
            let Ok(prim) = SdfPath::new(prim) else {
                continue;
            };
            collect_property(reader, &prim, name, &mut |identifier, authored| {
                if let Some(reference) = AssetReference::for_asset_value(&identifier, authored) {
                    references.insert(reference);
                }
            });
        }
        for prim in change.resynced.iter().filter(|path| !path.is_property()) {
            structural.insert(prim.clone());
        }
    }
    let roots = structural
        .iter()
        .filter(|path| {
            !structural.iter().any(|parent| {
                parent != *path
                    && path
                        .as_str()
                        .strip_prefix(parent.as_str())
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut prims = BTreeSet::new();
    let mut stack = roots;
    while let Some(prim) = stack.pop() {
        if prims.insert(prim.clone()) {
            stack.extend(reader.children(&prim));
        }
    }
    references.extend(native_references_for_prims(reader, prims));
    references
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composed_requirements_select_active_variants_arrays_and_samples() {
        let root = "twin://fixture/scene.usda";
        let recipe = lunco_usd_compose::recipe::StageRecipe::new(
            root,
            std::collections::HashMap::from([(
                root.to_owned(),
                br#"#usda 1.0
(defaultPrim = "World")
def Scope "World" (
    variants = { string choice = "first" }
    prepend variantSets = "choice"
) {
    asset[] images = [@images/a.png@, @images/b.png@]
    asset frames.timeSamples = { 0: @images/frame0.png@, 1: @images/frame1.png@ }
    variantSet "choice" = {
        "first" { asset selected = @images/first.png@ }
        "second" { asset selected = @images/second.png@ }
    }
    def Scope "Inactive" (active = false) { asset omitted = @images/inactive.png@ }
}
"#
                .to_vec(),
            )]),
        );
        let plan = crate::UsdStageProjectionPlan::from_recipe(&recipe).unwrap();
        let identifiers: BTreeSet<_> = plan
            .asset_identifiers()
            .values()
            .flatten()
            .cloned()
            .collect();
        let identifiers = identifiers
            .into_iter()
            .filter(|identifier| identifier != root)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            identifiers,
            ["a", "b", "frame0", "frame1", "first"]
                .into_iter()
                .map(|name| format!("twin://fixture/images/{name}.png"))
                .collect()
        );
    }

    #[test]
    fn consumed_requirements_use_the_prepared_twin_search_identity() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("sim/images")).unwrap();
        std::fs::write(root.path().join("sim/images/present.png"), b"fixture").unwrap();
        let roots = lunco_assets_core::TwinRoots::default();
        let authority = roots.register("fixture", root.path()).unwrap();
        let source = format!("twin://{authority}/sim/scene.usda");
        let recipe = lunco_usd_compose::recipe::StageRecipe::new(
            &source,
            std::collections::HashMap::from([(
                source.clone(),
                br#"#usda 1.0
def Scope "World" {
    asset pending = @images/pending.png@
    asset present = @images/present.png@
    asset anchored = @./images/anchored.png@
}
"#
                .to_vec(),
            )]),
        );
        let mut plan = crate::UsdStageProjectionPlan::from_recipe(&recipe).unwrap();
        plan.prepare_native_asset_paths(
            Some(bevy::asset::AssetPath::parse(&source).into_owned()),
            Some(&roots),
        );
        let identities: BTreeSet<_> = plan
            .asset_identifiers()
            .values()
            .flatten()
            .cloned()
            .collect();
        assert!(identities.contains(&format!("twin://{authority}/images/pending.png")));
        assert!(identities.contains(&format!("twin://{authority}/sim/images/present.png")));
        assert!(identities.contains(&format!("twin://{authority}/sim/images/anchored.png")));
        assert!(!identities.contains(&format!("twin://{authority}/sim/images/pending.png")));
    }

    #[test]
    fn property_resync_prepares_only_its_asset_values() {
        let prefix = if cfg!(windows) {
            "file:///C:/fixture"
        } else {
            "file:///fixture"
        };
        let root = format!("{prefix}/scene.usda");
        let recipe = lunco_usd_compose::recipe::StageRecipe::new(
            &root,
            std::collections::HashMap::from([(
                root.clone(),
                br#"#usda 1.0
def Scope "World" {
    bool enabled = true
    asset image = @image.png@
    asset animated.timeSamples = { 0: @frame.png@ }
    def Scope "Child" { asset image = @child.png@ }
}
"#
                .to_vec(),
            )]),
        );
        let stage = crate::canonical::CanonicalStage::from_recipe(&recipe).unwrap();
        let refs = |path: &str| {
            changed_native_references(
                &stage.view(),
                &[crate::canonical::RawStageChange {
                    resynced: vec![SdfPath::new(path).unwrap()],
                    ..Default::default()
                }],
            )
        };
        let image = AssetReference::Native(format!("{prefix}/image.png"));
        let frame = AssetReference::Native(format!("{prefix}/frame.png"));
        let child = AssetReference::Native(format!("{prefix}/child.png"));
        assert!(refs("/World.enabled").is_empty());
        assert!(refs("/World.removed").is_empty());
        assert_eq!(refs("/World.image"), BTreeSet::from([image.clone()]));
        assert_eq!(refs("/World.animated"), BTreeSet::from([frame.clone()]));
        assert_eq!(refs("/World"), BTreeSet::from([image, frame, child]));
    }
}

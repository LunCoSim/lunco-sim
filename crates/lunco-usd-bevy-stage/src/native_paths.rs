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
    references: &mut BTreeSet<AssetReference>,
) {
    let Some(source_layer) = source_layer else {
        return;
    };
    match lunco_usd_compose::canonicalize_at(
        path.asset_path(),
        Some(&openusd::ar::ResolvedPath::new(source_layer)),
    ) {
        Ok(identifier) => {
            if let Some(reference) = AssetReference::for_asset_value(&identifier, path.asset_path())
            {
                references.insert(reference);
            }
        }
        Err(error) => bevy::log::error!(%error, "invalid composed USD asset identity"),
    }
}

fn collect_value(
    value: Option<Value>,
    source_layer: Option<&str>,
    references: &mut BTreeSet<AssetReference>,
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
    references: &mut BTreeSet<AssetReference>,
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
            collect_property(reader, &prim, &name, &mut references);
        }
        if let Some(reference) = reader.binary_asset_uri(&prim)
            && native_reference(&reference)
        {
            references.insert(AssetReference::Native(reference));
        }
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
        for property in change.info_only.iter().chain(
            change
                .resynced
                .iter()
                .filter(|path| path.is_property_path()),
        ) {
            let Some((prim, name)) = property.as_str().split_once('.') else {
                continue;
            };
            let Ok(prim) = SdfPath::new(prim) else {
                continue;
            };
            collect_property(reader, &prim, name, &mut references);
        }
        for prim in change
            .resynced
            .iter()
            .filter(|path| !path.is_property_path())
        {
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

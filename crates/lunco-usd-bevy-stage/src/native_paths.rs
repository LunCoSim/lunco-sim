//! Native asset preparation over the shared composed USD read surface.

use crate::UsdReadObject;
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::BTreeSet;

fn native_reference(reference: &str) -> bool {
    reference
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
}

fn collect_value(value: Option<Value>, references: &mut BTreeSet<String>) {
    match value {
        Some(Value::AssetPath(path)) if native_reference(path.as_str()) => {
            references.insert(path.into_string());
        }
        Some(Value::AssetPathVec(paths)) => {
            references.extend(
                paths
                    .into_iter()
                    .map(|path| path.into_string())
                    .filter(|reference| native_reference(reference)),
            );
        }
        _ => {}
    }
}

/// Read native asset-valued attributes and binary arcs from composed prims.
/// Arrays and time samples use the same typed USD owner; no domain schemas are
/// enumerated by the asset preparation boundary.
pub fn native_references_for_prims(
    reader: &dyn UsdReadObject,
    prims: impl IntoIterator<Item = SdfPath>,
) -> BTreeSet<String> {
    let mut references = BTreeSet::new();
    for prim in prims {
        for name in reader.attr_names(&prim) {
            collect_value(reader.attr_value(&prim, &name), &mut references);
            for time in reader.time_sample_times(&prim, &name) {
                collect_value(reader.attr_value_at(&prim, &name, time), &mut references);
            }
        }
        if let Some(reference) = reader.binary_asset_uri(&prim)
            && native_reference(&reference)
        {
            references.insert(reference);
        }
    }
    references
}

/// Discover only the authored changed properties and structurally affected
/// subtrees. Ordinary transform changes do not snapshot or scan a whole stage.
pub fn changed_native_references(
    reader: &dyn UsdReadObject,
    changes: &[crate::canonical::RawStageChange],
) -> BTreeSet<String> {
    let mut references = BTreeSet::new();
    let mut structural = BTreeSet::new();
    for change in changes {
        for property in &change.info_only {
            let Some((prim, name)) = property.as_str().split_once('.') else {
                continue;
            };
            let Ok(prim) = SdfPath::new(prim) else {
                continue;
            };
            collect_value(reader.attr_value(&prim, name), &mut references);
            for time in reader.time_sample_times(&prim, name) {
                collect_value(reader.attr_value_at(&prim, name, time), &mut references);
            }
        }
        for prim in &change.resynced {
            structural.insert(prim.prim_path());
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

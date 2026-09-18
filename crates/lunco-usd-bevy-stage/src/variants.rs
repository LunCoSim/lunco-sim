//! Enumerate the variants a layer or stage **offers**.
//!
//! Composition answers "which variant is selected" — [`usd::Prim::variant_sets`]
//! → `get_all_variant_selections` gives that, correctly across reference arcs.
//! It cannot answer "which variants COULD be selected", because a composed
//! stage only ever shows one selection at a time. Anything offering a choice —
//! an Inspector picker, a validation pass that probes every variant — needs the
//! authored side, and that is what this module reads.
//!
//! The trick is that variants leave their names in the *paths* of the specs they
//! contain: a variant block authors under `/Prim{set=name}`, so walking every
//! authored path and collecting the variant components enumerates the sets
//! without composing anything.
//!
//! Results are keyed **by set name**, not by prim path. A referenced layer
//! authors its variants in its own namespace while the prim a caller cares about
//! lives at the referencing path, so a strict path match finds nothing in
//! exactly the case a picker is most wanted (a wrapper scene that references a
//! scene and pins a selection). The tradeoff: two unrelated prims whose sets
//! share a name pool their options.

use std::collections::{BTreeMap, BTreeSet};

/// Variant names per set name, sorted and deduplicated.
pub type VariantOptions = BTreeMap<String, Vec<String>>;

/// Variants authored in one layer's data.
///
/// Use this when there is no stage yet — reading a `.usda` that has not been
/// composed (or cannot be).
pub fn variant_options_in_data(data: &dyn openusd::sdf::AbstractData) -> VariantOptions {
    let mut acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    collect_into(data, &mut acc);
    finish(acc)
}

/// Variants authored anywhere in a composed stage's loaded layers — including
/// those reached across reference and payload arcs, which is where a scene's
/// own variantSet lives when the caller is looking at a wrapper that references
/// it.
pub fn variant_options_in_stage(stage: &openusd::usd::Stage) -> VariantOptions {
    let mut acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in stage.layer_identifiers() {
        if let Some(layer) = stage.layer(&id) {
            collect_into(layer.data(), &mut acc);
        }
    }
    finish(acc)
}

fn collect_into(
    data: &dyn openusd::sdf::AbstractData,
    acc: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for path in data.spec_paths() {
        for component in path.components() {
            if let openusd::sdf::PathComponent::Variant { set, selection } = component {
                // `{set=}` — the bare variant-set path — is the container, not a
                // selectable variant.
                if !selection.is_empty() {
                    acc.entry(set.to_string())
                        .or_default()
                        .insert(selection.to_string());
                }
            }
        }
    }
}

fn finish(acc: BTreeMap<String, BTreeSet<String>>) -> VariantOptions {
    acc.into_iter()
        .map(|(k, v)| (k, v.into_iter().collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_VARIANTS: &str = r#"#usda 1.0
def Xform "Traverse" (
    prepend variantSets = "terrain"
    variants = { string terrain = "apollo15" }
)
{
    variantSet "terrain" = {
        "apollo15" { double3 xformOp:translate = (1, 0, 0) }
        "change4"  { double3 xformOp:translate = (2, 0, 0) }
    }
}
"#;

    /// Both variants are enumerated — including the one NOT selected, which is
    /// the whole reason this exists (composition would only ever show
    /// `apollo15`).
    #[test]
    fn enumerates_every_variant_not_just_the_selected_one() {
        let data = openusd::usda::parse(TWO_VARIANTS).expect("parse");
        let opts = variant_options_in_data(&data);
        assert_eq!(
            opts.get("terrain").map(Vec::as_slice),
            Some(["apollo15".to_string(), "change4".to_string()].as_slice()),
        );
    }

    /// A layer with no variantSet yields nothing rather than an empty-named set.
    #[test]
    fn plain_layer_offers_no_variants() {
        let data = openusd::usda::parse("#usda 1.0\ndef Xform \"A\" { }\n").expect("parse");
        assert!(variant_options_in_data(&data).is_empty());
    }
}

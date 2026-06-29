//! Path-addressed USD authoring for the document layer (Phase C2/C3).
//!
//! The document layer used to mutate its canonical `.usda` **source text** by
//! splicing byte ranges (`lunco-usd::text_edit`). That is the CQ-503 nested-child
//! corruption class: editing `/World/Box.radius` could clobber
//! `/World/Box/Inner.radius` because the two share an attribute name and the
//! splicer reasons about text, not structure.
//!
//! This module routes every edit through openusd's authoring engine instead.
//! The document's canonical representation is a Send-safe [`sdf::Data`] holding
//! the **root layer's own authored specs** (references, payloads, sublayers and
//! all — composition is NOT flattened in, so the data still round-trips
//! losslessly with external USD tools). To apply an edit we:
//!
//!   1. Serialize the current data back to USDA text and re-open it as the root
//!      layer of a transient [`Stage`] through a stub resolver (every external
//!      arc resolves to an empty layer — we never traverse the composition, so
//!      missing referenced files don't matter).
//!   2. Author the edit by **SDF path** (`define_prim`, `create_attribute`,
//!      `remove_prim`) into that root layer. Path-addressed authoring cannot
//!      touch a sibling or nested prim that happens to share a name.
//!   3. Extract the root layer's authored specs back out as a fresh
//!      [`sdf::Data`] ([`extract_root_layer_data`]) — references untouched,
//!      because the edit only added/removed opinions on the root layer.
//!
//! `Stage` is `!Send` (`Rc`-backed), so it never escapes a synchronous call; the
//! `sdf::Data` in and out is the Send-safe handoff the rest of the stack uses.

use anyhow::{anyhow, Result};
use openusd::ar::{self, ResolvedPath};
use openusd::sdf::{self, Path as SdfPath, SpecData, Value};
use openusd::usd::Stage;
use openusd::usda;

/// Synthetic identifier for a document's in-memory root layer. Ends in `.usda`
/// so openusd parses it as text.
const DOC_ROOT_ID: &str = "lunco://__lunco_document_root__.usda";

const EMPTY_USDA: &[u8] = b"#usda 1.0\n";

/// Resolver that serves the document's root layer from memory and routes every
/// other arc (sublayers, references, payloads) to an empty stub. The document
/// authoring path never traverses the composed stage — it only reads back the
/// root layer's own specs — so stubbing external arcs is lossless: the root's
/// authored `references`/`payload`/`subLayers` opinions survive in
/// [`extract_root_layer_data`]; only their *resolution* is skipped.
struct DocResolver {
    root_bytes: Vec<u8>,
}

impl ar::Resolver for DocResolver {
    fn create_identifier(&self, asset_path: &str, _anchor: Option<&ResolvedPath>) -> String {
        // Every non-root arc collapses to the same stub id; the root keeps its
        // own. No anchoring needed — stub bytes are identical everywhere.
        asset_path.to_string()
    }

    fn resolve(&self, asset_path: &str) -> Option<ResolvedPath> {
        Some(ResolvedPath::new(asset_path))
    }

    fn resolve_for_new_asset(&self, asset_path: &str) -> Option<ResolvedPath> {
        Some(ResolvedPath::new(asset_path))
    }

    fn open_asset(&self, resolved_path: &ResolvedPath) -> std::io::Result<Box<dyn ar::Asset>> {
        let key = resolved_path.to_str().unwrap_or_default();
        if key == DOC_ROOT_ID {
            Ok(Box::new(std::io::Cursor::new(self.root_bytes.clone())))
        } else {
            Ok(Box::new(std::io::Cursor::new(EMPTY_USDA.to_vec())))
        }
    }

    fn get_modification_timestamp(
        &self,
        _asset_path: &str,
        _resolved_path: &ResolvedPath,
    ) -> Option<std::time::SystemTime> {
        None
    }
}

/// Serialize `data` to USDA text. The canonical round-trip used both for
/// authoring (re-open as a stage) and for saving the document to disk.
pub fn data_to_usda(data: &sdf::Data) -> Result<String> {
    usda::TextWriter::write_to_string(data).map_err(|e| anyhow!("serialize layer to USDA: {e}"))
}

/// Parse USDA `text` into a single root layer's [`sdf::Data`] (no composition).
/// Inverse of [`data_to_usda`]; used to load a document and to apply a
/// full-source replacement.
pub fn usda_to_data(text: &str) -> Result<sdf::Data> {
    usda::parse(text).map_err(|e| anyhow!("USD parse error: {e}"))
}

/// Open `data` as the root layer of a transient, writable [`Stage`]. Authoring
/// calls (`stage.define_prim`, `stage.create_attribute`, `stage.remove_prim`)
/// target the root layer by default, so callers author directly on the returned
/// stage and then pass it to [`extract_root_layer_data`].
///
/// `Stage` is `!Send`; keep it on the stack of one synchronous edit.
pub fn open_doc_stage(data: &sdf::Data) -> Result<Stage> {
    let text = data_to_usda(data)?;
    let resolver = DocResolver {
        root_bytes: text.into_bytes(),
    };
    Stage::builder()
        .resolver(resolver)
        .open(DOC_ROOT_ID)
        .map_err(|e| anyhow!("open document stage: {e}"))
}

/// Extract the **root layer's** authored specs from `stage` as a fresh
/// [`sdf::Data`]. This is the document's new canonical representation after an
/// edit — references/payloads/sublayers are preserved verbatim because the edit
/// only altered opinions on the root layer; the composition is never flattened
/// in.
pub fn extract_root_layer_data(stage: &Stage) -> Result<sdf::Data> {
    sdf::Data::from_abstract(stage.root_layer().data())
        .map_err(|e| anyhow!("extract root layer data: {e}"))
}

/// Author a `references = @asset_path@` arc onto the prim at `prim_path` in
/// `data` (an explicit single-item reference list — `asset_path` is the bare
/// path, no `@` delimiters; the serializer adds them). The prim spec must
/// already exist — a reference is prim metadata, so the caller defines the prim
/// first (e.g. [`open_doc_stage`] + `define_prim` + [`extract_root_layer_data`]).
///
/// openusd's Stage authoring API exposes no `add_reference`, so this is authored
/// at the `sdf` level — the symmetric counterpart of how `compose` *reads*
/// `Value::ReferenceListOp`, kept here so the op layer never hand-builds openusd
/// value variants. The reference survives `data_to_usda` as a `references = @…@`
/// metadata opinion and is resolved at render time by the PCP composer.
pub fn author_reference(data: &mut sdf::Data, prim_path: &SdfPath, asset_path: &str) -> Result<()> {
    let spec = data
        .spec_mut(prim_path)
        .ok_or_else(|| anyhow!("author_reference: no prim spec at {prim_path}"))?;
    let reference = sdf::Reference {
        asset_path: asset_path.to_string(),
        ..Default::default()
    };
    spec.add(
        "references",
        Value::ReferenceListOp(sdf::ReferenceListOp::explicit([reference])),
    );
    Ok(())
}

/// Remove the time sample at `time` from the attribute at `attr_path`, the
/// symmetric counterpart of authoring one via openusd's `Attribute::set_at`.
/// Returns the removed [`Value`] (so the op layer can build a typed inverse) or
/// `None` if the attribute carried no sample at that exact time. When the last
/// sample is removed the `timeSamples` field is cleared entirely, so the spec
/// round-trips identically to one that never authored samples.
///
/// openusd's Stage `Attribute` API exposes sample *authoring* (`set_at`) but no
/// per-sample erase, so this drops to the `sdf` spec level — the same level at
/// which [`author_reference`] writes reference metadata.
pub fn remove_time_sample(data: &mut sdf::Data, attr_path: &SdfPath, time: f64) -> Result<Option<Value>> {
    let Some(spec) = data.spec_mut(attr_path) else {
        return Ok(None);
    };
    let Some(Value::TimeSamples(mut map)) = spec.get("timeSamples").cloned() else {
        return Ok(None);
    };
    // `total_cmp` matches the total ordering openusd's `set_time_sample` uses, so
    // a sample authored at any `f64` (incl. signed zero / NaN) is locatable here.
    let Some(idx) = map.iter().position(|(t, _)| t.total_cmp(&time).is_eq()) else {
        return Ok(None);
    };
    let (_, removed) = map.remove(idx);
    if map.is_empty() {
        spec.remove("timeSamples");
    } else {
        spec.add("timeSamples", Value::TimeSamples(map));
    }
    Ok(Some(removed))
}

/// Parse a single attribute value literal (e.g. `"(1, 0, 0)"`, `"0.5"`,
/// `"(0.2, 0.2, 0.8)"`) of the given USD `type_name` into an [`sdf::Value`], by
/// embedding it in a throwaway USDA snippet and letting openusd's own parser do
/// the typing. General over every value type the parser understands, so the
/// document op layer doesn't reimplement USD value literal parsing.
pub fn parse_attribute_value(type_name: &str, literal: &str) -> Result<Value> {
    let snippet = format!(
        "#usda 1.0\ndef \"_v\"\n{{\n    {type_name} _a = {literal}\n}}\n"
    );
    let data = usda_to_data(&snippet)?;
    let attr = SdfPath::new("/_v._a").map_err(|e| anyhow!("attr path: {e}"))?;
    data.spec(&attr)
        .and_then(|s| s.get("default"))
        .cloned()
        .ok_or_else(|| anyhow!("could not parse `{type_name} = {literal}` into a USD value"))
}

/// Overlay the `runtime` layer's opinions onto `base` as an **sdf layer-stack
/// merge** and return the composed [`sdf::Data`]: runtime fields win per spec,
/// runtime-only specs are added, and `primChildren` lists are unioned so the
/// merged prim tree lists children authored in either layer.
///
/// This is the sdf-level layer composition openusd does **not** expose. Its
/// only flatten (`compose::flatten_stage`) runs full PCP — it resolves
/// references/payloads/variants, which is exactly wrong for an *authored*
/// composed view: a base `references = @asset.usda@` opinion must survive as an
/// opinion, not be pulled in. References/payloads ride along untouched here
/// because they are just fields on the base spec. This is NOT a substitute for
/// render-time PCP composition (that is [`compose_native_fs`] /
/// `compose::compose_to_data`); it is the document's own two-layer (base +
/// runtime) merge.
pub fn compose_layers(base: &sdf::Data, runtime: &sdf::Data) -> sdf::Data {
    let mut out = base.clone();
    for (path, rspec) in runtime.iter() {
        match out.spec_mut(path) {
            Some(bspec) => {
                for (key, value) in &rspec.fields {
                    if key == "primChildren" {
                        union_prim_children(bspec, value);
                    } else {
                        // Runtime opinion wins (stronger layer). `add` upserts
                        // in place, preserving field order.
                        bspec.add(key, value.clone());
                    }
                }
            }
            None => {
                // Runtime-only spec: copy it in wholesale.
                *out.create_spec(path.clone(), rspec.ty) = rspec.clone();
            }
        }
    }
    out
}

/// Union a runtime `primChildren` token list into the base spec's, preserving
/// base order and appending only names base doesn't already list — so a runtime
/// `over` that adds one child doesn't drop the base's siblings.
fn union_prim_children(bspec: &mut SpecData, rval: &Value) {
    let Value::TokenVec(rkids) = rval else {
        bspec.add("primChildren", rval.clone());
        return;
    };
    match bspec.get_mut("primChildren") {
        Some(Value::TokenVec(bkids)) => {
            for k in rkids {
                if !bkids.iter().any(|x| x == k) {
                    bkids.push(k.clone());
                }
            }
        }
        _ => bspec.add("primChildren", rval.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd_data::UsdDataExt;

    const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\ndef Xform \"World\"\n{\n    def Sphere \"Box\"\n    {\n        double radius = 1\n        def Sphere \"Inner\"\n        {\n            double radius = 9\n        }\n    }\n}\n";

    /// A reference arc authored onto a defined prim survives serialization back
    /// to USDA and re-parsing — the load-bearing property for the C4b spawn
    /// producer (a spawn = a runtime prim that `references` its asset).
    #[test]
    fn author_reference_round_trips_through_usda() {
        // Define the spawn prim exactly as `AddPrim` does (stage round-trip),
        // then author the reference into the extracted data.
        let stage = open_doc_stage(&usda_to_data("#usda 1.0\ndef Xform \"World\"\n{\n}\n").unwrap())
            .unwrap();
        stage.define_prim("/World/spawn_1").unwrap();
        let mut data = extract_root_layer_data(&stage).unwrap();
        let prim = SdfPath::new("/World/spawn_1").unwrap();
        author_reference(&mut data, &prim, "vessels/rover.usda").unwrap();

        // Serializes as a `references = @…@` opinion...
        let text = data_to_usda(&data).unwrap();
        assert!(text.contains("@vessels/rover.usda@"), "USDA must carry the reference:\n{text}");

        // ...and survives a re-parse as an explicit ReferenceListOp.
        let reparsed = usda_to_data(&text).unwrap();
        match reparsed.spec(&prim).and_then(|s| s.get("references")) {
            Some(Value::ReferenceListOp(op)) => {
                assert_eq!(op.explicit_items.len(), 1);
                assert_eq!(op.explicit_items[0].asset_path, "vessels/rover.usda");
            }
            other => panic!("expected a ReferenceListOp at {prim}, got {other:?}"),
        }
    }

    /// THE load-bearing property: a resolver-opened root layer is writable,
    /// path-addressed authoring lands in it, and `extract_root_layer_data`
    /// reflects the change — proving the whole C2/C3 mechanic.
    #[test]
    fn author_round_trips_through_stage() {
        let data = usda_to_data(SCENE).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        // Author a brand-new prim by SDF path.
        stage.define_prim("/World/Rover").unwrap().set_type_name("Xform").unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World/Rover").unwrap()).as_deref(),
            Some("Xform"),
            "newly authored prim must appear in the extracted root layer data"
        );
        // Pre-existing structure survives.
        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World/Box").unwrap()).as_deref(),
            Some("Sphere")
        );
    }

    /// CQ-503 anti-corruption property: setting the PARENT's `radius` must not
    /// touch the nested CHILD's same-named `radius`. This is exactly what text
    /// splicing could not guarantee.
    #[test]
    fn parent_attribute_edit_does_not_clobber_nested_child() {
        let data = usda_to_data(SCENE).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        let val = parse_attribute_value("double", "2").unwrap();
        stage.create_attribute("/World/Box.radius", "double").unwrap().set(val).unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        assert_eq!(
            out.prim_attribute_value::<f64>(&SdfPath::new("/World/Box").unwrap(), "radius"),
            Some(2.0),
            "parent radius override must win"
        );
        assert_eq!(
            out.prim_attribute_value::<f64>(&SdfPath::new("/World/Box/Inner").unwrap(), "radius"),
            Some(9.0),
            "nested child's same-named radius must be untouched (CQ-503)"
        );
    }

    /// References authored on the root layer must survive an edit — the doc
    /// canonical is the root layer, NOT the flattened composition, so a
    /// `references = @other.usda@` opinion is preserved even though the stub
    /// resolver never resolves it.
    #[test]
    fn references_survive_authoring() {
        let with_ref = "#usda 1.0\ndef Xform \"World\"\n{\n    def \"Vehicle\" (\n        references = @vessels/rover.usda@\n    )\n    {\n    }\n}\n";
        let data = usda_to_data(with_ref).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        stage.define_prim("/World/Light").unwrap().set_type_name("DistantLight").unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        let vehicle = SdfPath::new("/World/Vehicle").unwrap();
        assert!(
            out.spec(&vehicle).and_then(|s| s.get("references")).is_some(),
            "root-layer reference opinion must survive an unrelated edit"
        );
        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World/Light").unwrap()).as_deref(),
            Some("DistantLight")
        );
    }

    /// `compose_layers` overlays runtime onto base: runtime-only prims appear,
    /// runtime attribute opinions win, base structure (incl. unrelated children)
    /// survives, and base references are preserved as opinions (not resolved).
    #[test]
    fn compose_layers_overlays_runtime_onto_base() {
        let base = usda_to_data(SCENE).unwrap(); // /World/Box(radius=1)/Inner(radius=9)
        // Runtime: override Box.radius and add a new sibling prim under /World.
        let runtime = usda_to_data(
            "#usda 1.0\nover \"World\"\n{\n    over \"Box\"\n    {\n        double radius = 7\n    }\n    def Sphere \"Obstacle\"\n    {\n    }\n}\n",
        )
        .unwrap();

        let composed = compose_layers(&base, &runtime);

        // Runtime attribute opinion wins.
        assert_eq!(
            composed.prim_attribute_value::<f64>(&SdfPath::new("/World/Box").unwrap(), "radius"),
            Some(7.0)
        );
        // Base-only nested child survives untouched.
        assert_eq!(
            composed.prim_attribute_value::<f64>(&SdfPath::new("/World/Box/Inner").unwrap(), "radius"),
            Some(9.0)
        );
        // Runtime-only prim is present in the composed tree...
        assert_eq!(
            composed.prim_type_name(&SdfPath::new("/World/Obstacle").unwrap()).as_deref(),
            Some("Sphere")
        );
        // ...and `prim_children` of /World lists BOTH the base Box and the runtime Obstacle.
        let kids: Vec<String> = composed
            .prim_children(&SdfPath::new("/World").unwrap())
            .iter()
            .map(|p| p.name().unwrap_or_default().to_string())
            .collect();
        assert!(kids.contains(&"Box".to_string()), "base child survives: {kids:?}");
        assert!(kids.contains(&"Obstacle".to_string()), "runtime child added: {kids:?}");
    }

    /// An empty runtime layer composes to exactly the base.
    #[test]
    fn compose_layers_empty_runtime_is_base() {
        let base = usda_to_data(SCENE).unwrap();
        let empty = usda_to_data("#usda 1.0\n").unwrap();
        let composed = compose_layers(&base, &empty);
        assert_eq!(
            composed.prim_type_name(&SdfPath::new("/World/Box").unwrap()).as_deref(),
            Some("Sphere")
        );
    }

    /// `remove_prim` drops the whole subtree by path.
    #[test]
    fn remove_prim_drops_subtree() {
        let data = usda_to_data(SCENE).unwrap();
        let stage = open_doc_stage(&data).unwrap();

        stage.remove_prim("/World/Box").unwrap();
        let out = extract_root_layer_data(&stage).unwrap();

        assert!(out.spec(&SdfPath::new("/World/Box").unwrap()).is_none());
        assert!(out.spec(&SdfPath::new("/World/Box/Inner").unwrap()).is_none());
        assert_eq!(
            out.prim_type_name(&SdfPath::new("/World").unwrap()).as_deref(),
            Some("Xform"),
            "sibling structure survives"
        );
    }
}
